use actix::prelude::*;
use anyhow::Ok;
use anyhow::Result;
use chrono::{Duration, Utc};
use clap::Parser;
use self_update::{backends::github::Update, cargo_crate_version, update::UpdateStatus};
use tokio::time::{sleep, Duration as TokioDuration};
use whiz::{
    actors::{command::CommandActor, console::ConsoleActor, watcher::WatcherActor},
    args::Command,
    config::Config,
    global_config::GlobalConfig,
    utils::recurse_config_file,
};

use std::process;

use whiz::args::Args;

async fn upgrade_check() -> Result<()> {
    let project = directories::ProjectDirs::from("com", "zifeo", "whiz")
        .expect("cannot get directory for projet");

    let config_path = project.config_local_dir().join("config.yml");
    let mut local_config = GlobalConfig::load(config_path.clone()).await?;

    if local_config.update_check + Duration::days(1) < Utc::now() {
        let current_version = cargo_crate_version!();
        let latest = tokio::task::spawn_blocking(move || {
            let update = Update::configure()
                .repo_owner("zifeo")
                .repo_name("whiz")
                .bin_name("whiz")
                .current_version(current_version)
                .build()?;

            Ok(update.get_latest_release()?)
        })
        .await??;

        if latest.version != current_version {
            println!(
                "New whiz update available: {} -> {} (use: whiz upgrade)",
                current_version, latest.version
            );
            println!("Will resume in 5 seconds...");
            sleep(TokioDuration::from_secs(5)).await;
        }
        local_config.update_check = Utc::now();
        local_config.save(config_path).await?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let system = System::with_tokio_rt(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap()
    });

    Arbiter::current().spawn(async { run().await.unwrap() });

    system.run()?;

    Ok(())
}

async fn run() -> Result<()> {
    upgrade_check()
        .await
        .unwrap_or_else(|e| eprintln!("cannot check for update: {}", e));

    let args = Args::parse();
    #[cfg(target_os = "windows")]
    std::env::set_var(
        "PWD",
        std::env::current_dir()
            .expect("could not read current directory")
            .to_str()
            .unwrap(),
    );

    if let Some(command) = args.command {
        match command {
            Command::Upgrade(opts) => {
                tokio::task::spawn_blocking(move || {
                    let mut update = Update::configure();
                    update
                        .repo_owner("zifeo")
                        .repo_name("whiz")
                        .bin_name("whiz")
                        .show_download_progress(true)
                        .current_version(cargo_crate_version!())
                        .no_confirm(opts.yes);

                    if let Some(version) = opts.version {
                        update.target_version_tag(&format!("v{version}"));
                    }

                    match update.build()?.update_extended()? {
                        UpdateStatus::UpToDate => println!("Already up to date!"),
                        UpdateStatus::Updated(release) => {
                            println!("Updated successfully to {}!", release.version);
                            println!(
                                "Release notes: https://github.com/zifeo/whiz/releases/tag/{}",
                                release.name
                            );
                        }
                    };
                    Ok(())
                })
                .await??;
            }
        }
        process::exit(0);
    };

    let (config_file, config_path) = recurse_config_file(&args.file).unwrap_or_else(|err| {
        eprintln!("file error: {}", err);
        process::exit(1);
    });

    let mut config = Config::from_file(&config_file).unwrap_or_else(|err| {
        eprintln!("config error: {}", err);
        process::exit(2);
    });

    let pipes_map = config.get_pipes_map().unwrap_or_else(|err| {
        eprintln!("config error: {}", err);
        process::exit(3);
    });

    if let Err(err) = config.filter_jobs(&args.run) {
        println!("argument error: {}", err);
        process::exit(4);
    };

    if args.list_jobs {
        let formatted_list_of_jobs = config.get_formatted_list_of_jobs();
        println!("List of jobs:\n{formatted_list_of_jobs}");
        process::exit(0);
    }

    let base_dir = config_path.parent().unwrap().to_path_buf();

    let console =
        ConsoleActor::new(Vec::from_iter(config.ops.keys().cloned()), args.timestamp).start();
    let watcher = WatcherActor::new(base_dir.clone()).start();
    CommandActor::from_config(
        &config,
        console.clone(),
        watcher,
        base_dir.clone(),
        args.verbose,
        pipes_map,
    )
    .await
    .unwrap_or_else(|err| {
        println!("error spawning commands: {}", err);
        System::current().stop();
        process::exit(9);
    });

    Ok(())
}

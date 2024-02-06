use actix::prelude::*;
use anyhow::anyhow;
use anyhow::Ok;
use anyhow::Result;
use chrono::{Duration, Utc};
use clap::Parser;
use self_update::{backends::github::Update, cargo_crate_version, update::UpdateStatus};
use semver::Version;
use std::eprintln;
use tokio::time::{sleep, Duration as TokioDuration};
use whiz::actors::command::CommandActorsBuilder;
use whiz::config::ops;
use whiz::config::ConfigBuilder;
use whiz::serial_mode;
use whiz::utils::find_config_path;
use whiz::{
    actors::{console::ConsoleActor, watcher::WatcherActor},
    args::Command,
    config::Config,
    global_config::GlobalConfig,
};
mod graph;

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

        if Version::parse(&latest.version)? > Version::parse(current_version)? {
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
    let args = Args::parse();

    if args.version {
        println!("whiz {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if let Some(Command::Upgrade(opts)) = args.command {
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
        return Ok(());
    };

    let system = System::with_tokio_rt(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap()
    });

    Arbiter::current().spawn(async {
        run(args).await.unwrap_or_else(|e| {
            eprintln!("{}", e);
            System::current().stop_with_code(1);
        });
    });

    let code = system.run_with_code()?;
    std::process::exit(code);
}

async fn run(args: Args) -> Result<()> {
    #[cfg(target_os = "windows")]
    std::env::set_var(
        "PWD",
        std::env::current_dir()
            .expect("could not read current directory")
            .to_str()
            .unwrap(),
    );

    upgrade_check()
        .await
        .unwrap_or_else(|e| eprintln!("cannot check for update: {}", e));

    let config = ConfigBuilder::new(find_config_path(
        &std::env::current_dir().unwrap(),
        &args.file,
    )?)
    .build()?;

    let Some(command) = args.command.as_ref() else {
        return start_default_mode(config, args).await;
    };

    match command {
        Command::Upgrade(_) => {
            unreachable!();
        }

        Command::ListJobs => {
            let formatted_list_of_jobs = ops::get_formatted_list_of_jobs(&config.ops);
            println!("List of jobs:\n{formatted_list_of_jobs}");
            System::current().stop_with_code(0);
            return Ok(());
        }

        Command::Graph(opts) => {
            let filtered_tasks: Vec<graph::Task> = config
                .ops
                .iter()
                .map(|task| graph::Task {
                    name: task.0.to_owned(),
                    depends_on: task.1.depends_on.resolve(),
                })
                .collect();

            match graph::draw_graph(filtered_tasks, opts.boxed)
                .map_err(|err| anyhow!("Error visualizing graph: {}", err))
            {
                Result::Ok(..) => {
                    System::current().stop_with_code(0);
                    return Ok(());
                }
                Err(e) => {
                    System::current().stop_with_code(1);
                    return Err(e);
                }
            };
        }

        Command::Execute(opts) => {
            serial_mode::start(opts, config).await?;
            System::current().stop_with_code(0);
            return Ok(());
        }
    }
}

async fn start_default_mode(config: Config, args: Args) -> Result<()> {
    let console =
        ConsoleActor::new(Vec::from_iter(config.ops.keys().cloned()), args.timestamp).start();
    let watcher = WatcherActor::new(config.base_dir.clone()).start();

    let base_dir = config.base_dir.clone();
    let colors_map = config.colors_map.clone();
    let pipes_map = config.pipes_map.clone();

    let cmds = CommandActorsBuilder::new(
        config,
        console.clone(),
        watcher,
        base_dir,   // TODO remove param
        colors_map, // TODO: remove param
    )
    .verbose(args.verbose)
    .pipes_map(pipes_map.clone()) // TODO: remove
    .globally_enable_watch(if args.exit_after { false } else { args.watch })
    .build()
    .await
    .map_err(|err| anyhow!("error spawning commands: {}", err))?;

    if args.exit_after {
        whiz::actors::grim_reaper::GrimReaperActor::start_new(cmds).await?;
    }

    Ok(())
}

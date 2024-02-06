use actix::prelude::*;
use anyhow::anyhow;
use anyhow::Ok;
use anyhow::Result;
use chrono::{Duration, Utc};
use clap::Parser;
use self_update::{backends::github::Update, cargo_crate_version, update::UpdateStatus};
use semver::Version;
use std::collections::HashMap;
use std::eprintln;
use std::fs::File;
use std::path::PathBuf;
use tokio::time::{sleep, Duration as TokioDuration};
use whiz::actors::command::CommandActorsBuilder;
use whiz::config::color::ColorOption;
use whiz::config::pipe::Pipe;
use whiz::serial_mode;
use whiz::{
    actors::{console::ConsoleActor, watcher::WatcherActor},
    args::Command,
    config::Config,
    global_config::GlobalConfig,
    utils::recurse_config_file,
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

struct ExtendedConfig {
    config: Config,
    base_dir: PathBuf,
    pipes_map: HashMap<String, Vec<Pipe>>,
    colors_map: HashMap<String, Vec<ColorOption>>,
}

impl ExtendedConfig {
    fn new(config_file: File, config_path: PathBuf, filter: &[String]) -> Result<Self> {
        let mut config =
            Config::from_file(&config_file).map_err(|err| anyhow!("config error: {}", err))?;

        let pipes_map = config
            .get_pipes_map()
            .map_err(|err| anyhow!("dag error: {}", err))?;

        let colors_map = config
            .get_colors_map()
            .map_err(|err| anyhow!("colors error: {}", err))?;

        config
            .filter_jobs(filter)
            .map_err(|err| anyhow!("argument error: {}", err))?;

        Ok(Self {
            config,
            base_dir: config_path.parent().unwrap().to_path_buf(),
            colors_map,
            pipes_map,
        })
    }
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

    let (config_file, config_path) =
        recurse_config_file(&args.file).map_err(|err| anyhow!("file error: {}", err))?;

    let Some(command) = args.command.as_ref() else {
        return start_default_mode(
            ExtendedConfig::new(config_file, config_path, &args.run)?,
            args,
        )
        .await;
    };

    match command {
        Command::Upgrade(_) => {
            unreachable!();
        }

        Command::ListJobs => {
            let config =
                Config::from_file(&config_file).map_err(|err| anyhow!("config error: {}", err))?;

            let formatted_list_of_jobs = config.get_formatted_list_of_jobs();
            println!("List of jobs:\n{formatted_list_of_jobs}");
            System::current().stop_with_code(0);
            return Ok(());
        }

        Command::Graph(opts) => {
            let config =
                Config::from_file(&config_file).map_err(|err| anyhow!("config error: {}", err))?;

            let filtered_tasks: Vec<graph::Task> = config
                .ops
                .into_iter()
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
            let base_dir = config_path.parent().unwrap().to_path_buf();
            serial_mode::start(opts, config_file, base_dir).await?;
            System::current().stop_with_code(0);
            return Ok(());
        }
    }
}

async fn start_default_mode(extended_config: ExtendedConfig, args: Args) -> Result<()> {
    let ExtendedConfig {
        config,
        base_dir,
        pipes_map,
        colors_map,
    } = extended_config;

    let console =
        ConsoleActor::new(Vec::from_iter(config.ops.keys().cloned()), args.timestamp).start();
    let watcher = WatcherActor::new(base_dir.clone()).start();
    let cmds = CommandActorsBuilder::new(
        config,
        console.clone(),
        watcher,
        base_dir.clone(),
        colors_map,
    )
    .verbose(args.verbose)
    .pipes_map(pipes_map)
    .globally_enable_watch(if args.exit_after { false } else { args.watch })
    .build()
    .await
    .map_err(|err| anyhow!("error spawning commands: {}", err))?;

    if args.exit_after {
        whiz::actors::grim_reaper::GrimReaperActor::start_new(cmds).await?;
    }

    Ok(())
}

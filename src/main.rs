use actix::prelude::*;
use whiz::{
    actors::{command::CommandActor, console::ConsoleActor, watcher::WatcherActor},
    config::Config,
    utils::recurse_default_config,
};

use anyhow::Result;
use std::env;

use std::process;

use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long)]
    file: Option<String>,

    #[clap(short, long)]
    verbose: bool,

    #[clap(short, long)]
    timestamp: bool,

    /// Run specific jobs
    #[clap(short, long, value_name = "JOB")]
    run: Vec<String>,

    /// List all the jobs set in the config file
    #[clap(long)]
    list_jobs: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let default_file = "whiz.yaml";
    let config_file = match args.file {
        Some(given_path) => given_path,
        None => match recurse_default_config(default_file) {
            Ok(path) => path.display().to_string(),
            Err(err) => {
                println!("file error: {}", err);
                process::exit(1);
            }
        },
    };

    let mut config = match Config::from_file(&config_file) {
        Ok(conf) => conf,
        Err(err) => {
            println!("file error: {}", err);
            process::exit(2);
        }
    };

    if let Err(err) = config.filter_jobs(&args.run) {
        println!("argument error: {}", err);
        process::exit(3);
    };

    if let Err(err) = config.build_dag() {
        println!("config error: {}", err);
        process::exit(4);
    };

    config.simplify_dependencies();

    if args.list_jobs {
        let formatted_list_of_jobs = config.get_formatted_list_of_jobs();
        println!("List of jobs:\n{formatted_list_of_jobs}");
        process::exit(0);
    }

    let base_dir = env::current_dir()?
        .join(config_file)
        .parent()
        .unwrap()
        .to_path_buf();

    let system = System::new();
    let exec = async move {
        let console =
            ConsoleActor::new(Vec::from_iter(config.ops.keys().cloned()), args.timestamp).start();
        let watcher = WatcherActor::new().start();
        CommandActor::from_config(&config, console, watcher, base_dir.clone(), args.verbose);
    };

    Arbiter::current().spawn(exec);

    system.run()?;
    Ok(())
}

use actix::prelude::*;
use whiz::{
    actors::{command::CommandActor, console::ConsoleActor, watcher::WatcherActor},
    config::Config,
    utils::recurse_config_file,
};

use anyhow::Result;

use std::process;

use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, default_value = "whiz.yaml")]
    file: String,

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

    let (config_file, config_path) = {
        match recurse_config_file(&args.file) {
            Ok(result) => result,
            Err(err) => {
                println!("file error: {}", err);
                process::exit(1);
            }
        }
    };

    let base_dir = config_path.parent().unwrap().to_path_buf();

    let mut config = match Config::from_file(&config_file, &base_dir) {
        Ok(conf) => conf,
        Err(err) => {
            println!("config error: {}", err);
            process::exit(2);
        }
    };

    if let Err(err) = config.filter_jobs(&args.run) {
        println!("argument error: {}", err);
        process::exit(3);
    };

    if args.list_jobs {
        let formatted_list_of_jobs = config.get_formatted_list_of_jobs();
        println!("List of jobs:\n{formatted_list_of_jobs}");
        process::exit(0);
    }

    let system = System::new();
    let exec = async move {
        let console =
            ConsoleActor::new(Vec::from_iter(config.ops.keys().cloned()), args.timestamp).start();
        let watcher = WatcherActor::new(base_dir.clone()).start();
        CommandActor::from_config(&config, console, watcher, base_dir.clone(), args.verbose);
    };

    Arbiter::current().spawn(exec);

    system.run()?;
    Ok(())
}

use actix::prelude::*;
use whiz::{
    actors::{command::CommandActor, console::ConsoleActor, watcher::WatcherActor},
    config::Config,
    utils::recurse_default_config
};

use anyhow::Result;
use std::env;

use std::process;

use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, default_value = "")]
    file: String,

    #[clap(short, long)]
    verbose: bool,

    #[clap(short, long)]
    timestamp: bool,

    /// Run specific jobs
    #[clap(short, long, value_name = "JOB")]
    run: Vec<String>,
}

fn main() -> Result<()> {
    let mut args = Args::parse();
    if args.file.is_empty() {
        args.file = recurse_default_config("whiz.yaml");
    }
    
    let args = args;

    let mut config = match Config::from_file(&args.file) {
        Ok(conf) => conf,
        Err(err) => {
            println!("file error: {}", err);
            process::exit(1);
        }
    };

    if let Err(err) = config.filter_jobs(&args.run) {
        println!("argument error: {}", err);
        process::exit(2);
    };

    if let Err(err) = config.build_dag() {
        println!("config error: {}", err);
        process::exit(3);
    };

    let base_dir = env::current_dir()?
        .join(args.file)
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

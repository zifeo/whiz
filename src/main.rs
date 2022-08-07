use actix::prelude::*;
use command_actor::CommandActor;

use std::collections::HashMap;
use std::error::Error;
use std::{env, io};
mod command_actor;
mod config;
mod console_actor;
mod watcher_actor;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, value_parser, default_value = "whiz.yaml")]
    file: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let conf = match config::Config::from_file(&args.file) {
        Ok(conf) => conf,
        Err(err) => {
            println!("file error: {}", err);
            process::exit(1);
        }
    };

    let dag = match conf.build_dag() {
        Ok(conf) => conf,
        Err(err) => {
            println!("config error: {}", err);
            process::exit(2);
        }
    };

    let working_dir = env::current_dir()?
        .join(args.file)
        .parent()
        .unwrap()
        .to_path_buf();

    let system = System::new();
    let exec = async move {
        let console =
            console_actor::ConsoleActor::new(Vec::from_iter(conf.ops.keys().rev())).start();
        let watcher = watcher_actor::WatcherActor::new().start();

        let mut commands: HashMap<String, Addr<CommandActor>> = HashMap::new();

        for (op_name, nexts) in dag.into_iter() {
            let op = conf.ops.get(&op_name).unwrap();

            let actor = command_actor::CommandActor::new(
                op_name.clone(),
                op.clone(),
                console.clone(),
                watcher.clone(),
                nexts
                    .iter()
                    .map(|e| commands.get(e).expect("who").clone())
                    .collect(),
                working_dir.clone(),
            )
            .start();
            commands.insert(op_name, actor);
        }
    };

    Arbiter::current().spawn(exec);

    system.run()?;
    Ok(())
}

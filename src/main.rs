use actix::prelude::*;
use actors::command::CommandActor;
use actors::console::ConsoleActor;
use actors::watcher::WatcherActor;
use config::Config;

use std::collections::HashMap;

use std::env;
mod actors;
mod config;
use anyhow::Result;

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

    let config = match Config::from_file(&args.file) {
        Ok(conf) => conf,
        Err(err) => {
            println!("file error: {}", err);
            process::exit(1);
        }
    };

    let dag = match config.build_dag() {
        Ok(conf) => conf,
        Err(err) => {
            println!("config error: {}", err);
            process::exit(2);
        }
    };

    let base_dir = env::current_dir()?
        .join(args.file)
        .parent()
        .unwrap()
        .to_path_buf();

    let system = System::new();
    let exec = async move {
        let console =
            ConsoleActor::new(Vec::from_iter(config.ops.keys().map(|e| e.clone()))).start();
        let watcher = WatcherActor::new().start();

        let mut commands: HashMap<String, Addr<CommandActor>> = HashMap::new();

        for (op_name, nexts) in dag.into_iter() {
            let op = config.ops.get(&op_name).unwrap();

            let actor = CommandActor::new(
                op_name.clone(),
                op.clone(),
                console.clone(),
                watcher.clone(),
                nexts
                    .iter()
                    .map(|e| commands.get(e).unwrap().clone())
                    .collect(),
                base_dir.clone(),
            )
            .start();
            commands.insert(op_name, actor);
        }
    };

    Arbiter::current().spawn(exec);

    system.run()?;
    Ok(())
}

use actix::prelude::*;
use globset::{Glob, GlobSetBuilder};
use watcher_actor::WatchGlob;

use std::io;
mod command_actor;
mod config;
mod console_actor;
mod watcher_actor;

fn main() -> io::Result<()> {
    let system = System::new();

    let exec = async {
        let conf = config::Config::from_file("test.hocon").unwrap();
        println!("parsed {:?}", conf);

        let console = console_actor::ConsoleActor::new(Vec::from_iter(conf.ops.keys())).start();
        let watcher = watcher_actor::WatcherActor::new(console.clone()).start();

        for (op_name, op) in conf.ops.into_iter() {
            let command =
                command_actor::CommandActor::new(op_name.clone(), op.clone(), console.clone())
                    .start();

            let mut on = GlobSetBuilder::new();
            for pattern in op.watches.resolve() {
                on.add(Glob::new(&pattern).unwrap());
            }

            let mut off = GlobSetBuilder::new();
            for pattern in op.ignores.resolve() {
                off.add(Glob::new(&pattern).unwrap());
            }

            let glob = WatchGlob {
                op: op_name,
                command,
                on: on.build().unwrap(),
                off: off.build().unwrap(),
            };

            watcher.do_send(glob);
        }
    };

    Arbiter::current().spawn(exec);

    system.run()
}

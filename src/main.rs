use actix::prelude::*;

use std::io;
mod command_actor;
mod config;
mod console_actor;
mod watcher_actor;

fn main() -> io::Result<()> {
    let system = System::new();

    Arbiter::new().spawn(async move {
        let conf = config::Config::from_file("test.hocon").unwrap();

        let console = console_actor::ConsoleActor::new().start();
        let watcher = watcher_actor::WatcherActor::new(console.clone()).start();

        for op in conf.operators.into_iter() {
            command_actor::CommandActor::new(console.clone()).start();
        }
    });

    system.run()?;

    Ok(())
}

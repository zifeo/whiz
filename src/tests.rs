use std::{env, future::Future};

use anyhow::{Ok, Result};

use crate::actors::command::{Status, WaitStatus};
use crate::actors::console::Register;
use crate::actors::watcher::WatchGlob;
use crate::{
    actors::{
        command::CommandActor,
        console::{ConsoleActor, Output, TermEvent},
        watcher::WatcherActor,
    },
    config::Config,
};
use actix::{actors::mocker::Mocker, prelude::*};

fn within_system<F: Future<Output = Result<()>>>(f: F) {
    let system = System::new();
    system.block_on(f).unwrap();
}

#[macro_export]
macro_rules! mock_actor {
    ( $tt:tt, { $( $mtch:ident : $ty:ty => $case:expr ), *, } ) => (
        Mocker::<$tt>::mock(Box::new(|msg, _ctx| {
            $(
                if msg.is::<$ty>() {
                    let $mtch = msg.downcast::<$ty>().unwrap();
                    Box::new($case)
                } else
            )*
            {
                println!("unexpect {:?}", msg.downcast::<Result<Status, std::io::Error>>());
                Box::new(None::<()>)
            }
        })).start()
    )
}

#[test]
fn hello() {
    within_system(async move {
        let config: Config = r#"
            test:
                shell: ls
            "#
        .parse()?;

        let console = mock_actor!(ConsoleActor, {
            msg: Output => {
                println!("---{:?}", msg.message);
                Some(())
            },
            _msg: Register => Some(()),
            _msg: TermEvent => Some(()),
        });

        let watcher = mock_actor!(WatcherActor, {
            _msg: WatchGlob => Some(()),
        });

        console
            .send(Output::now("test".to_string(), "message".to_string()))
            .await?;

        let commands =
            CommandActor::from_config(&config, console, watcher, env::current_dir().unwrap());

        let status = commands[0].send(WaitStatus).await?;
        println!("status: {:?}", status);

        Ok(())
    });
}

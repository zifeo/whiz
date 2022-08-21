use std::{env, future::Future};

use anyhow::{Ok, Result};

use actix::{actors::mocker::Mocker, prelude::*};

use crate::{
    actors::{
        command::CommandActor,
        console::{ConsoleActor, Output, TermEvent},
        watcher::WatcherActor,
    },
    config::Config,
};

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
                Box::new(Option::<()>::None)
            }
        })).start()
    )
}

#[test]
fn hello() -> Result<()> {
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
            _msg: TermEvent => Some(()),
        });

        let watcher = mock_actor!(WatcherActor, {
            _msg: TermEvent => Some(()),
        });

        console
            .send(Output::now("test".to_string(), "message".to_string()))
            .await?;

        let _commands =
            CommandActor::from_config(&config, console, watcher, env::current_dir().unwrap());

        Ok(())
    });

    Ok(())
}

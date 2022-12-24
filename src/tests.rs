use std::path::Path;
use std::{env, future::Future};

use anyhow::{Ok, Result};
use subprocess::ExitStatus;

use crate::actors::command::WaitStatus;
use crate::actors::console::Register;
use crate::actors::watcher::WatchGlob;
use crate::{
    actors::{
        command::CommandActor,
        console::{ConsoleActor, Output, TermEvent},
        watcher::WatcherActor,
    },
    config::Config,
    utils::recurse_default_config
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
                println!("unexpect {:?}", msg.downcast::<Result<ExitStatus, std::io::Error>>());
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
            .send(Output::now(
                "test".to_string(),
                "message".to_string(),
                false,
            ))
            .await?;

        let commands = CommandActor::from_config(
            &config,
            console,
            watcher,
            env::current_dir().unwrap(),
            false,
        );

        let status = commands[0].send(WaitStatus).await?;
        println!("status: {:?}", status);

        Ok(())
    });
}

#[test]
fn config_search_recursive() {
    assert!(env::current_dir().is_ok());
    let previous_cwd = 
        env::current_dir()
            .unwrap()
            .as_path()
            .display()
            .to_string();
    
    // change current working directory
    assert!(env::set_current_dir(Path::new("src")).is_ok());
    assert!(env::current_dir().is_ok());

    // cwd as string
    let new_cwd = 
        env::current_dir()
            .unwrap()
            .as_path()
            .display()
            .to_string();
    println!(" Working directory set to {}", new_cwd);

    // reset cwd to be safe
    assert!(env::set_current_dir(Path::new(&previous_cwd)).is_ok());
    println!(" Working directory reset to {}", previous_cwd);

    let config_name = "whiz.yaml";
    let expected_if_exist = 
        Path::new(&new_cwd)
            .join(&config_name)
            .display()
            .to_string();
    let config_got = recurse_default_config(config_name);

    println!(" Path \"{}\" should be different from \"{}\"", config_got, expected_if_exist);
    assert_ne!(config_got, expected_if_exist);
}

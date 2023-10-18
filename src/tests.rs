use std::collections::HashMap;
use std::path::Path;
use std::{env, future::Future};

use anyhow::{Ok, Result};
use subprocess::ExitStatus;

use crate::actors::command::WaitStatus;
use crate::actors::console::RegisterPanel;
use crate::actors::watcher::WatchGlob;
use crate::args::Args;
use crate::{
    actors::{
        command::CommandActor,
        console::{ConsoleActor, Output, TermEvent},
        watcher::WatcherActor,
    },
    config::Config,
    utils::recurse_config_file,
};
use actix::{actors::mocker::Mocker, prelude::*};
use assert_cmd::Command;
use clap::CommandFactory;

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
fn verify_cli() {
    Args::command().debug_assert()
}

#[test]
fn end_to_end() {
    let mut cmd = Command::cargo_bin("whiz").unwrap();
    cmd.arg("-h").assert().success();
}

#[test]
fn hello() {
    within_system(async move {
        let config: Config = r#"
            test:
                command: ls
            "#
        .parse()?;

        let console = mock_actor!(ConsoleActor, {
            msg: Output => {
                println!("---{:?}", msg.message);
                Some(())
            },
            _msg: RegisterPanel => Some(()),
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
            HashMap::new(),
            HashMap::new(),
        )
        .await?;

        let status = commands[0].send(WaitStatus).await?;
        println!("status: {:?}", status);

        Ok(())
    });
}

#[test]
fn config_search_recursive() {
    assert!(env::current_dir().is_ok());
    let previous_cwd = env::current_dir().unwrap().as_path().display().to_string();

    // change current working directory to {root_app}/src
    assert!(env::set_current_dir(Path::new("src")).is_ok());
    assert!(env::current_dir().is_ok());

    // cwd as string
    let new_cwd = env::current_dir().unwrap().as_path().display().to_string();
    println!(" Working directory set to {}", new_cwd);

    let config_name = "whiz.yaml";
    let expected_if_exist = Path::new(&new_cwd).join(config_name).display().to_string();

    let config_got = recurse_config_file(config_name);
    assert!(config_got.is_ok());

    let (_, config_path) = config_got.unwrap();
    let config_path_got = config_path.display().to_string();

    println!(" Config file located at {}", config_path_got);
    println!(
        " Path \"{}\" should be different from \"{}\"",
        config_path_got, expected_if_exist
    );
    assert_ne!(config_path_got, expected_if_exist);

    // reset cwd to be safe
    assert!(env::set_current_dir(Path::new(&previous_cwd)).is_ok());
    println!(" Working directory reset to {}", previous_cwd);
}

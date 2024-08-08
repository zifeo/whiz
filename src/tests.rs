use std::path::Path;
use std::sync::Arc;
use std::{env, future::Future};

use anyhow::{Ok, Result};

use subprocess::ExitStatus;

use crate::actors::command::{CommandActorsBuilder, WaitStatus};
use crate::actors::console::{OutputKind, RegisterPanel};
use crate::actors::watcher::WatchGlob;
use crate::args::Args;
use crate::config::{ConfigInner, RawConfig};
use crate::utils::find_config_path;
use crate::{
    actors::{
        console::{ConsoleActor, Output, PanelStatus, TermEvent},
        grim_reaper::GrimReaperActor,
        watcher::WatcherActor,
    },
    config::Config,
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
                println!("unexpected {:?} on {}",
                    msg.downcast::<Result<ExitStatus, std::io::Error>>(),
                    stringify!($tt)
                );
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

fn config_from_str(s: &str) -> Result<Config> {
    let raw: RawConfig = s.parse()?;
    Ok(Arc::new(ConfigInner::from_raw(raw, env::current_dir()?)?))
}

#[test]
fn hello() {
    within_system(async move {
        let config = config_from_str(
            r#"
            test:
                command: ls
            "#,
        )?;

        let console = mock_actor!(ConsoleActor, {
            msg: Output => {
                println!("---{:?}", msg.message);
                Some(())
            },
            _msg: RegisterPanel => Some(()),
            _msg: TermEvent => Some(()),
            _msg: PanelStatus => Some(()),
        });

        let watcher = mock_actor!(WatcherActor, {
            _msg: WatchGlob => Some(()),
        });

        console
            .send(Output::now(
                "test".to_string(),
                "message".to_string(),
                OutputKind::Command,
            ))
            .await?;

        let commands = CommandActorsBuilder::new(config, console, watcher)
            .build()
            .await?;

        let status = commands.get("test").unwrap().send(WaitStatus).await?;
        println!("status: {:?}", status);

        Ok(())
    });
}

#[test]
fn test_grim_reaper() {
    let system = System::with_tokio_rt(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap()
    });

    let fut = async move {
        let config_raw = r#"
test:
    entrypoint: 'python3 -c'
    command: 'print("hello whiz")'
long_test_dep:
    entrypoint: 'python3 -c'
    command: 'import time; time.sleep(1); print("wake up")'
long_test:
    entrypoint: 'python3 -c'
    command: 'print("my que to enter")'
    depends_on:
        - long_test_dep"#;
        let config: Config = config_from_str(config_raw)?;

        let console = mock_actor!(ConsoleActor, {
            msg: Output => {
                println!("---{:?}", msg.message);
                Some(())
            },
            _msg: PanelStatus => Some(()),
            _msg: RegisterPanel => Some(()),
            _msg: TermEvent => Some(()),
        });

        let watcher = mock_actor!(WatcherActor, {
            _msg: WatchGlob => Some(()),
        });

        let commands = CommandActorsBuilder::new(config, console, watcher)
            .build()
            .await?;

        GrimReaperActor::start_new(commands).await?;
        Ok(())
    };

    Arbiter::current().spawn(async { fut.await.unwrap() });

    let timer = std::time::SystemTime::now();
    assert_eq!(0, system.run_with_code().unwrap());
    let elapsed = timer.elapsed().unwrap();
    assert!(
        elapsed.as_millis() >= 1000,
        "test took less than a second: {elapsed:?}"
    );
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

    let config_path = find_config_path(&env::current_dir().unwrap(), config_name).unwrap();
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

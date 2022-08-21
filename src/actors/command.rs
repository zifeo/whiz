use actix::clock::sleep;
use actix::prelude::*;

use anyhow::Result;
use chrono::{DateTime, Local};
use subprocess::{Exec, ExitStatus, Popen, Redirection};

use globset::{Glob, GlobSetBuilder};
use path_clean::{self, PathClean};
use std::{collections::HashMap, env, time::Duration};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
};

use crate::config::Config;
use crate::config::Operator;

use super::console::{Output, Register};
use super::watcher::WatchGlob;

#[cfg(not(test))]
mod prelude {
    use crate::actors::{console::ConsoleActor, watcher::WatcherActor};

    pub type WatcherAct = WatcherActor;
    pub type ConsoleAct = ConsoleActor;
}

#[cfg(test)]
mod prelude {
    use crate::actors::{console::ConsoleActor, watcher::WatcherActor};
    use actix::actors::mocker::Mocker;

    pub type WatcherAct = Mocker<WatcherActor>;
    pub type ConsoleAct = Mocker<ConsoleActor>;
}

use prelude::*;

pub struct CommandActor {
    op_name: String,
    operator: Operator,
    console: Addr<ConsoleAct>,
    watcher: Addr<WatcherAct>,
    arbiter: Arbiter,
    child: Option<Popen>,
    nexts: Vec<Addr<CommandActor>>,
    last_run: DateTime<Local>,
    base_dir: PathBuf,
    self_addr: Option<Addr<CommandActor>>,
}

impl CommandActor {
    pub fn from_config(
        config: &Config,
        console: Addr<ConsoleAct>,
        watcher: Addr<WatcherAct>,
        base_dir: PathBuf,
    ) -> Vec<Addr<CommandActor>> {
        let mut commands: HashMap<String, Addr<CommandActor>> = HashMap::new();

        for (op_name, nexts) in config.build_dag().unwrap().into_iter() {
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

        commands
            .values()
            .into_iter()
            .map(|i| i.to_owned())
            .collect::<Vec<_>>()
    }

    pub fn new(
        op_name: String,
        operator: Operator,
        console: Addr<ConsoleAct>,
        watcher: Addr<WatcherAct>,
        nexts: Vec<Addr<CommandActor>>,
        base_dir: PathBuf,
    ) -> Self {
        Self {
            op_name,
            operator,
            console,
            watcher,
            arbiter: Arbiter::new(),
            child: None,
            nexts,
            last_run: Local::now(),
            base_dir,
            self_addr: None,
        }
    }

    fn kill(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            if child.poll().is_none() {
                child.terminate()?;
                child.wait_timeout(Duration::from_millis(100))?;

                if child.poll().is_none() {
                    self.console
                        .do_send(Output::now(self.op_name.clone(), "terminating".to_string()));
                    child.kill()?;
                    child.wait()?;
                }
            }
            self.child = None;
        }
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        self.kill()?;

        let args = &self.operator.shell;
        let mut envs: HashMap<String, String> = HashMap::new();
        envs.extend(env::vars());
        envs.extend(self.operator.resolve_envs()?);

        let mut p = Exec::cmd("bash")
            .cwd(
                self.operator
                    .workdir
                    .clone()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| env::current_dir().unwrap()),
            )
            .args(&["-c", args])
            .env_extend(&envs.into_iter().collect::<Vec<(String, String)>>())
            .stdout(Redirection::Pipe)
            .stderr(Redirection::Merge)
            .popen()
            .unwrap();

        let stdout = p.stdout.take().unwrap();
        let reader = BufReader::new(stdout);

        let console = self.console.clone();
        let op_name = self.op_name.clone();
        let self_addr = self.self_addr.clone();
        let fut = async move {
            for line in reader.lines() {
                console.do_send(Output::now(op_name.clone(), line.unwrap()));
            }
            if let Some(addr) = self_addr {
                addr.do_send(StdoutTerminated);
            }
        };

        self.child = Some(p);

        self.last_run = Local::now();
        self.arbiter.spawn(fut);

        Ok(())
    }

    fn exit_code(&mut self) -> Option<i32> {
        match &mut self.child {
            None => None,
            Some(p) => p.poll().map(|c| match c {
                ExitStatus::Exited(c) => Some(c as i32),
                ExitStatus::Other(c) => Some(c),
                ExitStatus::Signaled(c) => Some(c as i32),
                ExitStatus::Undetermined => None,
            }),
        }
        .flatten()
    }
}

impl Actor for CommandActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let addr = ctx.address();
        self.self_addr = Some(addr.clone());

        self.console.do_send(Register {
            title: self.op_name.clone(),
            addr,
        });

        let dir = self
            .base_dir
            .join(self.operator.workdir.as_ref().unwrap_or(&"".to_string()))
            .clean();

        let watches = self.operator.watches.resolve();

        if !watches.is_empty() {
            let mut on = GlobSetBuilder::new();
            for pattern in self.operator.watches.resolve() {
                on.add(Glob::new(&dir.join(&pattern).to_string_lossy()).unwrap());
            }

            let mut off = GlobSetBuilder::new();
            for pattern in self.operator.ignores.resolve() {
                off.add(Glob::new(&dir.join(&pattern).to_string_lossy()).unwrap());
            }

            let glob = WatchGlob {
                command: ctx.address(),
                on: on.build().unwrap(),
                off: off.build().unwrap(),
            };

            self.watcher.do_send(glob);
        }

        self.reload().unwrap();
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        self.self_addr = None;
        self.kill().unwrap();
    }
}

#[derive(Clone)]
pub struct Reload {
    trigger: String,
    at: DateTime<Local>,
}

impl Reload {
    pub fn now(trigger: String) -> Self {
        Self {
            trigger,
            at: Local::now(),
        }
    }
    fn with_trigger(&self, trigger: String) -> Self {
        Self {
            trigger,
            at: self.at,
        }
    }
}

impl Message for Reload {
    type Result = ();
}

impl Handler<Reload> for CommandActor {
    type Result = ();

    fn handle(&mut self, msg: Reload, _: &mut Context<Self>) -> Self::Result {
        self.console
            .do_send(Output::now(self.op_name.clone(), msg.trigger.clone()));

        self.reload().unwrap();
        for next in (&self.nexts).iter() {
            next.do_send(msg.with_trigger(format!("{} via {}", self.op_name, msg.trigger)));
        }
    }
}

#[derive(Message)]
#[rtype(result = "Result<Status, std::io::Error>")]
pub struct GetStatus;

#[derive(Debug)]
pub struct Status {
    pub exit_code: Option<i32>,
}

impl Handler<GetStatus> for CommandActor {
    type Result = Result<Status, std::io::Error>;

    fn handle(&mut self, _: GetStatus, _: &mut Self::Context) -> Self::Result {
        Ok(Status {
            exit_code: self.exit_code(),
        })
    }
}

#[derive(Message)]
#[rtype(result = "Result<Status, std::io::Error>")]
pub struct WaitStatus;

impl Handler<WaitStatus> for CommandActor {
    type Result = ResponseActFuture<Self, Result<Status, std::io::Error>>;

    fn handle(&mut self, _: WaitStatus, ctx: &mut Self::Context) -> Self::Result {
        let addr = ctx.address();
        let f = async move {
            loop {
                let status = addr.send(GetStatus).await.unwrap().unwrap();
                if status.exit_code.is_some() {
                    return status;
                }
                sleep(Duration::from_millis(20)).await;
            }
        }
        .into_actor(self)
        .map(|res, _act, _ctx| Ok(res));
        Box::pin(f)
    }
}
#[derive(Message)]
#[rtype(result = "()")]
struct StdoutTerminated;

impl Handler<StdoutTerminated> for CommandActor {
    type Result = ();

    fn handle(&mut self, _: StdoutTerminated, _: &mut Self::Context) -> Self::Result {
        let c = self
            .exit_code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".to_string());
        self.console
            .do_send(Output::now(self.op_name.clone(), format!("exited ({})", c)));
    }
}

pub struct PoisonPill;

impl Message for PoisonPill {
    type Result = ();
}

impl Handler<PoisonPill> for CommandActor {
    type Result = ();

    fn handle(&mut self, _: PoisonPill, ctx: &mut Context<Self>) -> Self::Result {
        ctx.stop();
    }
}

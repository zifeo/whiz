use actix::prelude::*;
use chrono::{DateTime, Local};
use subprocess::{Exec, Popen, Redirection};

use crate::{
    config::Operator,
    console_actor::{ConsoleActor, Output},
    watcher_actor::{WatchGlob, WatcherActor},
};
use globset::{Glob, GlobSetBuilder};
use std::{collections::HashMap, env};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
};

pub struct CommandActor {
    op_name: String,
    operator: Operator,
    console: Addr<ConsoleActor>,
    watcher: Addr<WatcherActor>,
    arbiter: Arbiter,
    child: Option<Popen>,
    nexts: Vec<Addr<CommandActor>>,
    last_run: DateTime<Local>,
}

impl CommandActor {
    pub fn new(
        op_name: String,
        operator: Operator,
        console: Addr<ConsoleActor>,
        watcher: Addr<WatcherActor>,
        nexts: Vec<Addr<CommandActor>>,
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
        }
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().unwrap();
            child.wait().unwrap();
            self.child = None;
        }
    }

    fn reload(&mut self) {
        self.kill();

        let args = &self.operator.shell;
        let mut envs: HashMap<String, String> = HashMap::new();
        envs.extend(env::vars());
        if let Some(vars) = &self.operator.envs {
            envs.extend(vars.clone());
        }

        let mut p = Exec::cmd("bash")
            .cwd(
                self.operator
                    .workdir
                    .clone()
                    .map(|p| PathBuf::from(p))
                    .unwrap_or(env::current_dir().unwrap()),
            )
            .args(&vec!["-c", args])
            .env_extend(&envs.into_iter().collect::<Vec<(String, String)>>())
            .stdout(Redirection::Pipe)
            .stderr(Redirection::Merge)
            .popen()
            .unwrap();

        let stdout = p.stdout.take().unwrap();
        let reader = BufReader::new(stdout);

        let console = self.console.clone();
        let op_name = self.op_name.clone();
        let fut = async move {
            for line in reader.lines() {
                console.do_send(Output::now(op_name.clone(), line.unwrap()));
            }
            console.do_send(Output::now(op_name, "out".to_string()));
        };

        self.child = Some(p);

        self.last_run = Local::now();
        self.arbiter.spawn(fut);
    }
}

impl Actor for CommandActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let mut on = GlobSetBuilder::new();
        for pattern in self.operator.watches.resolve() {
            on.add(Glob::new(&pattern).unwrap());
        }

        let mut off = GlobSetBuilder::new();
        for pattern in self.operator.ignores.resolve() {
            off.add(Glob::new(&pattern).unwrap());
        }

        let glob = WatchGlob {
            command: ctx.address(),
            on: on.build().unwrap(),
            off: off.build().unwrap(),
        };

        self.watcher.do_send(glob);

        self.reload();
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        self.kill();
        self.arbiter.stop();
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

        self.reload();
        for next in (&self.nexts).into_iter() {
            next.do_send(msg.with_trigger(format!("{} via {}", msg.trigger, self.op_name)));
        }
    }
}

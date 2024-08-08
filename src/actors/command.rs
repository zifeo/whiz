use actix::clock::sleep;
use actix::prelude::*;

use anyhow::Result;
use chrono::{DateTime, Local};
use subprocess::{ExitStatus, Popen, Redirection};

use globset::{Glob, GlobSetBuilder};
use path_absolutize::*;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::{collections::HashMap, time::Duration};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
};

use crate::actors::grim_reaper::PermaDeathInvite;
use crate::config::color::ColorOption;
use crate::config::{
    pipe::{OutputRedirection, Pipe},
    Config, Task,
};
use crate::exec::ExecBuilder;

use super::console::{Output, OutputKind, PanelStatus, RegisterPanel};
use super::watcher::{IgnorePath, WatchGlob};

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

pub struct ExtendedTask {
    name: String,
    task: Task,
    pipes: Vec<Pipe>,
    colors: Vec<ColorOption>,
    cwd: PathBuf,
}

impl Task {
    pub fn extend(&self, name: String, config: &Config) -> ExtendedTask {
        let cwd = self.get_absolute_workdir(&config.base_dir);
        let pipes = config.pipes_map.get(&name).unwrap_or(&Vec::new()).clone();
        let colors = config.colors_map.get(&name).unwrap_or(&Vec::new()).clone();

        ExtendedTask {
            name,
            task: self.clone(),
            pipes,
            colors,
            cwd,
        }
    }
}

#[derive(Debug)]
pub enum Child {
    NotStarted,
    Killed,
    Process(Popen),
    Exited(ExitStatus),
}

impl Child {
    fn poll(&mut self, kill: bool) -> Result<bool> {
        if let Child::Process(p) = self {
            match p.poll() {
                Some(exit) => {
                    *self = Self::Exited(exit);
                    Ok(true)
                }
                None if kill => {
                    p.terminate()?;
                    match p.wait_timeout(Duration::from_millis(500))? {
                        Some(_status) => {
                            //println!("terminated with {:?}", status);
                        }
                        None => {
                            p.kill()?;
                            let _status = p.wait()?;
                            //println!("killed with {:?} ", _status);
                        }
                    }

                    *self = Self::Killed;
                    Ok(true)
                }
                None => Ok(false),
            }
        } else {
            Ok(false)
        }
    }

    fn wait_or_kill(&mut self, dur: Duration) -> Result<bool> {
        if let Child::Process(p) = self {
            match p.wait_timeout(dur)? {
                Some(status) => {
                    *self = Child::Exited(status);
                    Ok(true)
                }
                None => {
                    p.terminate()?;
                    p.kill()?;
                    let _status = p.wait()?;
                    if p.wait_timeout(Duration::from_millis(500))?.is_none() {
                        p.kill()?;
                        p.wait()?;
                    }
                    *self = Self::Killed;
                    Ok(true)
                }
            }
        } else {
            Ok(false)
        }
    }

    fn exit_status(&mut self) -> Option<ExitStatus> {
        match &self {
            Child::Process(_) => None,
            Child::Killed => Some(ExitStatus::Undetermined),
            Child::Exited(exit) => Some(*exit),
            Child::NotStarted => panic!("should not happen"),
        }
    }
}

pub struct CommandActorsBuilder {
    config: Config,
    console: Addr<ConsoleAct>,
    watcher: Addr<WatcherAct>,
    verbose: bool,
    watch_enabled_globally: bool,
}

impl CommandActorsBuilder {
    pub fn new(config: Config, console: Addr<ConsoleAct>, watcher: Addr<WatcherAct>) -> Self {
        Self {
            config,
            console,
            watcher,
            verbose: false,
            watch_enabled_globally: true,
        }
    }

    pub fn verbose(self, toggle: bool) -> Self {
        Self {
            verbose: toggle,
            ..self
        }
    }

    pub fn globally_enable_watch(self, toggle: bool) -> Self {
        Self {
            watch_enabled_globally: toggle,
            ..self
        }
    }

    pub async fn build(self) -> Result<HashMap<String, Addr<CommandActor>>> {
        let Self {
            config,
            console,
            watcher,
            verbose,
            watch_enabled_globally,
        } = self;

        let mut commands: HashMap<String, Addr<CommandActor>> = HashMap::new();

        for (op_name, nexts) in config.build_dag().unwrap().into_iter() {
            let task = config.ops.get(&op_name).unwrap();

            let exec_builder = ExecBuilder::new(task, &config).await?;
            let op = task.extend(op_name.clone(), &config);

            let actor = CommandActor::new(
                op,
                console.clone(),
                watcher.clone(),
                nexts
                    .iter()
                    .map(|e| commands.get(e).unwrap().clone())
                    .collect(),
                verbose,
                watch_enabled_globally,
                exec_builder,
            )
            .start();

            if task.depends_on.resolve().is_empty() {
                actor.do_send(Reload::Start)
            }
            commands.insert(op_name, actor);
        }

        Ok(commands)
    }
}

pub struct CommandActor {
    operator: ExtendedTask,
    console: Addr<ConsoleAct>,
    watcher: Addr<WatcherAct>,
    arbiter: Arbiter,
    child: Child,
    nexts: Vec<Addr<CommandActor>>,
    self_addr: Option<Addr<CommandActor>>,
    pending_upstream: BTreeMap<String, usize>,
    verbose: bool,
    started_at: DateTime<Local>,
    watch: bool,
    death_invite: Option<PermaDeathInvite>,
    exec_builder: ExecBuilder,
}

impl CommandActor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        operator: ExtendedTask,
        console: Addr<ConsoleAct>,
        watcher: Addr<WatcherAct>,
        nexts: Vec<Addr<CommandActor>>,
        verbose: bool,
        watch: bool,
        exec_builder: ExecBuilder,
    ) -> Self {
        Self {
            operator,
            console,
            watcher,
            arbiter: Arbiter::new(),
            child: Child::NotStarted,
            nexts,
            self_addr: None,
            pending_upstream: BTreeMap::default(),
            verbose,
            started_at: Local::now(),
            watch,
            death_invite: None,
            exec_builder,
        }
    }

    fn log_info(&self, log: String) {
        let job_name = self.operator.name.clone();

        self.console
            .do_send(Output::now(job_name, log, OutputKind::Service));
    }

    fn log_debug(&self, log: String) {
        if self.verbose {
            self.log_info(log);
        }
    }

    fn ensure_stopped(&mut self) {
        if self.child.poll(true).unwrap() {
            self.send_reload();
        }
    }

    fn upstream(&self) -> String {
        Vec::from_iter(
            self.pending_upstream
                .iter()
                .map(|(k, v)| format!("{}×{}", v, k)),
        )
        .join(", ")
    }

    fn send_reload(&self) {
        for next in (self.nexts).iter() {
            next.do_send(Reload::Op(self.operator.name.clone()));
        }
    }

    fn send_will_reload(&self) {
        for next in (self.nexts).iter() {
            next.do_send(WillReload {
                op_name: self.operator.name.clone(),
            });
        }
    }

    fn reload(&mut self) -> Result<()> {
        self.log_debug(self.exec_builder.as_string());
        self.console.do_send(PanelStatus {
            panel_name: self.operator.name.clone(),
            status: None,
        });

        let mut p = self
            .exec_builder
            .build()
            .unwrap()
            .stdout(Redirection::Pipe)
            .stderr(Redirection::Merge)
            .popen()
            .unwrap();

        let stdout = p.stdout.take().unwrap();
        let reader = BufReader::new(stdout);

        let console = self.console.clone();
        let op_name = self.operator.name.clone();
        let self_addr = self.self_addr.clone();
        let started_at = Local::now();
        let cwd = self.operator.cwd.clone();
        let watcher = self.watcher.clone();
        let task_pipes = self.operator.pipes.clone();
        let task_colors = self.operator.colors.clone();

        let fut = async move {
            for line in reader.lines() {
                let mut line = line.unwrap();

                let task_pipe = task_pipes.iter().find(|pipe| pipe.regex.is_match(&line));

                if let Some(task_pipe) = task_pipe {
                    match &task_pipe.redirection {
                        OutputRedirection::Tab(name) => {
                            let mut tab_name = "".to_string();
                            if let Some(capture) = task_pipe.regex.captures(&line) {
                                capture.expand(&name.clone(), &mut tab_name);
                            }
                            if let Some(addr) = &self_addr {
                                // tabs must be created on each loop,
                                // as their name can be dynamic
                                console.do_send(RegisterPanel {
                                    name: tab_name.to_owned(),
                                    addr: addr.clone(),
                                    colors: task_colors.clone(),
                                });
                            }
                            console.do_send(Output::now(
                                tab_name.to_owned(),
                                line,
                                OutputKind::Command,
                            ));
                        }
                        OutputRedirection::File(path) => {
                            let path = task_pipe.regex.replace(&line, path);
                            let mut path = Path::new(path.as_ref()).to_path_buf();

                            // prepend base dir if the log file path is relative
                            if !path.starts_with("/") {
                                path = cwd.join(path);
                            }

                            let log_folder = Path::new(&path).parent().unwrap();
                            fs::create_dir_all(log_folder).unwrap();

                            // file must be created and opened on each loop
                            // as the path is dynamic, therefore there
                            // is no a way to optimize it to create it
                            // only once
                            let mut file = fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&path)
                                .unwrap();

                            // exlude file path from watcher before writing to it
                            // to avoid infinite loops
                            watcher.do_send(IgnorePath(path));

                            // append new line since strings from the buffer reader don't include it
                            line.push('\n');
                            file.write_all(line.as_bytes()).unwrap();
                        }
                    }
                } else {
                    console.do_send(Output::now(op_name.clone(), line, OutputKind::Command));
                }
            }

            if let Some(addr) = self_addr {
                addr.do_send(StdoutTerminated { started_at });
            }
        };

        self.child = Child::Process(p);
        self.started_at = started_at;
        self.arbiter.spawn(fut);

        Ok(())
    }

    fn accept_death_invite(&mut self, cx: &mut Context<Self>) {
        if let Some(invite) = self.death_invite.take() {
            let status = match &self.child {
                Child::Killed => ExitStatus::Other(1),
                Child::Exited(val) => *val,
                child => panic!("invalid death invite acceptance: {child:?}"),
            };
            invite.rsvp::<Self, Context<Self>>(self.operator.name.clone(), status, cx);
        }
    }
}

impl Actor for CommandActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let addr = ctx.address();
        self.self_addr = Some(addr.clone());

        self.console.do_send(RegisterPanel {
            name: self.operator.name.clone(),
            addr,
            colors: self.operator.colors.clone(),
        });

        let watches = self.operator.task.watch.resolve();

        if self.watch && !watches.is_empty() {
            let mut on = GlobSetBuilder::new();
            for pattern in watches {
                on.add(
                    Glob::new(
                        &self
                            .operator
                            .cwd
                            .join(pattern)
                            .absolutize()
                            .unwrap()
                            .to_string_lossy(),
                    )
                    .unwrap(),
                );
            }

            let mut off = GlobSetBuilder::new();
            for pattern in self.operator.task.ignore.resolve() {
                off.add(
                    Glob::new(
                        &self
                            .operator
                            .cwd
                            .join(pattern)
                            .absolutize()
                            .unwrap()
                            .to_string_lossy(),
                    )
                    .unwrap(),
                );
            }

            let glob = WatchGlob {
                command: ctx.address(),
                on: on.build().unwrap(),
                off: off.build().unwrap(),
            };

            self.watcher.do_send(glob);
        }
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        self.self_addr = None;
        self.child.poll(true).unwrap();
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct WillReload {
    pub op_name: String,
}

impl Handler<WillReload> for CommandActor {
    type Result = ();

    fn handle(&mut self, msg: WillReload, _: &mut Context<Self>) -> Self::Result {
        let counter = self.pending_upstream.remove(&msg.op_name).unwrap_or(0);
        self.pending_upstream
            .insert(msg.op_name.clone(), counter + 1);

        self.log_info(format!("Waiting on {}", msg.op_name));
        self.log_debug(format!("WAIT: +{} [{}]", msg.op_name, self.upstream()));

        self.ensure_stopped();

        self.send_will_reload();
    }
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub enum Reload {
    Start,
    Manual,
    Watch(String),
    Op(String),
}

impl Handler<Reload> for CommandActor {
    type Result = ();

    fn handle(&mut self, msg: Reload, _: &mut Context<Self>) -> Self::Result {
        self.ensure_stopped();

        match &msg {
            Reload::Start => {
                self.send_will_reload();
            }
            Reload::Manual => {
                if !self.pending_upstream.is_empty() {
                    self.log_info(format!(
                        "RELOAD: manual while pending on {}",
                        self.upstream()
                    ));
                } else {
                    self.log_info("RELOAD: manual".to_string());
                }
                self.send_will_reload();
            }
            Reload::Watch(files) => {
                self.log_info(format!("RELOAD: file changed: {files} "));
                self.send_will_reload();
            }
            Reload::Op(op_name) => {
                let counter = self.pending_upstream.remove(op_name).unwrap();

                if counter > 1 {
                    self.pending_upstream.insert(op_name.clone(), counter - 1);
                }

                self.log_debug(format!("WAIT: -{} [{}]", op_name.clone(), self.upstream()));

                if !self.pending_upstream.is_empty() {
                    return;
                } else {
                    self.log_info("Upstream(s) finished".to_string());
                }
            }
        }

        self.reload().unwrap();
    }
}

#[derive(Message)]
#[rtype(result = "Result<Option<ExitStatus>, std::io::Error>")]
pub struct GetStatus;

impl Handler<GetStatus> for CommandActor {
    type Result = Result<Option<ExitStatus>, std::io::Error>;

    fn handle(&mut self, _: GetStatus, _: &mut Self::Context) -> Self::Result {
        self.child.poll(false).unwrap();
        Ok(self.child.exit_status())
    }
}

#[derive(Message)]
#[rtype(result = "Result<ExitStatus, std::io::Error>")]
pub struct WaitStatus;

impl Handler<WaitStatus> for CommandActor {
    type Result = ResponseActFuture<Self, Result<ExitStatus, std::io::Error>>;

    fn handle(&mut self, _: WaitStatus, ctx: &mut Self::Context) -> Self::Result {
        let addr = ctx.address();
        let f = async move {
            loop {
                if let Some(status) = addr.send(GetStatus).await.unwrap().unwrap() {
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
struct StdoutTerminated {
    pub started_at: DateTime<Local>,
}

impl Handler<StdoutTerminated> for CommandActor {
    type Result = ();

    fn handle(&mut self, msg: StdoutTerminated, cx: &mut Self::Context) -> Self::Result {
        if msg.started_at == self.started_at {
            // since there's a chance that child might not be done by this point
            // wait for it die for a maximum of 1 seconds
            // before pulling the plug
            if self
                .child
                .wait_or_kill(Duration::from_millis(1000))
                .unwrap()
            {
                self.send_reload();
            }
            let exit = self.child.exit_status();
            self.console.do_send(PanelStatus {
                panel_name: self.operator.name.clone(),
                status: exit,
            });
            self.accept_death_invite(cx);
        }
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PoisonPill;

impl Handler<PoisonPill> for CommandActor {
    type Result = ();

    fn handle(&mut self, _: PoisonPill, ctx: &mut Context<Self>) -> Self::Result {
        self.accept_death_invite(ctx);
        ctx.stop();
    }
}

impl Handler<PermaDeathInvite> for CommandActor {
    type Result = ();

    fn handle(&mut self, evt: PermaDeathInvite, cx: &mut Context<Self>) -> Self::Result {
        self.child.poll(false).unwrap();
        let status = match &self.child {
            Child::Killed => Some(ExitStatus::Other(1)),
            Child::Exited(val) => Some(*val),
            _ => None,
        };
        if let Some(status) = status {
            evt.rsvp::<Self, Self::Context>(self.operator.name.clone(), status, cx);
        } else {
            self.death_invite = Some(evt);
        }
    }
}

use actix::clock::sleep;
use actix::prelude::*;

use anyhow::{Context as ErrorContext, Result};
use chrono::{DateTime, Local};
use subprocess::{Exec, ExitStatus, Popen, Redirection};

use dotenv_parser::parse_dotenv;
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

use shlex;

use crate::config::color::ColorOption;
use crate::actors::grim_reaper::PermaDeathInvite;
use crate::config::{
    pipe::{OutputRedirection, Pipe},
    Config, Task,
};

use super::console::{Output, PanelStatus, RegisterPanel};
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
    base_dir: PathBuf,
    verbose: bool,
    colors_map: HashMap<String, Vec<ColorOption>>,
    pipes_map: HashMap<String, Vec<Pipe>>,
    watch_enabled_globally: bool,
}

impl CommandActorsBuilder {
    pub fn new(
        config: Config,
        console: Addr<ConsoleAct>,
        watcher: Addr<WatcherAct>,
        base_dir: PathBuf,
        colors_map: HashMap<String, Vec<ColorOption>>,
    ) -> Self {
        Self {
            config,
            console,
            watcher,
            base_dir,
            verbose: false,
            pipes_map: Default::default(),
            watch_enabled_globally: true,
            colors_map,
        }
    }

    pub fn pipes_map(self, pipes_map: HashMap<String, Vec<Pipe>>) -> Self {
        Self { pipes_map, ..self }
    }

    pub fn verbose(self, toggle: bool) -> Self {
        Self {
            verbose: toggle,
            ..self
        }
    }

    pub fn add_highlighting(config: &mut Config) {
        let ops = config.ops.clone();

        for (k, task) in ops.iter() {
            for command in task.command.iter() {
                let old_command = command.clone();
                let mut new_args = Vec::new();
                for arg in old_command.split('\n') {
                    if arg.trim().len() == 0 {
                        continue;
                    }
                    new_args.push(format!("{} | tspin", arg));
                }
                let new_command = new_args.join("\n");
                let new_task = Task {
                    command: Some(new_command),
                    ..task.clone()
                };
                config.ops.insert(k.clone(), new_task);
            }
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
            mut config,
            console,
            watcher,
            base_dir,
            verbose,
            pipes_map,
            watch_enabled_globally,
            colors_map,
        } = self;
        let mut shared_env = HashMap::from_iter(std::env::vars());
        shared_env.extend(lade_sdk::resolve(&config.env, &shared_env)?);
        let shared_env = lade_sdk::hydrate(shared_env, base_dir.clone()).await?;

        let mut commands: HashMap<String, Addr<CommandActor>> = HashMap::new();
        CommandActorsBuilder::add_highlighting(&mut config);

        for (op_name, nexts) in config.build_dag().unwrap().into_iter() {
            let op = config.ops.get(&op_name).unwrap();
            let task_pipes = pipes_map.get(&op_name).unwrap_or(&Vec::new()).clone();
            let colors = colors_map.get(&op_name).unwrap_or(&Vec::new()).clone();
            let cwd = match op.workdir.clone() {
                Some(path) => base_dir.join(path),
                None => base_dir.clone(),
            };

            let mut env = HashMap::default();
            for env_file in op.env_file.resolve() {
                let path = cwd.join(env_file.clone());
                let file = fs::read_to_string(path.clone())
                    .with_context(|| format!("cannot find env_file {:?}", path.clone()))?;
                let values = parse_dotenv(&file)
                    .map_err(anyhow::Error::msg)
                    .with_context(|| format!("cannot parse env_file {:?}", path))?
                    .into_iter()
                    .map(|(k, v)| (k, v.replace("\\n", "\n")));

                env.extend(lade_sdk::resolve(&values.collect(), &shared_env)?);
            }
            env.extend(lade_sdk::resolve(&op.env.clone(), &shared_env)?);
            let mut env = lade_sdk::hydrate(env, cwd.clone()).await?;
            env.extend(shared_env.clone());

            let actor = CommandActor::new(
                op_name.clone(),
                op.clone(),
                console.clone(),
                watcher.clone(),
                nexts
                    .iter()
                    .map(|e| commands.get(e).unwrap().clone())
                    .collect(),
                cwd.clone(),
                verbose,
                env.into_iter().collect(),
                task_pipes,
                colors,
                op.entrypoint.clone(),
                watch_enabled_globally,
            )
            .start();

            if op.depends_on.resolve().is_empty() {
                actor.do_send(Reload::Start)
            }
            commands.insert(op_name, actor);
        }

        Ok(commands)
    }
}

pub struct CommandActor {
    op_name: String,
    operator: Task,
    console: Addr<ConsoleAct>,
    watcher: Addr<WatcherAct>,
    arbiter: Arbiter,
    child: Child,
    nexts: Vec<Addr<CommandActor>>,
    cwd: PathBuf,
    self_addr: Option<Addr<CommandActor>>,
    pending_upstream: BTreeMap<String, usize>,
    verbose: bool,
    started_at: DateTime<Local>,
    env: Vec<(String, String)>,
    pipes: Vec<Pipe>,
    colors: Vec<ColorOption>,
    entrypoint: Option<String>,
    watch: bool,
    death_invite: Option<PermaDeathInvite>,
}

impl CommandActor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        op_name: String,
        operator: Task,
        console: Addr<ConsoleAct>,
        watcher: Addr<WatcherAct>,
        nexts: Vec<Addr<CommandActor>>,
        cwd: PathBuf,
        verbose: bool,
        env: Vec<(String, String)>,
        pipes: Vec<Pipe>,
        colors: Vec<ColorOption>,
        entrypoint: Option<String>,
        watch: bool,
    ) -> Self {
        Self {
            op_name,
            operator,
            console,
            watcher,
            arbiter: Arbiter::new(),
            child: Child::NotStarted,
            nexts,
            cwd,
            self_addr: None,
            pending_upstream: BTreeMap::default(),
            verbose,
            started_at: Local::now(),
            env,
            pipes,
            colors,
            entrypoint,
            watch,
            death_invite: None,
        }
    }

    fn log_info(&self, log: String) {
        let job_name = self.op_name.clone();

        self.console.do_send(Output::now(job_name, log, true));
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
            next.do_send(Reload::Op(self.op_name.clone()));
        }
    }

    fn send_will_reload(&self) {
        for next in (self.nexts).iter() {
            next.do_send(WillReload {
                op_name: self.op_name.clone(),
            });
        }
    }

    fn reload(&mut self) -> Result<()> {
        let args = &self.operator.command;

        let default_entrypoint = {
            #[cfg(not(target_os = "windows"))]
            {
                "bash -c"
            }

            #[cfg(target_os = "windows")]
            {
                "cmd /c"
            }
        };

        let exec = {
            let entrypoint_lex = match &self.entrypoint {
                Some(e) => {
                    if !e.is_empty() {
                        e.as_str()
                    } else {
                        default_entrypoint
                    }
                }
                None => default_entrypoint,
            };

            let entrypoint_split = {
                let mut s = shlex::split(entrypoint_lex).unwrap();

                match args {
                    Some(a) => {
                        s.push(a.to_owned());
                        s
                    }
                    None => s,
                }
            };

            let entrypoint = &entrypoint_split[0];
            let nargs = entrypoint_split[1..]
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect::<Vec<String>>();

            self.log_debug(format!(
                "EXEC: {} {:?} at {:?}",
                entrypoint_lex, nargs, self.cwd
            ));
            self.console.do_send(PanelStatus {
                panel_name: self.op_name.clone(),
                status: None,
            });

            Exec::cmd(entrypoint).args(&nargs)
        };

        let mut p = exec
            .cwd(&self.cwd)
            .env_extend(&self.env)
            .stdout(Redirection::Pipe)
            .stderr(Redirection::Merge)
            .popen()
            .unwrap();

        let stdout = p.stdout.take().unwrap();
        let reader = BufReader::new(stdout);

        let console = self.console.clone();
        let op_name = self.op_name.clone();
        let self_addr = self.self_addr.clone();
        let started_at = Local::now();
        let cwd = self.cwd.clone();
        let watcher = self.watcher.clone();
        let task_pipes = self.pipes.clone();
        let task_colors = self.colors.clone();

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
                                    colors: task_colors.clone()
                                });
                            }
                            console.do_send(Output::now(tab_name.to_owned(), line.clone(), false));
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
                            file.write_all(line.clone().as_bytes()).unwrap();
                        }
                    }
                } else {
                    console.do_send(Output::now(op_name.clone(), line.clone(), false));
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
            invite.rsvp::<Self, Context<Self>>(self.op_name.clone(), status, cx);
        }
    }
}

impl Actor for CommandActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let addr = ctx.address();
        self.self_addr = Some(addr.clone());

        self.console.do_send(RegisterPanel {
            name: self.op_name.clone(),
            addr,
            colors: self.colors.clone()
        });

        let watches = self.operator.watch.resolve();

        if self.watch && !watches.is_empty() {
            let mut on = GlobSetBuilder::new();
            for pattern in self.operator.watch.resolve() {
                on.add(
                    Glob::new(
                        &self
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
            for pattern in self.operator.ignore.resolve() {
                off.add(
                    Glob::new(
                        &self
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
                panel_name: self.op_name.clone(),
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
            evt.rsvp::<Self, Self::Context>(self.op_name.clone(), status, cx);
        } else {
            self.death_invite = Some(evt);
        }
    }
}

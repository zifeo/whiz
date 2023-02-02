use actix::clock::sleep;
use actix::prelude::*;

use anyhow::Result;
use chrono::{DateTime, Local};
use regex::Regex;
use subprocess::{Exec, ExitStatus, Popen, Redirection};

use dotenv_parser::parse_dotenv;
use globset::{Glob, GlobSetBuilder};
use path_absolutize::*;
use path_clean::{self, PathClean};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::{collections::HashMap, env, time::Duration};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
};

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
        if let Child::Process(_) = &self {
            if let Child::Process(mut p) = std::mem::replace(self, Child::NotStarted) {
                if kill && p.poll().is_none() {
                    p.terminate()?;
                    p.wait_timeout(Duration::from_millis(10))?;

                    if p.poll().is_none() {
                        p.kill()?;
                        p.wait()?;
                    }
                }

                match p.poll() {
                    Some(exit) => {
                        *self = Self::Exited(exit);
                        Ok(true)
                    }
                    None if kill => {
                        *self = Self::Killed;
                        Ok(true)
                    }
                    None => {
                        *self = Child::Process(p);
                        Ok(false)
                    }
                }
            } else {
                panic!("cannot swap");
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

pub struct CommandActor {
    op_name: String,
    operator: Task,
    console: Addr<ConsoleAct>,
    watcher: Addr<WatcherAct>,
    arbiter: Arbiter,
    child: Child,
    nexts: Vec<Addr<CommandActor>>,
    base_dir: PathBuf,
    self_addr: Option<Addr<CommandActor>>,
    pending_upstream: BTreeMap<String, usize>,
    verbose: bool,
    started_at: DateTime<Local>,
    shared_env: HashMap<String, String>,
    pipes: Vec<Pipe>,
}

pub fn resolve_env(
    kvs: &HashMap<String, String>,
    vars: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    let re = Regex::new(r"(\$\{?(\w+)\}?)")?;
    let res = kvs
        .iter()
        .map(|(key, value)| {
            let hydration = re.captures_iter(value).fold(value.clone(), |agg, c| {
                agg.replace(&c[1], vars.get(&c[2]).unwrap_or(&"".to_string()))
            });
            (key.clone(), hydration)
        })
        .collect();
    Ok(res)
}

impl CommandActor {
    pub fn from_config(
        config: &Config,
        console: Addr<ConsoleAct>,
        watcher: Addr<WatcherAct>,
        base_dir: PathBuf,
        verbose: bool,
        pipes_map: HashMap<String, Vec<Pipe>>,
    ) -> Vec<Addr<CommandActor>> {
        let mut commands: HashMap<String, Addr<CommandActor>> = HashMap::new();

        for (op_name, nexts) in config.build_dag().unwrap().into_iter() {
            let op = config.ops.get(&op_name).unwrap();
            let task_pipes = pipes_map.get(&op_name).unwrap_or(&Vec::new()).clone();

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
                verbose,
                config.env.clone(),
                task_pipes,
            )
            .start();

            if op.depends_on.resolve().is_empty() {
                actor.do_send(Reload::Start)
            }
            commands.insert(op_name, actor);
        }

        commands
            .values()
            .into_iter()
            .map(|i| i.to_owned())
            .collect::<Vec<_>>()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        op_name: String,
        operator: Task,
        console: Addr<ConsoleAct>,
        watcher: Addr<WatcherAct>,
        nexts: Vec<Addr<CommandActor>>,
        base_dir: PathBuf,
        verbose: bool,
        shared_env: HashMap<String, String>,
        pipes: Vec<Pipe>,
    ) -> Self {
        Self {
            op_name,
            operator,
            console,
            watcher,
            arbiter: Arbiter::new(),
            child: Child::NotStarted,
            nexts,
            base_dir,
            self_addr: None,
            pending_upstream: BTreeMap::default(),
            verbose,
            started_at: Local::now(),
            shared_env,
            pipes,
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

    fn load_env(&self, cwd: &Path) -> Vec<(String, String)> {
        let mut env = HashMap::from_iter(env::vars());
        env.extend(resolve_env(&self.shared_env, &env).unwrap());
        for env_file in self.operator.env_file.resolve() {
            let path = cwd.join(env_file.clone());
            let file = fs::read_to_string(path.clone())
                .unwrap_or_else(|_| panic!("cannot find env_file {:?}", path.clone(),));
            let values = parse_dotenv(&file)
                .unwrap_or_else(|_| panic!("cannot parse env_file {:?}", path))
                .into_iter()
                .map(|(k, v)| (k, v.replace("\\n", "\n")));

            env.extend(resolve_env(&values.collect(), &env).unwrap());
        }
        env.extend(resolve_env(&self.operator.env.clone(), &env).unwrap());
        env.into_iter().collect()
    }

    fn reload(&mut self) -> Result<()> {
        let args = &self.operator.command;
        let cwd = match self.operator.workdir.clone() {
            Some(path) => self.base_dir.join(path),
            None => self.base_dir.clone(),
        };
        let env = self.load_env(&cwd);

        self.log_debug(format!("EXEC: {} at {:?}", args, cwd));
        self.console.do_send(PanelStatus {
            panel_name: self.op_name.clone(),
            status: None,
        });

        let mut p = Exec::cmd("bash")
            .cwd(cwd)
            .args(&["-c", args])
            .env_extend(&env.into_iter().collect::<Vec<(String, String)>>())
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
        let operator = self.operator.clone();
        let base_dir = self.base_dir.clone();
        let watcher = self.watcher.clone();
        let task_pipes = self.pipes.clone();

        let fut = async move {
            for line in reader.lines() {
                let mut line = line.unwrap();
                let mut base_dir = base_dir.clone();

                if let Some(workdir) = &operator.workdir {
                    base_dir = base_dir.join(workdir);
                }

                let task_pipe = task_pipes.iter().find(|pipe| pipe.regex.is_match(&line));

                if let Some(task_pipe) = task_pipe {
                    match &task_pipe.redirection {
                        OutputRedirection::Tab(name) => {
                            let name = task_pipe.regex.replace(&line, name).to_string();
                            if let Some(addr) = &self_addr {
                                // tabs must be created on each loop,
                                // as their name can be dynamic
                                console.do_send(RegisterPanel {
                                    name: name.to_owned(),
                                    addr: addr.clone(),
                                });
                            }
                            console.do_send(Output::now(name.to_owned(), line.clone(), false));
                        }
                        OutputRedirection::File(path) => {
                            let path = task_pipe.regex.replace(&line, path);
                            let mut path = Path::new(path.as_ref()).to_path_buf();

                            // prepend base dir if the log file path is relative
                            if !path.starts_with("/") {
                                path = base_dir.join(path);
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
}

impl Actor for CommandActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let addr = ctx.address();
        self.self_addr = Some(addr.clone());

        self.console.do_send(RegisterPanel {
            name: self.op_name.clone(),
            addr,
        });

        let dir = self
            .base_dir
            .join(self.operator.workdir.as_ref().unwrap_or(&"".to_string()))
            .clean();

        let watches = self.operator.watch.resolve();

        if !watches.is_empty() {
            let mut on = GlobSetBuilder::new();
            for pattern in self.operator.watch.resolve() {
                on.add(
                    Glob::new(&dir.join(pattern).absolutize().unwrap().to_string_lossy()).unwrap(),
                );
            }

            let mut off = GlobSetBuilder::new();
            for pattern in self.operator.ignore.resolve() {
                off.add(
                    Glob::new(&dir.join(pattern).absolutize().unwrap().to_string_lossy()).unwrap(),
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
                self.log_info(format!("RELOAD: files {} changed", files));
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
        println!("{:?}", self.child.exit_status());
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

    fn handle(&mut self, msg: StdoutTerminated, _: &mut Self::Context) -> Self::Result {
        if msg.started_at == self.started_at {
            self.ensure_stopped();
            let exit = self.child.exit_status();

            self.console.do_send(PanelStatus {
                panel_name: self.op_name.clone(),
                status: exit,
            });
        }
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PoisonPill;

impl Handler<PoisonPill> for CommandActor {
    type Result = ();

    fn handle(&mut self, _: PoisonPill, ctx: &mut Context<Self>) -> Self::Result {
        ctx.stop();
    }
}

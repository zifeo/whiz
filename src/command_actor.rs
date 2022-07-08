use actix::prelude::*;

use crate::{
    config::Operator,
    console_actor::{ConsoleActor, Output},
};
use duct::{cmd, Handle};
use std::{
    collections::HashMap,
    future::Future,
    io::{BufRead, BufReader},
};

pub struct CommandActor {
    op: String,
    operation: Operator,
    console: Addr<ConsoleActor>,
    arbiter: Arbiter,
    child: Option<Handle>,
}

impl CommandActor {
    pub fn new(op: String, operation: Operator, console: Addr<ConsoleActor>) -> Self {
        Self {
            op,
            operation,
            console,
            arbiter: Arbiter::new(),
            child: None,
        }
    }

    fn run(&mut self) {
        if let Some(child) = &self.child {
            child.kill().unwrap();
            self.child = None;
        }
        
        let args = &self.operation.shell;
        let command = cmd("bash", vec!["-c", args])
            .full_env(self.operation.envs.as_ref().unwrap_or(&HashMap::default()))
            .stderr_to_stdout()
            .stdout_capture();
        let reader = command.reader().unwrap();
        
        self.child = Some(command.start().unwrap());
        
        let console = self.console.clone();
        let op = self.op.clone();
        let fut = async move {
            for line in BufReader::new(reader).lines() {
                console.do_send(Output::now(op.clone(), line.unwrap()));
            }
        
            console.do_send(Output::now(op, "out".to_string()));
        };
        self.arbiter.spawn(fut);
    }
}

impl Actor for CommandActor {
    type Context = Context<Self>;

    fn started(&mut self, _: &mut Context<Self>) {
        self.run();
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        self.child.as_ref().unwrap().kill().unwrap();
        self.child = None;
    }
}

pub struct Reload;

impl Message for Reload {
    type Result = ();
}

impl Handler<Reload> for CommandActor {
    type Result = ();

    fn handle(&mut self, msg: Reload, _: &mut Context<Self>) -> Self::Result {
        self.run();
    }
}

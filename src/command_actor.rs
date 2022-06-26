use actix::prelude::*;

use crate::console_actor::{ConsoleActor, Output};
use duct::{cmd, Handle};
use std::io::{BufRead, BufReader};

pub struct CommandActor {
    console: Addr<ConsoleActor>,
    child: Option<Handle>,
}

impl CommandActor {
    pub fn new(console: Addr<ConsoleActor>) -> Self {
        Self {
            console,
            child: None,
        }
    }
}

impl Actor for CommandActor {
    type Context = Context<Self>;

    fn started(&mut self, _: &mut Context<Self>) {
        let args = vec!["-c", "for ((i=0; i<10; i=i+1)); do echo $i; sleep 1; done"];
        let command = cmd("bash", args).stderr_to_stdout().stdout_capture();
        let reader = command.reader().unwrap();

        self.child = Some(command.start().unwrap());

        let console = self.console.clone();
        Arbiter::new().spawn(async move {
            for line in BufReader::new(reader).lines() {
                console.do_send(Output::new(line.unwrap()));
            }

            console.do_send(Output::new("out".to_string()));
        });
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        self.child.as_ref().unwrap().kill().unwrap();
    }
}

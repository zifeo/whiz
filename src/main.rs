use actix::prelude::*;

use notify::{Watcher, RecursiveMode, watcher};
use std::sync::mpsc::channel;
use std::time::Duration;
use hocon::HoconLoader;
use serde::Deserialize;
use std::io;
use tui::{
    backend::CrosstermBackend,
    widgets::{Widget, Block, Borders},
    layout::{Layout, Constraint, Direction},
    Terminal
};

#[derive(Debug, Deserialize)]
struct Configuration {
    host: String,
    port: u8,
    auto_connect: bool,
}

struct Ping(usize);

impl Message for Ping {
    type Result = usize;
}

/// Actor
struct MyActor {
    count: usize,
}

/// Declare actor and its context
impl Actor for MyActor {
    type Context = Context<Self>;
}

/// Handler for `Ping` message
impl Handler<Ping> for MyActor {
    type Result = usize;

    fn handle(&mut self, msg: Ping, _: &mut Context<Self>) -> Self::Result {
        self.count += msg.0;
        self.count
    }
}

#[actix::main]
async fn main() {
    // start new actor
    let addr = MyActor { count: 10 }.start();

    // send message and get future for result
    let res = addr.send(Ping(10)).await;

    // handle() returns tokio handle
    println!("RESULT: {}", res.unwrap() == 20);


    let conf: Configuration = HoconLoader::new().load_file("dags.hocon").expect("").resolve().expect("msg");

    println!("{:?}", conf);

    let (tx, rx) = channel();

    // Create a watcher object, delivering debounced events.
    // The notification back-end is selected based on the platform.
    let mut watcher = watcher(tx, Duration::from_secs(10)).unwrap();

    // Add a path to be watched. All files and directories at that path and
    // below will be monitored for changes.
    watcher.watch(".", RecursiveMode::Recursive).unwrap();

    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("msg");

    terminal.draw(|f| {
        let size = f.size();
        let block = Block::default()
            .title("Block")
            .borders(Borders::ALL);
        f.render_widget(block, size);
    }).expect("sg");

    loop {
        match rx.recv() {
           Ok(event) => println!("{:?}", event),
           Err(e) => println!("watch error: {:?}", e),
        }
    }
    // stop system and exit
    System::current().stop();
}

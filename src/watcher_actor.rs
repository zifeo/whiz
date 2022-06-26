use actix::prelude::*;

use ignore::gitignore::{Gitignore};
use ignore::Match;
use notify::event::ModifyKind;
use notify::{
    recommended_watcher, Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::path::{Path};
use std::time::Duration;

use crate::console_actor::{ConsoleActor, Output};

pub struct WatcherActor {
    watcher: Option<RecommendedWatcher>,
    console: Addr<ConsoleActor>,
}

impl WatcherActor {
    pub fn new(console: Addr<ConsoleActor>) -> Self {
        Self {
            watcher: None,
            console,
        }
    }
}

impl Actor for WatcherActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let addr = ctx.address();

        let gi = Gitignore::new(Path::new(".gitignore")).0;
        let mut watcher = recommended_watcher(move |res: Result<Event, notify::Error>| {
            let event = res.unwrap();

            let mut go = false;
            for p in &event.paths {
                match gi.matched_path_or_any_parents(p, false) {
                    Match::Ignore(_) => {}
                    _ => go = true,
                }
            }

            match event.kind {
                EventKind::Create(_)
                | EventKind::Remove(_)
                | EventKind::Modify(ModifyKind::Data(_))
                | EventKind::Modify(ModifyKind::Name(_))
                    if go =>
                {
                    addr.do_send(WatchEvent(event));
                }
                _ => {}
            }
        })
        .unwrap();

        watcher
            .configure(Config::OngoingEvents(Some(Duration::from_secs(1))))
            .unwrap();
        watcher.configure(Config::PreciseEvents(false)).unwrap();

        watcher
            .watch(Path::new("."), RecursiveMode::Recursive)
            .unwrap();

        self.watcher = Some(watcher);
    }
}

struct WatchEvent(Event);

impl Message for WatchEvent {
    type Result = ();
}

impl Handler<WatchEvent> for WatcherActor {
    type Result = ();

    fn handle(&mut self, msg: WatchEvent, _: &mut Context<Self>) -> Self::Result {
        self.console.do_send(Output::new(format!("{:?}", msg.0)));

        ()
    }
}

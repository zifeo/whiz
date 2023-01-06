use actix::prelude::*;

use globset::GlobSet;
use ignore::gitignore::Gitignore;
use ignore::Match;
use notify::event::ModifyKind;
use notify::{recommended_watcher, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};

use super::command::{CommandActor, Reload};

pub struct WatcherActor {
    watcher: Option<RecommendedWatcher>,
    globs: Vec<WatchGlob>,
    base_dir: PathBuf,
}

impl WatcherActor {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            watcher: None,
            globs: Vec::default(),
            base_dir,
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

            let paths = event
                .paths
                .iter()
                .filter(|path| {
                    !matches!(
                        gi.matched_path_or_any_parents(path, false),
                        Match::Ignore(_)
                    )
                })
                .map(|path| path.to_path_buf())
                .collect::<Vec<_>>();

            if !paths.is_empty() {
                match event.kind {
                    EventKind::Create(_)
                    | EventKind::Remove(_)
                    | EventKind::Modify(ModifyKind::Data(_))
                    | EventKind::Modify(ModifyKind::Name(_)) => {
                        addr.do_send(WatchEvent(event, paths));
                    }
                    _ => {}
                }
            }
        })
        .unwrap();

        watcher
            .watch(&self.base_dir, RecursiveMode::Recursive)
            .unwrap();

        self.watcher = Some(watcher);
    }
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct WatchGlob {
    pub command: Addr<CommandActor>,
    pub on: GlobSet,
    pub off: GlobSet,
}

impl Handler<WatchGlob> for WatcherActor {
    type Result = ();

    fn handle(&mut self, msg: WatchGlob, _: &mut Context<Self>) -> Self::Result {
        self.globs.push(msg);
    }
}

#[derive(Message)]
#[rtype(result = "()")]
struct WatchEvent(Event, Vec<PathBuf>);

impl Handler<WatchEvent> for WatcherActor {
    type Result = ();

    fn handle(&mut self, msg: WatchEvent, _: &mut Context<Self>) -> Self::Result {
        let WatchEvent(_, paths) = msg;
        for glob in &self.globs {
            let paths = paths
                .iter()
                .filter(|path| glob.on.is_match(path) && !glob.off.is_match(path))
                .collect::<Vec<_>>();

            if !paths.is_empty() {
                let trigger = paths
                    .iter()
                    .map(|p| p.as_path().display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                glob.command.do_send(Reload::Watch(trigger))
            }
        }
    }
}

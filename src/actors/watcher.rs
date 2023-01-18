use actix::prelude::*;

use globset::GlobSet;
use ignore::gitignore::GitignoreBuilder;
use notify::event::ModifyKind;
use notify::{recommended_watcher, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;

use super::command::{CommandActor, Reload};

pub struct WatcherActor {
    watcher: Option<RecommendedWatcher>,
    globs: Vec<WatchGlob>,
    base_dir: PathBuf,
    // List of file paths to ignore on the watcher
    ignore: HashSet<PathBuf>,
}

impl WatcherActor {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            watcher: None,
            globs: Vec::default(),
            base_dir,
            ignore: HashSet::default(),
        }
    }
}

impl Actor for WatcherActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let addr = ctx.address();

        let mut git_ignore_builder = GitignoreBuilder::new(&self.base_dir);
        // add globs from `<project-root>/.gitignore`
        git_ignore_builder.add(self.base_dir.join(".gitignore"));
        // ignore `<project-root>/.git` folder
        git_ignore_builder.add_line(None, ".git/").unwrap();
        let git_ignore = git_ignore_builder.build();

        let mut watcher = recommended_watcher(move |res: Result<Event, notify::Error>| {
            let mut event = res.unwrap();

            if let Ok(git_ignore) = &git_ignore {
                event.paths.retain(|path| {
                    !git_ignore
                        .matched_path_or_any_parents(path, false)
                        .is_ignore()
                })
            };

            if !event.paths.is_empty() {
                match event.kind {
                    EventKind::Create(_)
                    | EventKind::Remove(_)
                    | EventKind::Modify(ModifyKind::Data(_))
                    | EventKind::Modify(ModifyKind::Name(_)) => {
                        addr.do_send(WatchEvent(event));
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
struct WatchEvent(Event);

impl Handler<WatchEvent> for WatcherActor {
    type Result = ();

    fn handle(&mut self, msg: WatchEvent, _: &mut Context<Self>) -> Self::Result {
        let WatchEvent(event) = msg;
        for glob in &self.globs {
            let paths = event
                .paths
                .iter()
                .filter(|path| {
                    !self.ignore.contains(path.as_path())
                        && glob.on.is_match(path)
                        && !glob.off.is_match(path)
                })
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

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct IgnorePath(pub PathBuf);

impl Handler<IgnorePath> for WatcherActor {
    type Result = ();

    fn handle(&mut self, msg: IgnorePath, _: &mut Context<Self>) -> Self::Result {
        let IgnorePath(path) = msg;
        self.ignore.insert(path);
    }
}

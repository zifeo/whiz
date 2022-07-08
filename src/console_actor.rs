use actix::prelude::*;
use chrono::prelude::*;

use std::collections::HashMap;
use std::{cmp::min, collections::VecDeque, io};

use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph, Tabs, Wrap},
    Terminal,
};

use crossterm::{
    cursor::{self},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

pub struct ConsoleActor {
    console: Terminal<CrosstermBackend<io::Stdout>>,
    index: usize,
    titles: Vec<String>,
    logs: HashMap<String, VecDeque<String>>,
}

impl ConsoleActor {
    pub fn new(ops: Vec<&String>) -> Self {
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).unwrap();

        Self {
            console: terminal,
            titles: ops
                .clone()
                .into_iter()
                .map(|x| x.to_string())
                .rev()
                .collect(),
            logs: ops
                .into_iter()
                .map(|op| (op.clone(), VecDeque::with_capacity(5)))
                .collect::<HashMap<_, _>>(),
            index: 0,
        }
    }
    pub fn next(&mut self) {
        self.index = (self.index + 1) % self.titles.len();
    }

    pub fn previous(&mut self) {
        if self.index > 0 {
            self.index -= 1;
        } else {
            self.index = self.titles.len() - 1;
        }
    }

    fn clean(&mut self) {
        self.console
            .draw(|f| {
                let clean =
                    Block::default().style(Style::default().bg(Color::White).fg(Color::Black));
                f.render_widget(clean, f.size());
            })
            .unwrap();
    }

    fn draw(&mut self) {
        self.console
            .draw(|f| {
                let size = f.size();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(3)].as_ref())
                    .split(size);

                let log = Vec::from_iter(
                    self.logs
                        .get(&self.titles[self.index])
                        .unwrap()
                        .clone()
                        .into_iter(),
                );

                let from = log.len() - min(chunks[0].height as usize, log.len());
                let text = log[from..log.len()].to_vec().join("\n");

                let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
                f.render_widget(paragraph, chunks[0]);

                let titles = self
                    .titles
                    .iter()
                    .map(|t| {
                        let (first, rest) = t.split_at(1);
                        Spans::from(vec![
                            Span::styled(first, Style::default().fg(Color::Yellow)),
                            Span::styled(rest, Style::default().fg(Color::Green)),
                        ])
                    })
                    .collect();
                let tabs = Tabs::new(titles)
                    .block(Block::default().borders(Borders::ALL))
                    .select(self.index)
                    .highlight_style(
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .bg(Color::Black),
                    );
                f.render_widget(tabs, chunks[1]);
            })
            .unwrap();
    }
}

impl Actor for ConsoleActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        enable_raw_mode().unwrap();
        execute!(
            self.console.backend_mut(),
            cursor::Hide,
            EnableMouseCapture,
            EnterAlternateScreen,
        )
        .unwrap();

        let addr = ctx.address();
        Arbiter::new().spawn(async move {
            loop {
                addr.do_send(TermEvent(event::read().unwrap()));
            }
        });

        self.clean();
        self.draw();
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        self.clean();

        execute!(
            self.console.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            cursor::Show,
        )
        .unwrap();
        disable_raw_mode().unwrap();
    }
}
struct TermEvent(Event);

impl Message for TermEvent {
    type Result = ();
}

impl Handler<TermEvent> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: TermEvent, _: &mut Context<Self>) -> Self::Result {
        match msg.0 {
            Event::Key(e) => match (e.modifiers, e.code) {
                (KeyModifiers::CONTROL, KeyCode::Char('c')) | (_, KeyCode::Char('q')) => {
                    System::current().stop();
                }
                (_, KeyCode::Right) => {
                    self.next();
                }
                (_, KeyCode::Left) => {
                    self.previous();
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            Event::Mouse(e) => match e.kind {
                MouseEventKind::ScrollUp => {}
                MouseEventKind::ScrollDown => {}
                _ => {}
            },
        }
        self.draw();

        ()
    }
}

pub struct Output {
    op: String,
    message: String,
    timestamp: DateTime<Local>,
}

impl Output {
    pub fn now(op: String, message: String) -> Self {
        Self {
            op,
            message,
            timestamp: Local::now(),
        }
    }
}

impl Message for Output {
    type Result = ();
}

impl Handler<Output> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: Output, _: &mut Context<Self>) -> Self::Result {
        let logs = self.logs.get_mut(&msg.op).unwrap();
        logs.push_back(msg.message);
        self.draw();

        ()
    }
}

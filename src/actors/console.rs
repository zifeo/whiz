use actix::prelude::*;
use chrono::prelude::*;

use std::{
    cmp::{max, min},
    collections::HashMap,
    io,
};

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

use super::command::{CommandActor, PoisonPill};

pub struct Panel {
    logs: Vec<String>,
    shift: usize,
    command: Addr<CommandActor>,
}

impl Panel {
    pub fn new(command: Addr<CommandActor>) -> Self {
        Self {
            logs: Vec::default(),
            shift: 0,
            command,
        }
    }
}

pub struct ConsoleActor {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    index: String,
    order: Vec<String>,
    arbiter: Arbiter,
    panels: HashMap<String, Panel>,
}

impl ConsoleActor {
    pub fn new(order: Vec<String>) -> Self {
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).unwrap();
        Self {
            terminal,
            index: order[0].clone(),
            order,
            arbiter: Arbiter::new(),
            panels: HashMap::default(),
        }
    }

    pub fn idx(&self) -> usize {
        self.order
            .iter()
            .position(|e| e == &self.index)
            .unwrap_or(0)
    }

    pub fn next(&mut self) {
        self.index = self.order[(self.idx() + 1) % self.order.len()].clone();
    }

    pub fn previous(&mut self) {
        self.index = self.order[(self.idx() + self.order.len() - 1) % self.order.len()].clone();
    }

    fn clean(&mut self) {
        self.terminal
            .draw(|f| {
                let clean =
                    Block::default().style(Style::default().bg(Color::White).fg(Color::Black));
                f.render_widget(clean, f.size());
            })
            .unwrap();
    }

    fn draw(&mut self) {
        let idx = self.idx();
        if let Some(focus) = &self.panels.get(&self.index) {
            self.terminal
                .draw(|f| {
                    let size = f.size();
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Min(0), Constraint::Length(3)].as_ref())
                        .split(size);

                    let logs = &focus.logs;
                    let from = max(logs.len() - focus.shift, chunks[0].height as usize)
                        - chunks[0].height as usize;
                    let to = min(from + chunks[0].height as usize, logs.len());
                    let text = logs[from..to].join("\n");

                    let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
                    f.render_widget(paragraph, chunks[0]);

                    let titles = self
                        .order
                        .iter()
                        .map(|panel| {
                            let (first, rest) = panel.split_at(1);
                            Spans::from(vec![
                                Span::styled(first, Style::default().fg(Color::Yellow)),
                                Span::styled(rest, Style::default().fg(Color::Green)),
                            ])
                        })
                        .collect();
                    let tabs = Tabs::new(titles)
                        .block(Block::default().borders(Borders::ALL))
                        .select(idx)
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
}

impl Actor for ConsoleActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        enable_raw_mode().unwrap();
        execute!(
            self.terminal.backend_mut(),
            cursor::Hide,
            EnableMouseCapture,
            EnterAlternateScreen,
        )
        .unwrap();

        let addr = ctx.address();
        self.arbiter.spawn(async move {
            loop {
                addr.do_send(TermEvent(event::read().unwrap()));
            }
        });

        self.clean();
        self.draw();
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        self.arbiter.stop();
        self.clean();

        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            cursor::Show,
        )
        .unwrap();
        disable_raw_mode().unwrap();
    }
}
pub struct TermEvent(Event);

impl Message for TermEvent {
    type Result = ();
}

impl Handler<TermEvent> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: TermEvent, _: &mut Context<Self>) -> Self::Result {
        match msg.0 {
            Event::Key(e) => match (e.modifiers, e.code) {
                (KeyModifiers::CONTROL, KeyCode::Char('c')) | (_, KeyCode::Char('q')) => {
                    self.panels
                        .values()
                        .for_each(|e| e.command.do_send(PoisonPill));
                    System::current().stop();
                }
                (_, KeyCode::Right) => {
                    self.next();
                }
                (_, KeyCode::Left) => {
                    self.previous();
                }
                (_, KeyCode::Up) => {
                    if let Some(focus) = self.panels.get_mut(&self.index) {
                        if focus.shift < focus.logs.len() {
                            focus.shift += 1;
                        }
                    }
                }
                (_, KeyCode::Down) => {
                    if let Some(focus) = self.panels.get_mut(&self.index) {
                        if focus.shift > 1 {
                            focus.shift -= 1;
                        }
                    }
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            Event::Mouse(e) => match e.kind {
                MouseEventKind::ScrollUp => {}
                MouseEventKind::ScrollDown => {}
                _ => {}
            },
            Event::FocusGained => {}
            Event::FocusLost => {}
            Event::Paste(_) => {}
        }
        self.draw();
    }
}

pub struct Output {
    op: String,
    pub message: String,
    _timestamp: DateTime<Local>,
}

impl Output {
    pub fn now(op: String, message: String) -> Self {
        Self {
            op,
            message,
            _timestamp: Local::now(),
        }
    }
}

impl Message for Output {
    type Result = ();
}

impl Handler<Output> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: Output, _: &mut Context<Self>) -> Self::Result {
        let focus = self.panels.get_mut(&msg.op).unwrap();
        focus.logs.push(msg.message);
        self.draw();
    }
}

pub struct Register {
    pub title: String,
    pub addr: Addr<CommandActor>,
}

impl Message for Register {
    type Result = ();
}

impl Handler<Register> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: Register, _: &mut Context<Self>) -> Self::Result {
        self.panels.insert(msg.title.clone(), Panel::new(msg.addr));
        self.draw();
    }
}

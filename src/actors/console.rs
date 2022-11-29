use actix::prelude::*;
use ansi_to_tui::IntoText;
use chrono::prelude::*;
use std::str;
use std::{cmp::min, collections::HashMap, io};
use tui::backend::Backend;
use tui::layout::Rect;
use tui::Frame;

use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph, Tabs, Wrap},
    Terminal,
};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use super::command::{CommandActor, PoisonPill, Reload};

pub struct Panel {
    logs: Vec<(String, Style)>,
    lines: u16,
    shift: u16,
    command: Addr<CommandActor>,
}

impl Panel {
    pub fn new(command: Addr<CommandActor>) -> Self {
        Self {
            logs: Vec::default(),
            lines: 0,
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
    timestamp: bool,
}

pub fn chunks<T: Backend>(f: &Frame<T>) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)].as_ref())
        .split(f.size())
}

impl ConsoleActor {
    pub fn new(order: Vec<String>, timestamp: bool) -> Self {
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).unwrap();
        Self {
            terminal,
            index: order[0].clone(),
            order,
            arbiter: Arbiter::new(),
            panels: HashMap::default(),
            timestamp,
        }
    }

    pub fn up(&mut self) {
        let height = chunks(&self.terminal.get_frame())[0].height;
        if let Some(focus) = self.panels.get_mut(&self.index) {
            if focus.shift < focus.lines - height {
                focus.shift += 1;
            }
        }
    }

    pub fn down(&mut self) {
        if let Some(focus) = self.panels.get_mut(&self.index) {
            if focus.shift >= 1 {
                focus.shift -= 1;
            }
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
                    let chunks = chunks(f);
                    let logs = &focus.logs;

                    let log_height = chunks[0].height as u16;
                    let curr = focus.lines - min(focus.lines, log_height);

                    let lines: Vec<Spans> = logs
                        .iter()
                        .flat_map(|l| {
                            let mut t = l.0.into_text().unwrap();
                            t.patch_style(l.1);
                            t.lines
                        })
                        .collect();

                    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
                    let paragraph = paragraph.scroll((curr - min(curr, focus.shift), 0));
                    f.render_widget(paragraph, chunks[0]);

                    let /*mut*/ titles: Vec<Spans> = self
                        .order
                        .iter()
                        .map(|panel| {
                            Spans::from(Span::styled(panel, Style::default().fg(Color::Green)))
                        })
                        .collect();
                    /*
                    titles.push(Spans::from(Span::raw(format!(
                        "shift {} / window {} / lines {} / max {} / compute {}",
                        focus.shift,
                        log_height,
                        logs.len(),
                        focus.lines,
                        f.size().width,
                    ))));
                    */
                    let tabs = Tabs::new(titles)
                        .block(Block::default().borders(Borders::ALL))
                        .select(idx)
                        .highlight_style(
                            Style::default()
                                .add_modifier(Modifier::BOLD)
                                .bg(Color::DarkGray),
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
            cursor::Show,
        )
        .unwrap();
        disable_raw_mode().unwrap();
    }
}

#[derive(Message, Debug)]
#[rtype(result = "()")]
pub struct TermEvent(Event);

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
                (_, KeyCode::Char('r')) => {
                    if let Some(focus) = self.panels.get(&self.index) {
                        focus.command.do_send(Reload::Manual);
                    }
                }
                (_, KeyCode::Right | KeyCode::Char('l')) => {
                    self.next();
                }
                (_, KeyCode::Left | KeyCode::Char('h')) => {
                    self.previous();
                }
                (_, KeyCode::Up | KeyCode::Char('k'))
                | (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
                    self.up();
                }
                (_, KeyCode::Down | KeyCode::Char('j'))
                | (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
                    self.down();
                }
                _ => {}
            },
            Event::Resize(width, _) => {
                for panel in self.panels.values_mut() {
                    panel.shift = 0;
                    let new_lines = (&panel.logs)
                        .iter()
                        .fold(0, |agg, l| agg + wrapped_lines(&l.0, width));
                    panel.lines = new_lines;
                }
            }
            Event::Mouse(e) => match e.kind {
                MouseEventKind::ScrollUp => {
                    self.up();
                }
                MouseEventKind::ScrollDown => {
                    self.down();
                }
                _ => {}
            },
            _ => {}
        }
        self.draw();
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Output {
    op: String,
    pub message: String,
    service: bool,
    timestamp: DateTime<Local>,
}

impl Output {
    pub fn now(op: String, message: String, service: bool) -> Self {
        Self {
            op,
            message,
            service,
            timestamp: Local::now(),
        }
    }
}

fn wrapped_lines(message: &String, width: u16) -> u16 {
    let clean = strip_ansi_escapes::strip(message).unwrap();
    textwrap::wrap(str::from_utf8(&clean).unwrap(), width as usize).len() as u16
}

impl Handler<Output> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: Output, _: &mut Context<Self>) -> Self::Result {
        let focus = self.panels.get_mut(&msg.op).unwrap();
        let style = match msg.service {
            true => Style::default().bg(Color::DarkGray),
            false => Style::default(),
        };

        let message = match self.timestamp {
            true => format!("{}  {}", msg.timestamp.format("%H:%M:%S%.3f"), msg.message),
            false => msg.message,
        };
        let width = self.terminal.get_frame().size().width;
        focus.lines += wrapped_lines(&message, width);
        focus.logs.push((message, style));
        self.draw();
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Register {
    pub title: String,
    pub addr: Addr<CommandActor>,
}

impl Handler<Register> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: Register, _: &mut Context<Self>) -> Self::Result {
        self.panels.insert(msg.title.clone(), Panel::new(msg.addr));
        self.draw();
    }
}

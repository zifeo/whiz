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

    pub fn up(&mut self, shift: u16) {
        let log_height = self.get_log_height();
        if let Some(focused_panel) = self.panels.get_mut(&self.index) {
            // maximum_scroll is the number of lines
            // overflowing in the current focused panel
            let maximum_scroll = focused_panel.lines - min(focused_panel.lines, log_height);

            // `focused_panel.shift` goes from 0 until maximum_scroll
            focused_panel.shift = min(focused_panel.shift + shift, maximum_scroll);
        }
    }

    pub fn down(&mut self, shift: u16) {
        if let Some(focused_panel) = self.panels.get_mut(&self.index) {
            if focused_panel.shift >= shift {
                focused_panel.shift -= shift;
            } else {
                focused_panel.shift = 0;
            }
        }
    }

    pub fn get_log_height(&mut self) -> u16 {
        let frame = self.terminal.get_frame();
        chunks(&frame)[0].height
    }

    pub fn go_to(&mut self, panel_index: usize) {
        if panel_index < self.order.len() {
            self.index = self.order[panel_index].clone();
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
        if let Some(focused_panel) = &self.panels.get(&self.index) {
            self.terminal
                .draw(|f| {
                    let chunks = chunks(f);
                    let logs = &focused_panel.logs;

                    let log_height = chunks[0].height;
                    let maximum_scroll = focused_panel.lines - min(focused_panel.lines, log_height);

                    let lines: Vec<Spans> = logs
                        .iter()
                        .flat_map(|l| {
                            let mut t = l.0.into_text().unwrap();
                            t.patch_style(l.1);
                            t.lines
                        })
                        .collect();

                    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });

                    // scroll by default until the last line
                    let paragraph = paragraph
                        .scroll((maximum_scroll - min(maximum_scroll, focused_panel.shift), 0));
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
                (KeyModifiers::CONTROL, KeyCode::Char('c'))
                | (KeyModifiers::NONE, KeyCode::Char('q')) => {
                    self.panels
                        .values()
                        .for_each(|e| e.command.do_send(PoisonPill));
                    System::current().stop();
                }
                (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k'))
                | (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
                    self.up(1);
                }
                (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j'))
                | (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
                    self.down(1);
                }
                (KeyModifiers::CONTROL, key_code) => match key_code {
                    KeyCode::Char('f') => {
                        let log_height = self.get_log_height();
                        self.down(log_height);
                    }
                    KeyCode::Char('u') => {
                        let log_height = self.get_log_height();
                        self.up(log_height / 2);
                    }
                    KeyCode::Char('d') => {
                        let log_height = self.get_log_height();
                        self.down(log_height / 2);
                    }
                    KeyCode::Char('b') => {
                        let log_height = self.get_log_height();
                        self.up(log_height);
                    }
                    _ => {}
                },
                (KeyModifiers::NONE, key_code) => match key_code {
                    KeyCode::Char('r') => {
                        if let Some(focused_panel) = self.panels.get(&self.index) {
                            focused_panel.command.do_send(Reload::Manual);
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        self.next();
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        self.previous();
                    }
                    KeyCode::Char(ch) => {
                        if ch.is_ascii_digit() {
                            let mut panel_index = ch.to_digit(10).unwrap() as usize;
                            // first tab is key 1, therefore
                            // in key 0 go to last tab
                            if panel_index == 0 {
                                panel_index = self.order.len() - 1;
                            } else {
                                panel_index -= 1;
                            }
                            self.go_to(panel_index);
                        }
                    }
                    _ => {}
                },
                _ => {}
            },
            Event::Resize(width, _) => {
                for panel in self.panels.values_mut() {
                    panel.shift = 0;
                    let new_lines = panel
                        .logs
                        .iter()
                        .fold(0, |agg, l| agg + wrapped_lines(&l.0, width));
                    panel.lines = new_lines;
                }
            }
            Event::Mouse(e) => match e.kind {
                MouseEventKind::ScrollUp => {
                    self.up(1);
                }
                MouseEventKind::ScrollDown => {
                    self.down(1);
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
        let focused_panel = self.panels.get_mut(&msg.op).unwrap();
        let style = match msg.service {
            true => Style::default().bg(Color::DarkGray),
            false => Style::default(),
        };

        let message = match self.timestamp {
            true => format!("{}  {}", msg.timestamp.format("%H:%M:%S%.3f"), msg.message),
            false => msg.message,
        };
        let width = self.terminal.get_frame().size().width;
        focused_panel.lines += wrapped_lines(&message, width);
        focused_panel.logs.push((message, style));
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

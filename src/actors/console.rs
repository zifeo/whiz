use actix::prelude::*;
use chrono::prelude::*;
use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::prelude::Alignment;
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::Frame;
use std::borrow::Cow;
use std::rc::Rc;
use std::{cmp::min, collections::HashMap, io};
use std::{str, usize};
use subprocess::ExitStatus;

use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Paragraph, Tabs, Wrap},
    Terminal,
};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::config::color::{ColorOption, Colorizer};

use super::command::{CommandActor, PoisonPill, Reload};

const MENU_WIDTH: u16 = 30;
const MAX_CHARS: usize = (MENU_WIDTH - 6) as usize;

enum LayoutDirection {
    Horizontal,
    Vertical,
}

impl LayoutDirection {
    fn get_opposite_orientation(&self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

enum AppMode {
    Menu,
    View,
}

impl AppMode {
    fn get_opposite_mode(&self) -> Self {
        match self {
            Self::View => Self::Menu,
            Self::Menu => Self::View,
        }
    }
}

pub struct Panel {
    logs: Vec<(String, Style)>,
    lines: u16,
    shift: u16,
    command: Addr<CommandActor>,
    status: Option<ExitStatus>,
    colors: Vec<ColorOption>,
}

impl Panel {
    pub fn new(command: Addr<CommandActor>, colors: Vec<ColorOption>) -> Self {
        Self {
            logs: Vec::default(),
            lines: 0,
            shift: 0,
            command,
            status: None,
            colors,
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
    layout_direction: LayoutDirection,
    mode: AppMode,
    list_state: ListState,
}

fn chunks(mode: &AppMode, direction: &LayoutDirection, f: &Frame) -> Rc<[Rect]> {
    let chunks_constraints = match mode {
        AppMode::Menu => match direction {
            LayoutDirection::Horizontal => vec![Constraint::Min(0), Constraint::Length(3)],
            LayoutDirection::Vertical => vec![Constraint::Min(0), Constraint::Length(MENU_WIDTH)],
        },
        AppMode::View => vec![Constraint::Min(0)],
    };
    let direction = match direction {
        LayoutDirection::Horizontal => Direction::Vertical,
        LayoutDirection::Vertical => Direction::Horizontal,
    };
    Layout::default()
        .direction(direction)
        .constraints(chunks_constraints)
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
            mode: AppMode::Menu,
            layout_direction: LayoutDirection::Horizontal,
            list_state: ListState::default().with_selected(Some(0)),
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
        chunks(&self.mode, &self.layout_direction, &frame)[0].height
    }

    pub fn go_to(&mut self, panel_index: usize) {
        if panel_index < self.order.len() {
            self.index.clone_from(&self.order[panel_index]);
        }
    }

    pub fn idx(&self) -> usize {
        self.order
            .iter()
            .position(|e| e == &self.index)
            .unwrap_or(0)
    }

    pub fn next(&mut self) {
        self.index
            .clone_from(&self.order[(self.idx() + 1) % self.order.len()]);
        self.list_state.select(Some(self.idx()))
    }

    pub fn previous(&mut self) {
        self.index
            .clone_from(&self.order[(self.idx() + self.order.len() - 1) % self.order.len()]);
        self.list_state.select(Some(self.idx()))
    }

    fn clean(&mut self) {
        self.terminal
            .draw(|f| {
                let clean = Block::default().style(Style::default().fg(Color::Black));
                f.render_widget(clean, f.size());
            })
            .unwrap();
    }

    fn draw(&mut self) {
        let idx = self.idx();
        if let Some(focused_panel) = &self.panels.get(&self.index) {
            self.terminal
                .draw(|f| {
                    let chunks = chunks(&self.mode, &self.layout_direction, f);
                    let logs = &focused_panel.logs;

                    let log_height = chunks[0].height;
                    let maximum_scroll = focused_panel.lines - min(focused_panel.lines, log_height);

                    let lines: Vec<Line> = logs
                        .iter()
                        .flat_map(|(str, base_style)| {
                            let colorizer = Colorizer::new(&focused_panel.colors, *base_style);
                            colorizer.patch_text(str)
                        })
                        .collect();

                    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });

                    // scroll by default until the last line
                    let paragraph = paragraph
                        .scroll((maximum_scroll - min(maximum_scroll, focused_panel.shift), 0));
                    f.render_widget(paragraph, chunks[0]);

                    //Format titles
                    let titles: Vec<Line> = self
                        .order
                        .iter()
                        .map(|panel| {
                            let mut span = self
                                .panels
                                .get(panel)
                                .map(|p| match p.status {
                                    Some(ExitStatus::Exited(0)) => Span::styled(
                                        format!("{}.", panel),
                                        Style::default().fg(Color::Green),
                                    ),
                                    Some(_) => Span::styled(
                                        format!("{}!", panel),
                                        Style::default().fg(Color::Red),
                                    ),
                                    None => Span::styled(format!("{}*", panel), Style::default()),
                                })
                                .unwrap_or_else(|| Span::styled(panel, Style::default()));
                            // Replace the titles whoms length is greater than MAX_CHARS with an
                            // ellipse
                            span = Span::styled(
                                ellipse_if_too_long(span.content).into_owned(),
                                span.style,
                            );
                            Line::from(span)
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
                    match self.mode {
                        AppMode::Menu => {
                            match self.layout_direction {
                                LayoutDirection::Horizontal => {
                                    let tabs = Tabs::new(titles)
                                        .block(Block::default().borders(Borders::ALL))
                                        .select(idx)
                                        .highlight_style(
                                            Style::default()
                                                .add_modifier(Modifier::BOLD)
                                                .bg(Color::DarkGray),
                                        );
                                    f.render_widget(tabs, chunks[1]);
                                }
                                LayoutDirection::Vertical => {
                                    let list = List::new(
                                        titles
                                            .into_iter()
                                            .map(ListItem::new)
                                            .collect::<Vec<ListItem>>(),
                                    )
                                    .block(
                                        Block::default()
                                            .borders(Borders::ALL)
                                            .title("Task List")
                                            .title_alignment(Alignment::Center),
                                    )
                                    .highlight_style(
                                        Style::default()
                                            .bg(Color::DarkGray)
                                            .add_modifier(Modifier::BOLD),
                                    );
                                    f.render_stateful_widget(list, chunks[1], &mut self.list_state)
                                }
                            };
                        }
                        AppMode::View => {}
                    };
                })
                .unwrap();
        }
    }

    pub fn switch_layout(&mut self) {
        self.layout_direction = self.layout_direction.get_opposite_orientation();
    }
    pub fn switch_mode(&mut self) {
        self.mode = self.mode.get_opposite_mode();
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

impl TermEvent {
    pub fn quit() -> Self {
        Self(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )))
    }
}

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
                    KeyCode::Tab => self.switch_layout(),
                    KeyCode::Char('m') => self.switch_mode(),
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
    panel_name: String,
    pub message: String,
    service: bool,
    timestamp: DateTime<Local>,
}

impl Output {
    pub fn now(panel_name: String, message: String, service: bool) -> Self {
        Self {
            panel_name,
            message,
            service,
            timestamp: Local::now(),
        }
    }
}

fn wrapped_lines(message: &String, width: u16) -> u16 {
    let clean = strip_ansi_escapes::strip(message);
    textwrap::wrap(str::from_utf8(&clean).unwrap(), width as usize).len() as u16
}

// Replace the character that are max that MAX_CHARS with an ellipse ...
fn ellipse_if_too_long(task_title: Cow<'_, str>) -> Cow<str> {
    if task_title.len() >= MAX_CHARS {
        let mut task_title = task_title.to_string();
        task_title.replace_range(MAX_CHARS.., "...");
        Cow::Owned(task_title.to_string())
    } else {
        task_title
    }
}

/// Formats a message with a timestamp in `"{timestamp}  {message}"`.
fn format_message(message: &str, timestamp: &DateTime<Local>) -> String {
    format!("{}  {}", timestamp.format("%H:%M:%S%.3f"), message)
}

impl Handler<Output> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: Output, _: &mut Context<Self>) -> Self::Result {
        let panel = self.panels.get_mut(&msg.panel_name).unwrap();
        let style = match msg.service {
            true => Style::default().bg(Color::DarkGray),
            false => Style::default(),
        };

        let message = match self.timestamp {
            true => format_message(&msg.message, &msg.timestamp),
            false => msg.message,
        };
        let width = self.terminal.get_frame().size().width;
        panel.lines += wrapped_lines(&message, width);
        panel.logs.push((message, style));
        self.draw();
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct RegisterPanel {
    pub name: String,
    pub addr: Addr<CommandActor>,
    pub colors: Vec<ColorOption>,
}

impl Handler<RegisterPanel> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: RegisterPanel, _: &mut Context<Self>) -> Self::Result {
        if !self.panels.contains_key(&msg.name) {
            let new_panel = Panel::new(msg.addr, msg.colors);
            self.panels.insert(msg.name.clone(), new_panel);
        }
        if !self.order.contains(&msg.name) {
            self.order.push(msg.name);
        }
        self.draw();
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PanelStatus {
    pub panel_name: String,
    pub status: Option<ExitStatus>,
}

impl Handler<PanelStatus> for ConsoleActor {
    type Result = ();

    fn handle(&mut self, msg: PanelStatus, ctx: &mut Context<Self>) -> Self::Result {
        let focused_panel = self.panels.get_mut(&msg.panel_name).unwrap();
        focused_panel.status = msg.status;

        if let Some(message) = msg.status.map(|c| format!("Status: {:?}", c)) {
            ctx.address()
                .do_send(Output::now(msg.panel_name, message, true));
        }

        self.draw();
    }
}

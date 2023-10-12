use std::{fmt::Display, rc::Rc};

use crossterm::event::KeyCode;
use ratatui::{
    prelude::{Backend, Constraint, Layout, Rect},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};
use termgraph::{LineGlyphBuilder, LineGlyphs, NodeFormat};

pub enum LineFormat {
    Ascii,
    Boxed,
}

#[derive(PartialEq)]
pub enum Message {
    ScrollDown,
    ScrollUp,
    ScrollRight,
    ScrollLeft,
    Quit,
}

#[derive(Default)]
pub struct Model {
    vertical_scroll_state: ScrollbarState,
    horizontal_scroll_state: ScrollbarState,
    vertical_scroll: u16,
    horizontal_scroll: u16,
    pub should_quit: bool,
    graph_string_representation: String,
    indipendent_tasks: String,
}

impl Model {
    pub fn new(graph_string_representation: &[u8], indipendent_tasks: String) -> Self {
        Model {
            vertical_scroll: 0,
            horizontal_scroll: 0,
            should_quit: false,
            horizontal_scroll_state: ScrollbarState::default(),
            vertical_scroll_state: ScrollbarState::default(),
            graph_string_representation: String::from_utf8_lossy(graph_string_representation)
                .into_owned(),
            indipendent_tasks,
        }
    }
}

pub fn handle_key_event() -> Result<Option<Message>, Box<dyn std::error::Error>> {
    let message = if crossterm::event::poll(std::time::Duration::from_millis(250))? {
        if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
            match key.code {
                KeyCode::Char('q') => Message::Quit,
                KeyCode::Char('j') | KeyCode::Down => Message::ScrollDown,
                KeyCode::Char('k') | KeyCode::Up => Message::ScrollUp,
                KeyCode::Char('h') | KeyCode::Left => Message::ScrollLeft,
                KeyCode::Char('l') | KeyCode::Right => Message::ScrollRight,
                _ => return Ok(None),
            }
        } else {
            return Ok(None);
        }
    } else {
        return Ok(None);
    };

    Ok(Some(message))
}

pub fn update(model: &mut Model, msg: Message) -> Option<Message> {
    use Message::*;
    match msg {
        ScrollRight => {
            model.horizontal_scroll = model.horizontal_scroll.saturating_add(5);
            model.horizontal_scroll_state = model
                .horizontal_scroll_state
                .position(model.horizontal_scroll);
        }
        ScrollLeft => {
            model.horizontal_scroll = model.horizontal_scroll.saturating_sub(5);
            model.horizontal_scroll_state = model
                .horizontal_scroll_state
                .position(model.horizontal_scroll);
        }
        ScrollUp => {
            model.vertical_scroll = model.vertical_scroll.saturating_sub(5);
            model.vertical_scroll_state =
                model.vertical_scroll_state.position(model.vertical_scroll);
        }

        ScrollDown => {
            model.vertical_scroll = model.vertical_scroll.saturating_add(5);
            model.vertical_scroll_state =
                model.vertical_scroll_state.position(model.vertical_scroll);
        }
        Quit => model.should_quit = true,
    }
    None
}

pub struct Drawer {}
impl Drawer {
    fn render_indipendent_tasks<B: Backend>(
        frame: &mut Frame<B>,
        chunks: Rc<[Rect]>,
        model: &mut Model,
    ) {
        frame.render_widget(
            Paragraph::new(model.indipendent_tasks.as_str())
                .block(
                    Block::new()
                        .title("Indipendent task")
                        .title_alignment(ratatui::prelude::Alignment::Center)
                        .borders(Borders::ALL),
                )
                // .alignment(ratatui::prelude::Alignment::Center)
                .scroll((0, model.horizontal_scroll)),
            chunks.clone()[0],
        );
    }

    fn render_dependency_graph<B: Backend>(
        frame: &mut Frame<B>,
        chunks: Rc<[Rect]>,
        model: &mut Model,
    ) {
        frame.render_widget(
            Paragraph::new(model.graph_string_representation.to_owned())
                .block(
                    Block::new()
                        .title("Dependency Graph")
                        .title_alignment(ratatui::prelude::Alignment::Center)
                        .borders(Borders::ALL),
                )
                .scroll((model.vertical_scroll, model.horizontal_scroll)),
            chunks.clone()[1],
        );
    }

    pub fn render_scrollbar<B: Backend>(
        model: &mut Model,
        frame: &mut Frame<B>,
        chunks: Rc<[Rect]>,
    ) {
        frame.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::HorizontalTop),
            chunks[1],
            &mut model.horizontal_scroll_state,
        );

        frame.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalLeft),
            chunks[1],
            &mut model.vertical_scroll_state,
        );
    }

    pub fn get_layout<T: Backend>(frame: &Frame<T>) -> Rc<[Rect]> {
        Layout::default()
            .direction(ratatui::prelude::Direction::Vertical)
            .constraints(vec![Constraint::Length(5), Constraint::Min(0)])
            .split(frame.size())
    }

    pub fn draw<B: Backend>(model: &mut Model, frame: &mut Frame<B>) {
        let chunks = Self::get_layout(frame);
        Self::render_scrollbar(model, frame, chunks.clone());
        Self::render_dependency_graph(frame, chunks.clone(), model);
        Self::render_indipendent_tasks(frame, chunks.clone(), model);
    }
}

pub struct TaskFormatter {}
impl TaskFormatter {
    /// Creates a new Instance of the Formatter
    pub fn new() -> Self {
        Self {}
    }

    pub fn from_commandline(line_format: LineFormat) -> LineGlyphs {
        match line_format {
            LineFormat::Ascii => LineGlyphBuilder::ascii().finish(),
            LineFormat::Boxed => LineGlyphBuilder::ascii()
                .vertical('\u{2502}')
                .crossing('\u{253C}')
                .horizontal('\u{2500}')
                .arrow_down('▼')
                .finish(),
        }
    }
}

impl<ID, T> NodeFormat<ID, T> for TaskFormatter
where
    T: Display,
{
    fn format_node(&self, _: &ID, name: &T) -> String {
        format!("|{}|", name)
    }
}

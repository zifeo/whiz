pub use graph_task::{Graph, Task};
use ratatui::prelude::{CrosstermBackend, Terminal};
use std::error::Error;
use termgraph::fdisplay;

use ui::{Drawer, Model, TaskFormatter};

use self::ui::LineFormat;

pub mod graph_task;
mod ui;

pub fn draw_graph(tasks_list: Vec<Task>, boxed: bool) -> Result<(), Box<dyn Error>> {
    // let config = command::parse_argument()?;
    // let tasks = Task::from_file(config.file_path)?;
    let boxed = match boxed {
        true => LineFormat::Boxed,
        _ => LineFormat::Ascii,
    };
    let graph = Graph::from_tasks_list(&tasks_list);

    //use termgraph to generate the ascii representation
    let config = termgraph::Config::new(TaskFormatter::new(), 200)
        .line_glyphs(TaskFormatter::from_commandline(boxed));
    let mut ascii_graph = termgraph::DirectedGraph::new();
    ascii_graph.add_nodes(graph.nodes());
    ascii_graph.add_edges(graph.edges());

    // Write graphics into the buffer
    let mut formatted_ascii_graph = Vec::new();
    fdisplay(&ascii_graph, &config, &mut formatted_ascii_graph);

    //Start ratatui initializaion
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stderr(), crossterm::terminal::EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stderr()))?;

    // let mut ui = Model::default();
    let mut ui = Model::new(&formatted_ascii_graph, graph.format_independent_task());

    loop {
        terminal.draw(|f| {
            Drawer::draw(&mut ui, f);
        })?;

        let mut current_msg = ui::handle_key_event()?;

        while current_msg.is_some() {
            current_msg = ui::update(&mut ui, current_msg.unwrap())
        }

        if ui.should_quit {
            break;
        }
    }

    crossterm::execute!(std::io::stderr(), crossterm::terminal::LeaveAlternateScreen)?;
    crossterm::terminal::disable_raw_mode()?;

    Ok(())
}

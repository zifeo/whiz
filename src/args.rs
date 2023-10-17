use clap::{Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
pub struct Upgrade {
    /// Upgrade to specific version (e.g. 1.0.0)
    #[arg(long)]
    pub version: Option<String>,

    /// Do not ask for version confirmation
    #[arg(short, long, default_value_t = false)]
    pub yes: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct Graph {
    /// Draw the line using box-drawing character
    #[arg(long, short, default_value_t = false)]
    pub boxed: bool,
}

/// Set of subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Upgrade whiz.
    Upgrade(Upgrade),
    /// Print the graphical ascii representation
    Graph(Graph),
    /// List all the jobs set in the config file
    ListJobs,
}

#[derive(Parser, Debug)]
#[command(
    name = "whiz",
    about,
    long_about= None,
)]
pub struct Args {
    #[arg(long)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(short, long, default_value = "whiz.yaml")]
    pub file: String,

    #[arg(short, long)]
    pub verbose: bool,

    #[arg(short, long)]
    pub timestamp: bool,

    /// Run specific jobs
    #[arg(short, long, value_name = "JOB")]
    pub run: Vec<String>,
}

use clap::{Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
pub struct Upgrade {
    /// Upgrade to specific version (e.g. 1.0.0)
    #[clap(long)]
    pub version: Option<String>,

    /// Do not ask for version confirmation
    #[clap(short, long, default_value_t = false)]
    pub yes: bool,
}

/// Set of subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Upgrade whiz.
    Upgrade(Upgrade),
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    #[clap(subcommand)]
    pub command: Option<Command>,

    #[clap(short, long, default_value = "whiz.yaml")]
    pub file: String,

    #[clap(short, long)]
    pub verbose: bool,

    #[clap(short, long)]
    pub timestamp: bool,

    /// Run specific jobs
    #[clap(short, long, value_name = "JOB")]
    pub run: Vec<String>,

    /// List all the jobs set in the config file
    #[clap(long)]
    pub list_jobs: bool,
}

use clap::Subcommand;

pub mod upgrade;

/// Set of subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Upgrade to the latest version of whiz.
    Upgrade,
}

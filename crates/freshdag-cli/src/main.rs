//! `freshdag` — command-line entry point.
//!
//! Commands land in follow-up PRs (see `docs/BUILD_PLAN.md`). This binary
//! intentionally does nothing beyond parsing today; behaviour arrives with
//! the engine and store implementations.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "freshdag",
    version,
    about = "Dynamic dependency tracking and validity certificates for agent-generated artifacts."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Explain why an artifact is stale.
    Why { artifact: String },
    /// Print an artifact's validity certificate.
    Cert { artifact: String },
    /// Emit the dependency graph.
    Graph,
    /// Watch for dependency changes.
    Watch,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Why { artifact }) => {
            eprintln!("freshdag why: not implemented yet ({artifact})");
        }
        Some(Command::Cert { artifact }) => {
            eprintln!("freshdag cert: not implemented yet ({artifact})");
        }
        Some(Command::Graph) => {
            eprintln!("freshdag graph: not implemented yet");
        }
        Some(Command::Watch) => {
            eprintln!("freshdag watch: not implemented yet");
        }
        None => {
            eprintln!("freshdag: run `freshdag --help` to see available commands.");
        }
    }
}

//! `freshdag` — command-line entry point.
//!
//! `check` is the v0 surface (`ARCHITECTURE.md §11`). The remaining
//! subcommands are stubs and say so, exiting `> 2` rather than `0`: a
//! stub that exits `0` tells a CI job "all good" about work that never
//! happened, which is the same class of lie invariant #7 exists to
//! prevent.

mod check;
mod exit;
mod render;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use exit::Exit;

/// Default store root, relative to the working directory.
const DEFAULT_STORE: &str = ".freshdag";

#[derive(Debug, Parser)]
#[command(
    name = "freshdag",
    version,
    about = "Dynamic dependency tracking and validity certificates for agent-generated artifacts.",
    long_about = "Dynamic dependency tracking and validity certificates for \
agent-generated artifacts.\n\n\
`freshdag check` answers one question: may I reuse this artifact? Exit \
codes are contractual — 0 valid, 1 stale, 2 unknown, anything above 2 \
is a tool error rather than a verdict."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Probe an artifact's dependencies and report its validity.
    Check(CheckArgs),
    /// Explain why an artifact is stale.
    Why {
        /// Artifact id or path.
        artifact: String,
    },
    /// Print an artifact's validity certificate.
    Cert {
        /// Artifact id or path.
        artifact: String,
    },
    /// Emit the dependency graph.
    Graph,
    /// Watch for dependency changes.
    Watch,
}

#[derive(Debug, Args)]
struct CheckArgs {
    /// Artifact id (`blake3:…`) or the path recorded for it.
    artifact: String,

    /// Store root holding the canonical observation log.
    #[arg(long, default_value = DEFAULT_STORE, value_name = "DIR")]
    store: PathBuf,

    /// Emit the certificate as JSON. Exit codes are unchanged.
    #[arg(long)]
    json: bool,

    /// Treat `likely-valid` as reusable (exit 0 instead of 2).
    ///
    /// Off by default: `likely-valid` means nothing drifted but at
    /// least one dependency could only be verified heuristically, and
    /// invariant #8 requires that stay distinguishable from a proof.
    /// Turning it on is an operator's explicit decision, not the
    /// tool's.
    #[arg(long)]
    accept_likely_valid: bool,
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    // clap's own error exit code is 2, which this CLI defines as
    // "unknown". Parsing by hand keeps a typo'd flag from looking like
    // a validity verdict.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let _ = err.print();
            // `--help` / `--version` are `Err` but not failures.
            return if err.use_stderr() {
                Exit::Usage.code()
            } else {
                0
            };
        }
    };

    let exit = match cli.command {
        Some(Command::Check(args)) => check_command(&args),
        Some(Command::Why { artifact }) => unimplemented_command("why", Some(&artifact)),
        Some(Command::Cert { artifact }) => unimplemented_command("cert", Some(&artifact)),
        Some(Command::Graph) => unimplemented_command("graph", None),
        Some(Command::Watch) => unimplemented_command("watch", None),
        None => {
            eprintln!("freshdag: run `freshdag --help` to see available commands.");
            Exit::Usage
        }
    };
    exit.code()
}

fn check_command(args: &CheckArgs) -> Exit {
    let checked = match check::run(&args.store, &args.artifact) {
        Ok(checked) => checked,
        Err(err) => {
            eprintln!("freshdag check: {err}");
            return err.exit();
        }
    };

    for warning in &checked.warnings {
        eprintln!("freshdag check: warning: {warning}");
    }

    let exit = Exit::for_status_with_reasons(
        checked.certificate.status.value,
        &checked.certificate.status.reasons,
        args.accept_likely_valid,
    );

    if args.json {
        match check::to_json(&checked.certificate) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("freshdag check: {err}");
                return err.exit();
            }
        }
    } else {
        print!("{}", render::certificate(&checked.certificate, exit));
    }

    exit
}

fn unimplemented_command(name: &str, artifact: Option<&str>) -> Exit {
    match artifact {
        Some(artifact) => eprintln!("freshdag {name}: not implemented yet ({artifact})"),
        None => eprintln!("freshdag {name}: not implemented yet"),
    }
    eprintln!(
        "freshdag {name}: exiting {} — an unimplemented command must not \
         report success.",
        Exit::NotImplemented.code()
    );
    Exit::NotImplemented
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn unimplemented_commands_never_report_success() {
        for name in ["why", "cert", "graph", "watch"] {
            let exit = unimplemented_command(name, None);
            assert!(
                exit.code() > 2,
                "`freshdag {name}` exits {}; a stub must not look like a verdict",
                exit.code()
            );
        }
    }
}

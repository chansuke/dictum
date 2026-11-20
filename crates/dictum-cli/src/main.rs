//! `dictum eval`, `dictum repl` and `dictum schema infer`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use dictum::Limits;
use dictum::jiff::Timestamp;
use dictum_cli::{Session, infer_schema};

#[derive(Parser)]
#[command(name = "dictum", version, about = "Evaluate dictum rules over JSON facts")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Evaluate one expression and print its value.
    Eval {
        #[command(flatten)]
        session: SessionArgs,
        /// The rule, e.g. 'user.Age >= 18'.
        expr: String,
    },
    /// Read expressions from standard input, one per line, and print each value.
    Repl {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Work with schemas.
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
}

#[derive(Subcommand)]
enum SchemaCommand {
    /// Print a draft schema for a JSON facts file, to review and edit.
    Infer {
        /// The facts, as JSON.
        facts: PathBuf,
    },
}

#[derive(Args)]
struct SessionArgs {
    /// The TOML schema that describes the facts.
    #[arg(long)]
    schema: PathBuf,
    /// The facts, as JSON.
    #[arg(long)]
    facts: PathBuf,
    /// What now() returns, in RFC 3339 (default: the current time).
    #[arg(long)]
    now: Option<Timestamp>,
    /// The step limit of one evaluation.
    #[arg(long)]
    max_steps: Option<u64>,
    /// The allocation limit of one evaluation.
    #[arg(long)]
    max_alloc: Option<u64>,
}

impl SessionArgs {
    fn session(&self) -> Result<Session, String> {
        let (schema, facts) = (read(&self.schema)?, read(&self.facts)?);
        let mut limits = Limits::default();
        if let Some(n) = self.max_steps {
            limits = limits.with_max_steps(n);
        }
        if let Some(n) = self.max_alloc {
            limits = limits.with_max_alloc(n);
        }
        Ok(Session::new(&schema, &facts, self.now.unwrap_or_else(Timestamp::now))?.with_limits(limits))
    }
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The rule's value, or its error and exit status 1. Bad input is status 2.
fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Eval { session, expr } => session.session().map(|s| match s.eval(&expr) {
            Ok(v) => {
                println!("{v}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(1)
            }
        }),
        Command::Repl { session } => session.session().map(|s| repl(&s)),
        Command::Schema { command: SchemaCommand::Infer { facts } } => {
            read(&facts).and_then(|f| infer_schema(&f)).map(|s| {
                print!("{s}");
                ExitCode::SUCCESS
            })
        }
    };
    result.unwrap_or_else(|e| {
        eprintln!("dictum: {e}");
        ExitCode::from(2)
    })
}

/// One expression per line until `:quit` or the end of input. A failing line prints
/// its error and the loop goes on.
fn repl(session: &Session) -> ExitCode {
    let interactive = std::io::stdin().is_terminal();
    let prompt = || {
        if interactive {
            print!("dictum> ");
            let _ = std::io::stdout().flush();
        }
    };
    prompt();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let expr = line.trim();
        if expr == ":quit" || expr == ":q" {
            break;
        }
        if !expr.is_empty() {
            match session.eval(expr) {
                Ok(v) => println!("{v}"),
                Err(e) => println!("{e}"),
            }
        }
        prompt();
    }
    ExitCode::SUCCESS
}

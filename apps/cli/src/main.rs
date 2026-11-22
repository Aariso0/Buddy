use buddy_core::session::{create_session, read_session, recover_session, Preset, SessionManifest};
use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "buddy-cli")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    New {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        id: String,
    },
    Show {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        id: String,
    },
    Recover {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        id: String,
    },
}

// RecoveryOutcome does not derive Serialize; the harness exposes only these
// two keys and leaves buddy-core unchanged.
#[derive(Serialize)]
struct RecoveryJson<'a> {
    needs_repair: bool,
    manifest: &'a SessionManifest,
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Commands) -> Result<String, Box<dyn std::error::Error>> {
    match command {
        Commands::Session { command } => match command {
            SessionCommands::New { root, id } => {
                let manifest = create_session(&root, &id, Utc::now(), Preset::Lecture)?;
                Ok(serde_json::to_string(&manifest)?)
            }
            SessionCommands::Show { root, id } => {
                let manifest = read_session(&root, &id)?;
                Ok(serde_json::to_string(&manifest)?)
            }
            SessionCommands::Recover { root, id } => {
                let outcome = recover_session(&root, &id)?;
                Ok(serde_json::to_string(&RecoveryJson {
                    needs_repair: outcome.needs_repair,
                    manifest: &outcome.manifest,
                })?)
            }
        },
    }
}

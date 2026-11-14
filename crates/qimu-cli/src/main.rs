//! qimu CLI — functionally equivalent to the GUI across all operations
//! (spec §4.4): fully non-interactive, JSON in/out, stable exit codes.
//! POC stage covers the deck inspection + split/reassemble verification
//! commands; the command surface grows with the core library.

use clap::{Parser, Subcommand};
use qimu_core::pptx::{self, Package};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "qimu", version, about = "启幕 PPT 管理器 CLI")]
struct Cli {
    /// Emit JSON (POC commands always emit JSON; the flag is accepted for
    /// forward compatibility with the full command surface).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Operations on a single .pptx file.
    Deck {
        #[command(subcommand)]
        cmd: DeckCommand,
    },
}

#[derive(Subcommand)]
enum DeckCommand {
    /// Basic facts about a deck: size, slide count, per-page fingerprints.
    Info { path: String },
    /// Split into per-slide fragments, reassemble, verify page-by-page
    /// against the source (the §3.1 import safety net).
    SplitVerify { path: String },
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    match cli.command {
        Command::Deck { cmd } => match cmd {
            DeckCommand::Info { path } => {
                let bytes = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;
                let pkg = Package::load(&bytes)?;
                let slides = pptx::slides(&pkg)?;
                let out = serde_json::json!({
                    "path": path,
                    "bytes": bytes.len(),
                    "slide_count": slides.len(),
                    "package_fingerprint": pptx::package_fingerprint(&pkg),
                    "slides": slides,
                });
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
                Ok(())
            }
            DeckCommand::SplitVerify { path } => {
                let bytes = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;
                let pkg = Package::load(&bytes)?;
                let report = pptx::split_reassemble_verify(&pkg)?;
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
                if report.ok {
                    Ok(())
                } else {
                    Err("verification failed".into())
                }
            }
        },
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("{}", serde_json::json!({ "error": e }));
            ExitCode::from(1)
        }
    }
}

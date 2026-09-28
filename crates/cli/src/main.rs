use std::path::PathBuf;

use anyhow::Result;
use archroom_catalog::Catalog;
use clap::{Parser, Subcommand};

/// `archroom-cli`: the headless entry point into the engine, catalog and
/// services crates (plan §4.1) — used for tests, batch jobs and benchmarks,
/// and to keep the UI toolkit replaceable.
#[derive(Parser)]
#[command(name = "archroom-cli", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Catalog maintenance: create/open and integrity-check a `.arcat` file.
    Catalog {
        #[command(subcommand)]
        action: CatalogAction,
    },
}

#[derive(Subcommand)]
enum CatalogAction {
    /// Create the catalog if missing, then run `PRAGMA integrity_check`.
    Check { path: PathBuf },
    /// Copy the catalog to `dest` via the SQLite online-backup API.
    Backup { path: PathBuf, dest: PathBuf },
}

fn main() -> Result<()> {
    archroom_core::tracing_setup::init();
    let cli = Cli::parse();

    match cli.command {
        Command::Catalog { action } => match action {
            CatalogAction::Check { path } => {
                let catalog = Catalog::create_or_open(&path)?;
                if catalog.integrity_check()? {
                    println!("ok: {}", path.display());
                } else {
                    anyhow::bail!("integrity check failed for {}", path.display());
                }
            }
            CatalogAction::Backup { path, dest } => {
                let catalog = Catalog::create_or_open(&path)?;
                catalog.backup_to(&dest)?;
                println!("backed up {} -> {}", path.display(), dest.display());
            }
        },
    }

    Ok(())
}

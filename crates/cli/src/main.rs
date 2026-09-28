use std::path::PathBuf;

use anyhow::Result;
use archroom_catalog::Catalog;
use archroom_services::import::{ImportMode, ImportOptions, ImportProgressSink, run_import};
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
    /// Import a folder of photos into a catalog (plan §7.1).
    Import {
        /// The `.arcat` catalog to import into (created if missing).
        catalog: PathBuf,
        /// The folder to scan, recursively.
        source: PathBuf,
        /// Copy files into this folder (`{dest}/{YYYY}/{YYYY-MM-DD}/…`)
        /// instead of referencing them in place.
        #[arg(long)]
        copy_to: Option<PathBuf>,
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
        Command::Import {
            catalog,
            source,
            copy_to,
        } => {
            let mode = match copy_to {
                Some(dest) => ImportMode::Copy { dest },
                None => ImportMode::Add,
            };
            let opts = ImportOptions {
                catalog_path: catalog,
                source,
                mode,
            };
            let summary = run_import(&opts, &StdoutProgressSink)?;
            println!(
                "imported {} of {} scanned ({} duplicate{} skipped)",
                summary.imported,
                summary.scanned,
                summary.skipped_duplicates,
                if summary.skipped_duplicates == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
    }

    Ok(())
}

struct StdoutProgressSink;

impl ImportProgressSink for StdoutProgressSink {
    fn progress(&self, scanned: u32, imported: u32, total: u32) {
        println!("  {scanned}/{total} scanned, {imported} imported");
    }
}

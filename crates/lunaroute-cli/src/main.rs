//! LunaRoute CLI
//!
//! Command-line interface for managing and operating LunaRoute

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "lunaroute")]
#[command(about = "LunaRoute - Intelligent LLM API Gateway", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new LunaRoute configuration
    Init,
    /// Start the LunaRoute server
    Serve,
    /// Test routing rules
    Route,
    /// Export session data
    Export,
    /// Manage API keys
    Keys,
    /// View metrics
    Metrics,
    /// Import JSONL session logs into SQLite database
    ImportSessions {
        /// Path to JSONL sessions directory
        #[arg(long, default_value = "~/.lunaroute/sessions")]
        sessions_dir: PathBuf,

        /// Path to SQLite database
        #[arg(long, default_value = "~/.lunaroute/sessions.db")]
        db_path: PathBuf,

        /// Number of sessions to process in one batch
        #[arg(long, default_value = "10")]
        batch_size: usize,

        /// Skip sessions that already exist in database
        #[arg(long, default_value = "true")]
        skip_existing: bool,

        /// Continue importing even if some sessions fail
        #[arg(long, default_value = "true")]
        continue_on_error: bool,

        /// Show what would be imported without writing to DB
        #[arg(long, default_value = "false")]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => println!("Initializing LunaRoute..."),
        Commands::Serve => println!("Starting LunaRoute server..."),
        Commands::Route => println!("Testing routes..."),
        Commands::Export => println!("Exporting sessions..."),
        Commands::Keys => println!("Managing keys..."),
        Commands::Metrics => println!("Viewing metrics..."),
        Commands::ImportSessions {
            sessions_dir,
            db_path,
            batch_size,
            skip_existing,
            continue_on_error,
            dry_run,
        } => {
            // Expand tilde in paths
            let sessions_dir = shellexpand::tilde(&sessions_dir.to_string_lossy()).to_string();
            let db_path = shellexpand::tilde(&db_path.to_string_lossy()).to_string();

            let config = lunaroute_session::ImportConfig {
                sessions_dir: PathBuf::from(sessions_dir),
                db_path: PathBuf::from(db_path),
                batch_size,
                skip_existing,
                continue_on_error,
                dry_run,
            };

            lunaroute_session::import_sessions(config).await?;
        }
    }

    Ok(())
}

use clap::{Parser, Subcommand};
use openfragd::{AppConfig, app};
use std::{net::SocketAddr, path::PathBuf};
use tokio::net::TcpListener;

#[derive(Debug, Parser)]
#[command(name = "openfragd", version, about = "Local openfrag v1 daemon")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value = "127.0.0.1:7130")]
        bind: SocketAddr,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli { command } = Cli::parse();
    match command {
        Command::Serve { bind, data_dir } => serve(bind, data_dir).await,
    }
}

async fn serve(
    bind: SocketAddr,
    data_dir: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !bind.ip().is_loopback() {
        return Err("openfrag only binds to a loopback address".into());
    }
    let data_dir = data_dir.unwrap_or_else(default_data_directory);
    let router = app(AppConfig::new(data_dir))
        .await
        .map_err(|error| format!("failed to initialize openfrag: {error:?}"))?;
    let listener = TcpListener::bind(bind).await?;
    println!("openfrag v1 listening on http://{}", listener.local_addr()?);
    axum::serve(listener, router).await?;
    Ok(())
}

fn default_data_directory() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("openfrag")
}

use clap::{Parser, Subcommand};
use openfrag_setup::{HostFacts, RecorderInstall, SessionKind, evaluate, install_gsi};
use openfragd::{AppConfig, app};
use std::{
    ffi::OsStr,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use tokio::net::TcpListener;
use uuid::Uuid;

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
    /// Inspect local Demo import, GSI, capture, and Manual Flag compatibility.
    Doctor {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        cs2_cfg_dir: Option<PathBuf>,
    },
    /// Install openfrag's private loopback-only CS2 GSI configuration.
    SetupGsi {
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        cs2_cfg_dir: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli { command } = Cli::parse();
    match command {
        Command::Serve { bind, data_dir } => serve(bind, data_dir).await,
        Command::Doctor {
            json,
            data_dir,
            cs2_cfg_dir,
        } => doctor(json, data_dir, cs2_cfg_dir),
        Command::SetupGsi {
            data_dir,
            cs2_cfg_dir,
        } => setup_gsi(data_dir, &cs2_cfg_dir),
    }
}

fn doctor(
    json: bool,
    data_dir: Option<PathBuf>,
    cs2_cfg_dir: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = data_dir.unwrap_or_else(default_data_directory);
    let cfg_dir = cs2_cfg_dir.or_else(find_cs2_cfg_directory);
    let recorder = find_command("gpu-screen-recorder").map(RecorderInstall::Native);
    let audio_source_available = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .is_some_and(|runtime| runtime.join("pipewire-0").exists());
    let report = evaluate(&HostFacts {
        data_directory_writable: writable_directory(&data_dir),
        cs2_cfg_directory: cfg_dir,
        recorder,
        ffmpeg_available: find_command("ffmpeg").is_some(),
        audio_source_available,
        session: session_kind(),
        // Availability alone does not prove that the user approved a binding.
        global_shortcuts_portal: false,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for check in report.checks {
            println!("{:?}: {:?} - {}", check.id, check.status, check.summary);
            if !check.action.is_empty() {
                println!("  {}", check.action);
            }
        }
    }
    Ok(())
}

fn setup_gsi(
    data_dir: Option<PathBuf>,
    cs2_cfg_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = data_dir.unwrap_or_else(default_data_directory);
    fs::create_dir_all(&data_dir)?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let installation = install_gsi(cs2_cfg_dir, &data_dir, &token)
        .map_err(|error| format!("failed to install GSI configuration: {error:?}"))?;
    println!(
        "Game State Integration configured at {}",
        installation.config_path.display()
    );
    Ok(())
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

fn writable_directory(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_dir() && !metadata.permissions().readonly())
}

fn find_command(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| fs::metadata(candidate).is_ok_and(|metadata| metadata.is_file()))
}

fn find_cs2_cfg_directory() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        home.join(".local/share/Steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg"),
        home.join(".steam/steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

fn session_kind() -> SessionKind {
    match std::env::var_os("XDG_SESSION_TYPE").as_deref() {
        Some(value) if value == OsStr::new("wayland") => SessionKind::Wayland,
        Some(value) if value == OsStr::new("x11") => SessionKind::X11,
        _ => SessionKind::Other,
    }
}

use clap::{Parser, Subcommand, ValueEnum};
use openfrag_setup::{
    CaptureConfiguration, CaptureRecorder, Discovery, HostFacts, HostProbe, ProbeCommands,
    ProbeEnvironment, ProbeFilesystem, SystemProbe, evaluate, install_gsi,
    write_capture_configuration,
};
use openfragd::{AppConfig, app};
use std::{
    ffi::OsString,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    time::{Duration, Instant},
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
        #[arg(long)]
        steam_id: String,
    },
    /// Persist private replay capture settings without starting the recorder.
    SetupCapture {
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long, value_enum)]
        recorder: CaptureRecorderChoice,
        #[arg(long)]
        recorder_path: Option<PathBuf>,
        #[arg(long)]
        capture_target: String,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        ffprobe_path: PathBuf,
        #[arg(long, action = clap::ArgAction::Set, required = true)]
        enabled: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CaptureRecorderChoice {
    Native,
    Flatpak,
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
            steam_id,
        } => setup_gsi(data_dir, &cs2_cfg_dir, &steam_id),
        Command::SetupCapture {
            data_dir,
            recorder,
            recorder_path,
            capture_target,
            output_dir,
            ffprobe_path,
            enabled,
        } => setup_capture(
            data_dir,
            recorder,
            recorder_path,
            &capture_target,
            output_dir,
            ffprobe_path,
            enabled,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn setup_capture(
    data_dir: Option<PathBuf>,
    recorder: CaptureRecorderChoice,
    recorder_path: Option<PathBuf>,
    capture_target: &str,
    output_directory: PathBuf,
    ffprobe_path: PathBuf,
    enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let data_directory = data_dir.unwrap_or_else(default_data_directory);
    fs::create_dir_all(&data_directory)?;
    let recorder = match (recorder, recorder_path) {
        (CaptureRecorderChoice::Native, Some(path)) => CaptureRecorder::Native(path),
        (CaptureRecorderChoice::Native, None) => {
            return Err("--recorder-path is required for the native recorder".into());
        }
        (CaptureRecorderChoice::Flatpak, None) => CaptureRecorder::Flatpak,
        (CaptureRecorderChoice::Flatpak, Some(_)) => {
            return Err("--recorder-path is not accepted for the Flatpak recorder".into());
        }
    };
    let configuration = CaptureConfiguration::new(
        enabled,
        recorder,
        capture_target,
        output_directory,
        ffprobe_path,
    )
    .map_err(|error| format!("invalid capture configuration: {error:?}"))?;
    let installation = write_capture_configuration(&data_directory, &configuration)
        .map_err(|error| format!("failed to persist capture configuration: {error:?}"))?;
    println!(
        "Replay capture configured at {}",
        installation.path.display()
    );
    Ok(())
}

fn doctor(
    json: bool,
    data_dir: Option<PathBuf>,
    cs2_cfg_dir: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let probe = SystemProbe::new(LocalEnvironment, LocalFilesystem, HeadlessCommands);
    let mut discovered = probe.discover();
    if let Some(data_dir) = data_dir {
        discovered.data_directory = discovery_for_writable(&data_dir);
    }
    if let Some(cfg_dir) = cs2_cfg_dir {
        discovered.cs2_cfg_candidates = cfg_dir.is_dir().then_some(cfg_dir).into_iter().collect();
    }
    let report = evaluate(&HostFacts::from(&discovered));
    if json {
        let mut value = serde_json::to_value(&report)?;
        value["diagnostics"] = serde_json::json!({"ffprobe": discovery_json(&discovered.ffprobe)});
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        for check in report.checks {
            println!("{:?}: {:?} - {}", check.id, check.status, check.summary);
            if !check.action.is_empty() {
                println!("  {}", check.action);
            }
        }
        println!("Ffprobe: {}", discovery_text(&discovered.ffprobe));
    }
    Ok(())
}

fn setup_gsi(
    data_dir: Option<PathBuf>,
    cs2_cfg_dir: &Path,
    steam_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = data_dir.unwrap_or_else(default_data_directory);
    fs::create_dir_all(&data_dir)?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let installation = install_gsi(cs2_cfg_dir, &data_dir, &token, steam_id)
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
    let router = app(&AppConfig::new(data_dir))
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

struct LocalEnvironment;
impl ProbeEnvironment for LocalEnvironment {
    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }
    fn home(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}
struct LocalFilesystem;
impl ProbeFilesystem for LocalFilesystem {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn writable(&self, path: &Path) -> Result<bool, String> {
        fs::metadata(path)
            .map(|metadata| metadata.is_dir() && !metadata.permissions().readonly())
            .map_err(|error| error.to_string())
    }
    fn executable(&self, name: &str) -> bool {
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|directory| directory.join(name))
            .any(|candidate| fs::metadata(candidate).is_ok_and(|metadata| metadata.is_file()))
    }
}
struct HeadlessCommands;
impl ProbeCommands for HeadlessCommands {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        limit: usize,
        timeout: Duration,
    ) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
        let mut child = ProcessCommand::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                let output = child
                    .wait_with_output()
                    .map_err(|error| error.to_string())?;
                return Ok((
                    status.success(),
                    bounded(output.stdout, limit),
                    bounded(output.stderr, limit),
                ));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} timed out"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn bounded(mut bytes: Vec<u8>, limit: usize) -> Vec<u8> {
    bytes.truncate(limit);
    bytes
}
fn discovery_for_writable(path: &Path) -> Discovery {
    LocalFilesystem.writable(path).map_or_else(
        |error| Discovery::Unknown(format!("cannot verify {}: {error}", path.display())),
        |writable| {
            if writable {
                Discovery::Available
            } else {
                Discovery::Missing(format!("{} is not writable", path.display()))
            }
        },
    )
}
fn discovery_text(discovery: &Discovery) -> String {
    match discovery {
        Discovery::Available => "available".into(),
        Discovery::Missing(reason) => format!("missing - {reason}"),
        Discovery::Unknown(reason) => format!("unknown - {reason}"),
    }
}
fn discovery_json(discovery: &Discovery) -> serde_json::Value {
    match discovery {
        Discovery::Available => serde_json::json!({"status":"available"}),
        Discovery::Missing(reason) => serde_json::json!({"status":"missing","reason":reason}),
        Discovery::Unknown(reason) => serde_json::json!({"status":"unknown","reason":reason}),
    }
}

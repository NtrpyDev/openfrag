//! Headless, non-activating host discovery for `OpenFrag` setup.
#![allow(clippy::missing_errors_doc)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    Missing(String),
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShortcutBinding {
    Unverified,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortalFacts {
    pub service: Availability,
    pub binding: ShortcutBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostFacts {
    pub xdg_data: Availability,
    pub steam_cs2_cfg_candidates: Vec<PathBuf>,
    pub native_recorder: Availability,
    pub flatpak_recorder: Availability,
    pub ffmpeg: Availability,
    pub audio_source: Availability,
    pub session_type: Option<String>,
    pub global_shortcuts_portal: PortalFacts,
}

/// The injectable setup boundary; callers never need to invoke desktop APIs directly.
pub trait HostProbe {
    fn inspect(&self) -> HostFacts;
}
pub trait Environment {
    fn var(&self, name: &str) -> Option<OsString>;
    fn home_dir(&self) -> Option<PathBuf>;
}
pub trait Filesystem {
    fn writable_directory(&self, path: &Path) -> Result<bool, String>;
    fn exists(&self, path: &Path) -> bool;
    fn executable_in_path(&self, name: &str) -> bool;
}
pub trait CommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        limit: usize,
        timeout: Duration,
    ) -> Result<CommandOutput, String>;
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub struct SystemProbe<E, F, C> {
    environment: E,
    filesystem: F,
    commands: C,
}
impl<E, F, C> SystemProbe<E, F, C> {
    #[must_use]
    pub fn new(environment: E, filesystem: F, commands: C) -> Self {
        Self {
            environment,
            filesystem,
            commands,
        }
    }
}

impl<E: Environment, F: Filesystem, C: CommandRunner> HostProbe for SystemProbe<E, F, C> {
    fn inspect(&self) -> HostFacts {
        let home = self.environment.home_dir();
        let data = self
            .environment
            .var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|value| value.join(".local/share")));
        let xdg_data = match data.as_deref() {
            Some(path) => self.filesystem.writable_directory(path).map_or_else(
                |error| Availability::Unknown(format!("cannot verify {}: {error}", path.display())),
                |ok| {
                    if ok {
                        Availability::Available
                    } else {
                        Availability::Missing(format!("{} is not writable", path.display()))
                    }
                },
            ),
            None => Availability::Missing("HOME and XDG_DATA_HOME are unavailable".to_owned()),
        };
        let steam_cs2_cfg_candidates = steam_candidates(home.as_deref(), data.as_deref());
        let native_recorder = executable(&self.filesystem, "gpu-screen-recorder");
        let ffmpeg = executable(&self.filesystem, "ffmpeg");
        let flatpak_recorder = command_availability(
            &self.commands,
            "flatpak",
            &[
                "info",
                "--show-application",
                "com.dec05eba.gpu_screen_recorder",
            ],
            "gpu-screen-recorder Flatpak",
        );
        let audio_source = audio_availability(&self.commands);
        let session_type = self
            .environment
            .var("XDG_SESSION_TYPE")
            .and_then(|value| value.into_string().ok())
            .filter(|value| !value.is_empty());
        let portal = portal_availability(&self.commands);
        HostFacts {
            xdg_data,
            steam_cs2_cfg_candidates,
            native_recorder,
            flatpak_recorder,
            ffmpeg,
            audio_source,
            session_type,
            global_shortcuts_portal: PortalFacts {
                service: portal,
                binding: ShortcutBinding::Unverified,
            },
        }
    }
}

fn steam_candidates(home: Option<&Path>, data: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(data) = data {
        candidates.push(
            data.join("Steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg"),
        );
    }
    if let Some(home) = home {
        candidates.push(
            home.join(
                ".steam/steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg",
            ),
        );
        candidates.push(home.join(".var/app/com.valvesoftware.Steam/data/Steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg"));
    }
    candidates.sort();
    candidates.dedup();
    candidates
}
fn executable(filesystem: &impl Filesystem, name: &str) -> Availability {
    if filesystem.executable_in_path(name) {
        Availability::Available
    } else {
        Availability::Missing(format!("{name} is not available on PATH"))
    }
}
fn command_availability(
    commands: &impl CommandRunner,
    program: &str,
    args: &[&str],
    label: &str,
) -> Availability {
    match commands.run(program, args, 8192, Duration::from_secs(1)) {
        Ok(output) if output.success => Availability::Available,
        Ok(_) => Availability::Missing(format!("{label} is not installed")),
        Err(error) => Availability::Unknown(format!("cannot inspect {label}: {error}")),
    }
}
fn audio_availability(commands: &impl CommandRunner) -> Availability {
    let pipewire = commands.run("pw-cli", &["info", "0"], 8192, Duration::from_secs(1));
    if matches!(pipewire, Ok(CommandOutput { success: true, .. })) {
        return Availability::Available;
    }
    match commands.run("pactl", &["info"], 8192, Duration::from_secs(1)) {
        Ok(output) if output.success => Availability::Available,
        Ok(_) => Availability::Missing("no PipeWire or PulseAudio source is available".to_owned()),
        Err(error) => Availability::Unknown(format!("cannot inspect PipeWire/PulseAudio: {error}")),
    }
}
fn portal_availability(commands: &impl CommandRunner) -> Availability {
    // `busctl list` observes names only; it does not call CreateSession or activate a portal UI.
    match commands.run(
        "busctl",
        &["--user", "--no-legend", "list"],
        64 * 1024,
        Duration::from_secs(1),
    ) {
        Ok(output)
            if output.success
                && String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                    line.split_whitespace().next() == Some("org.freedesktop.portal.Desktop")
                }) =>
        {
            Availability::Available
        }
        Ok(output) if output.success => {
            Availability::Missing("XDG GlobalShortcuts portal service is not registered".to_owned())
        }
        Ok(_) => {
            Availability::Unknown("cannot list user D-Bus names without activation".to_owned())
        }
        Err(error) => Availability::Unknown(format!(
            "cannot inspect user D-Bus without activation: {error}"
        )),
    }
}

#[derive(Default)]
pub struct StdEnvironment;
impl Environment for StdEnvironment {
    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }
    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}
#[derive(Default)]
pub struct StdFilesystem;
impl Filesystem for StdFilesystem {
    fn writable_directory(&self, path: &Path) -> Result<bool, String> {
        if !path.is_dir() {
            return Ok(false);
        }
        let probe = path.join(format!(".openfrag-write-probe-{}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(_) => {
                std::fs::remove_file(probe).map_err(|error| error.to_string())?;
                Ok(true)
            }
            Err(error) => Ok(error.kind() != std::io::ErrorKind::PermissionDenied),
        }
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn executable_in_path(&self, name: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|path| self.exists(&path.join(name)))
        })
    }
}
#[derive(Default)]
pub struct StdCommandRunner;
impl CommandRunner for StdCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        limit: usize,
        timeout: Duration,
    ) -> Result<CommandOutput, String> {
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let deadline = Instant::now() + timeout;
        while child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            if Instant::now() >= deadline {
                child.kill().map_err(|error| error.to_string())?;
                let _ = child.wait();
                return Err(format!("{program} timed out"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut output = child
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        if output.stdout.len() > limit || output.stderr.len() > limit {
            return Err(format!("{program} output exceeded {limit} bytes"));
        }
        output.stdout.truncate(limit);
        output.stderr.truncate(limit);
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct FakeEnvironment {
        vars: BTreeMap<String, OsString>,
        home: Option<PathBuf>,
    }
    impl Environment for FakeEnvironment {
        fn var(&self, name: &str) -> Option<OsString> {
            self.vars.get(name).cloned()
        }
        fn home_dir(&self) -> Option<PathBuf> {
            self.home.clone()
        }
    }
    #[derive(Default)]
    struct FakeFilesystem {
        writable: bool,
        executables: Vec<String>,
    }
    impl Filesystem for FakeFilesystem {
        fn writable_directory(&self, _: &Path) -> Result<bool, String> {
            Ok(self.writable)
        }
        fn exists(&self, _: &Path) -> bool {
            true
        }
        fn executable_in_path(&self, name: &str) -> bool {
            self.executables.iter().any(|value| value == name)
        }
    }
    #[derive(Default)]
    struct FakeCommands {
        responses: RefCell<BTreeMap<String, Result<CommandOutput, String>>>,
    }
    impl CommandRunner for FakeCommands {
        fn run(
            &self,
            program: &str,
            _: &[&str],
            _: usize,
            _: Duration,
        ) -> Result<CommandOutput, String> {
            self.responses
                .borrow()
                .get(program)
                .cloned()
                .unwrap_or_else(|| Err("not installed".to_owned()))
        }
    }
    fn success(stdout: &str) -> Result<CommandOutput, String> {
        Ok(CommandOutput {
            success: true,
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
        })
    }
    #[test]
    fn native_host_has_actionable_facts_and_unverified_manual_flag() {
        let root = temporary_directory();
        let mut environment = FakeEnvironment {
            home: Some(root.clone()),
            ..Default::default()
        };
        environment.vars.insert(
            "XDG_DATA_HOME".to_owned(),
            root.join("data").into_os_string(),
        );
        environment
            .vars
            .insert("XDG_SESSION_TYPE".to_owned(), "wayland".into());
        let commands = FakeCommands::default();
        commands
            .responses
            .borrow_mut()
            .insert("pw-cli".to_owned(), success("PipeWire"));
        commands.responses.borrow_mut().insert(
            "busctl".to_owned(),
            success("org.freedesktop.portal.Desktop 1\n"),
        );
        commands
            .responses
            .borrow_mut()
            .insert("flatpak".to_owned(), Err("missing".to_owned()));
        let facts = SystemProbe::new(
            environment,
            FakeFilesystem {
                writable: true,
                executables: vec!["gpu-screen-recorder".to_owned(), "ffmpeg".to_owned()],
            },
            commands,
        )
        .inspect();
        assert_eq!(facts.xdg_data, Availability::Available);
        assert_eq!(facts.native_recorder, Availability::Available);
        assert_eq!(
            facts.flatpak_recorder,
            Availability::Unknown("cannot inspect gpu-screen-recorder Flatpak: missing".to_owned())
        );
        assert_eq!(facts.session_type.as_deref(), Some("wayland"));
        assert_eq!(
            facts.global_shortcuts_portal.service,
            Availability::Available
        );
        assert_eq!(
            facts.global_shortcuts_portal.binding,
            ShortcutBinding::Unverified
        );
        assert_eq!(facts.steam_cs2_cfg_candidates.len(), 3);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn flatpak_and_missing_dependencies_remain_actionable() {
        let root = temporary_directory();
        let environment = FakeEnvironment {
            home: Some(root.clone()),
            ..Default::default()
        };
        let commands = FakeCommands::default();
        commands.responses.borrow_mut().insert(
            "flatpak".to_owned(),
            success("com.dec05eba.gpu_screen_recorder\n"),
        );
        commands.responses.borrow_mut().insert(
            "pw-cli".to_owned(),
            Ok(CommandOutput {
                success: false,
                stdout: vec![],
                stderr: vec![],
            }),
        );
        commands.responses.borrow_mut().insert(
            "pactl".to_owned(),
            Ok(CommandOutput {
                success: false,
                stdout: vec![],
                stderr: vec![],
            }),
        );
        commands
            .responses
            .borrow_mut()
            .insert("busctl".to_owned(), success("org.example.Other 1\n"));
        let facts = SystemProbe::new(
            environment,
            FakeFilesystem {
                writable: true,
                ..Default::default()
            },
            commands,
        )
        .inspect();
        assert!(matches!(facts.xdg_data, Availability::Available));
        assert_eq!(facts.flatpak_recorder, Availability::Available);
        assert!(matches!(facts.native_recorder, Availability::Missing(_)));
        assert!(matches!(facts.ffmpeg, Availability::Missing(_)));
        assert!(matches!(facts.audio_source, Availability::Missing(_)));
        assert!(matches!(
            facts.global_shortcuts_portal.service,
            Availability::Missing(_)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
    fn temporary_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!("openfrag-setup-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

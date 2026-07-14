//! A testable supervisor for a `gpu-screen-recorder` capture process.
#![allow(clippy::missing_errors_doc, clippy::struct_excessive_bools)]

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// How the recorder is launched. Arguments are always passed directly, never to a shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Launch {
    Native {
        program: OsString,
        args: Vec<OsString>,
    },
    Flatpak {
        app_id: OsString,
        args: Vec<OsString>,
    },
}

impl Launch {
    #[must_use]
    pub fn argv(&self) -> Vec<OsString> {
        match self {
            Self::Native { program, args } => std::iter::once(program.clone())
                .chain(args.clone())
                .collect(),
            Self::Flatpak { app_id, args } => [
                OsString::from("flatpak"),
                OsString::from("run"),
                app_id.clone(),
            ]
            .into_iter()
            .chain(args.clone())
            .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveProvenance {
    ManualFlag,
    AutoRoundEnd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    User1,
    Interrupt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaInfo {
    pub duration_ms: u64,
    pub video_streams: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveAcknowledgement {
    pub path: PathBuf,
    pub provenance: SaveProvenance,
    pub media: MediaInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveDisposition {
    Signalled,
    Coalesced,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Process(String),
    Filesystem(String),
    Media(String),
    NotRunning,
    NoSaveInFlight,
    UnsafeOutputPath,
    SymlinkOutput,
    InvalidOutput,
    OutputNotFound,
}

pub trait Process {
    fn spawn(&mut self, argv: &[OsString]) -> Result<(), String>;
    fn try_wait(&mut self) -> Result<Option<i32>, String>;
    fn signal(&mut self, signal: Signal) -> Result<(), String>;
    /// Returns stderr bytes accumulated since the previous call.
    fn drain_stderr(&mut self) -> Result<Vec<u8>, String>;
}

pub trait Filesystem {
    fn outputs_since(&self, directory: &Path, since_ms: u64) -> Result<Vec<PathBuf>, String>;
    fn is_symlink(&self, path: &Path) -> Result<bool, String>;
    fn is_regular_file(&self, path: &Path) -> Result<bool, String>;
}

pub trait Clock {
    fn now_ms(&self) -> u64;
}
pub trait MediaProbe {
    fn verify(&self, path: &Path) -> Result<MediaInfo, String>;
}

pub struct Config {
    pub launch: Launch,
    pub output_directory: PathBuf,
    pub max_stderr_bytes: usize,
    pub stable_after: Duration,
}

impl Config {
    #[must_use]
    pub fn new(launch: Launch, output_directory: PathBuf) -> Self {
        Self {
            launch,
            output_directory,
            max_stderr_bytes: 16 * 1024,
            stable_after: Duration::from_secs(30),
        }
    }
}

struct InFlight {
    provenance: SaveProvenance,
    since_ms: u64,
}

/// Coordinates one recorder child and one durable save acknowledgement at a time.
pub struct Supervisor<P, F, C, M> {
    process: P,
    filesystem: F,
    clock: C,
    probe: M,
    config: Config,
    running: bool,
    shutting_down: bool,
    started_ms: u64,
    stable: bool,
    in_flight: Option<InFlight>,
    queued: Option<SaveProvenance>,
    next_restart_ms: Option<u64>,
    failures: usize,
    stderr: VecDeque<u8>,
}

impl<P: Process, F: Filesystem, C: Clock, M: MediaProbe> Supervisor<P, F, C, M> {
    #[must_use]
    pub fn new(process: P, filesystem: F, clock: C, probe: M, config: Config) -> Self {
        Self {
            process,
            filesystem,
            clock,
            probe,
            config,
            running: false,
            shutting_down: false,
            started_ms: 0,
            stable: false,
            in_flight: None,
            queued: None,
            next_restart_ms: None,
            failures: 0,
            stderr: VecDeque::new(),
        }
    }

    pub fn start(&mut self) -> Result<(), Error> {
        self.spawn()
    }
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }
    #[must_use]
    pub fn next_restart_ms(&self) -> Option<u64> {
        self.next_restart_ms
    }
    #[must_use]
    pub fn stderr_tail(&self) -> Vec<u8> {
        self.stderr.iter().copied().collect()
    }

    pub fn request_save(&mut self, provenance: SaveProvenance) -> Result<SaveDisposition, Error> {
        if !self.running {
            return Err(Error::NotRunning);
        }
        if self.in_flight.is_some() {
            if self.queued.is_none() {
                self.queued = Some(provenance);
            }
            return Ok(SaveDisposition::Coalesced);
        }
        self.process.signal(Signal::User1).map_err(Error::Process)?;
        self.in_flight = Some(InFlight {
            provenance,
            since_ms: self.clock.now_ms(),
        });
        Ok(SaveDisposition::Signalled)
    }

    /// Discover and verify a newly emitted file before acknowledging the save.
    pub fn discover_save(&mut self) -> Result<Option<SaveAcknowledgement>, Error> {
        let Some(in_flight) = &self.in_flight else {
            return Err(Error::NoSaveInFlight);
        };
        let candidates = self
            .filesystem
            .outputs_since(&self.config.output_directory, in_flight.since_ms)
            .map_err(Error::Filesystem)?;
        let Some(path) = candidates.into_iter().next() else {
            return Ok(None);
        };
        self.acknowledge(path).map(Some)
    }

    pub fn acknowledge(&mut self, path: PathBuf) -> Result<SaveAcknowledgement, Error> {
        let provenance = self
            .in_flight
            .as_ref()
            .ok_or(Error::NoSaveInFlight)?
            .provenance;
        self.validate_output(&path)?;
        let media = self.probe.verify(&path).map_err(Error::Media)?;
        let acknowledgement = SaveAcknowledgement {
            path,
            provenance,
            media,
        };
        self.in_flight = None;
        if let Some(next) = self.queued.take() {
            self.request_save(next)?;
        }
        Ok(acknowledgement)
    }

    /// Polls exit/stderr state and starts a due restart. Returns the detected exit code.
    pub fn poll(&mut self) -> Result<Option<i32>, Error> {
        self.append_stderr()?;
        if self.running {
            if !self.stable
                && self.clock.now_ms().saturating_sub(self.started_ms)
                    >= duration_ms(self.config.stable_after)
            {
                self.failures = 0;
                self.stable = true;
            }
            if let Some(code) = self.process.try_wait().map_err(Error::Process)? {
                self.running = false;
                if !self.shutting_down {
                    let delay = restart_delay(self.failures);
                    self.failures = self.failures.saturating_add(1);
                    self.next_restart_ms =
                        Some(self.clock.now_ms().saturating_add(duration_ms(delay)));
                }
                return Ok(Some(code));
            }
        }
        if !self.shutting_down
            && !self.running
            && self
                .next_restart_ms
                .is_some_and(|at| self.clock.now_ms() >= at)
        {
            self.spawn()?;
        }
        Ok(None)
    }

    pub fn shutdown(&mut self) -> Result<(), Error> {
        self.shutting_down = true;
        self.next_restart_ms = None;
        if self.running {
            self.process
                .signal(Signal::Interrupt)
                .map_err(Error::Process)?;
        }
        Ok(())
    }

    fn spawn(&mut self) -> Result<(), Error> {
        self.process
            .spawn(&self.config.launch.argv())
            .map_err(Error::Process)?;
        self.running = true;
        self.shutting_down = false;
        self.stable = false;
        self.started_ms = self.clock.now_ms();
        self.next_restart_ms = None;
        Ok(())
    }
    fn append_stderr(&mut self) -> Result<(), Error> {
        for byte in self.process.drain_stderr().map_err(Error::Process)? {
            self.stderr.push_back(byte);
            if self.stderr.len() > self.config.max_stderr_bytes {
                self.stderr.pop_front();
            }
        }
        Ok(())
    }
    fn validate_output(&self, path: &Path) -> Result<(), Error> {
        if !path.starts_with(&self.config.output_directory)
            || path.components().any(|c| matches!(c, Component::ParentDir))
        {
            return Err(Error::UnsafeOutputPath);
        }
        if self
            .filesystem
            .is_symlink(path)
            .map_err(Error::Filesystem)?
        {
            return Err(Error::SymlinkOutput);
        }
        if !self
            .filesystem
            .is_regular_file(path)
            .map_err(Error::Filesystem)?
        {
            return Err(Error::InvalidOutput);
        }
        Ok(())
    }
}

#[must_use]
pub fn restart_delay(failures: usize) -> Duration {
    Duration::from_secs([1, 2, 4, 8, 16, 30][failures.min(5)])
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// A concrete adapter using `std::process::Command`; it does not invoke a shell.
#[derive(Default)]
pub struct StdProcess {
    child: Option<std::process::Child>,
    stderr: Option<Receiver<Vec<u8>>>,
}
impl Process for StdProcess {
    fn spawn(&mut self, argv: &[OsString]) -> Result<(), String> {
        let (program, command_args) = argv.split_first().ok_or_else(|| "empty argv".to_owned())?;
        let mut child = std::process::Command::new(program)
            .args(command_args)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let (sender, receiver) = mpsc::channel();
        if let Some(mut stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                loop {
                    let mut chunk = [0; 4096];
                    match stderr.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(length) if sender.send(chunk[..length].to_vec()).is_err() => break,
                        Ok(_) => {}
                    }
                }
            });
        }
        self.child = Some(child);
        self.stderr = Some(receiver);
        Ok(())
    }
    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        let Some(child) = &mut self.child else {
            return Ok(None);
        };
        child
            .try_wait()
            .map(|status| status.map(|s| s.code().unwrap_or(-1)))
            .map_err(|e| e.to_string())
    }
    fn signal(&mut self, signal: Signal) -> Result<(), String> {
        let child = self.child.as_ref().ok_or_else(|| "no child".to_owned())?;
        let signal = match signal {
            Signal::User1 => nix::sys::signal::Signal::SIGUSR1,
            Signal::Interrupt => nix::sys::signal::Signal::SIGINT,
        };
        let pid = i32::try_from(child.id()).map_err(|_| "child pid does not fit i32".to_owned())?;
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal).map_err(|e| e.to_string())
    }
    fn drain_stderr(&mut self) -> Result<Vec<u8>, String> {
        Ok(self
            .stderr
            .as_ref()
            .map_or_else(Vec::new, |receiver| receiver.try_iter().flatten().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeProcess {
        spawns: Vec<Vec<OsString>>,
        signals: Vec<Signal>,
        exits: Vec<Option<i32>>,
        stderr: Vec<u8>,
    }
    impl Process for FakeProcess {
        fn spawn(&mut self, argv: &[OsString]) -> Result<(), String> {
            self.spawns.push(argv.to_vec());
            Ok(())
        }
        fn try_wait(&mut self) -> Result<Option<i32>, String> {
            Ok(if self.exits.is_empty() {
                None
            } else {
                self.exits.remove(0)
            })
        }
        fn signal(&mut self, signal: Signal) -> Result<(), String> {
            self.signals.push(signal);
            Ok(())
        }
        fn drain_stderr(&mut self) -> Result<Vec<u8>, String> {
            Ok(std::mem::take(&mut self.stderr))
        }
    }
    #[derive(Default)]
    struct FakeFs {
        outputs: Vec<PathBuf>,
        symlink: bool,
        regular: bool,
    }
    impl Filesystem for FakeFs {
        fn outputs_since(&self, _: &Path, _: u64) -> Result<Vec<PathBuf>, String> {
            Ok(self.outputs.clone())
        }
        fn is_symlink(&self, _: &Path) -> Result<bool, String> {
            Ok(self.symlink)
        }
        fn is_regular_file(&self, _: &Path) -> Result<bool, String> {
            Ok(self.regular)
        }
    }
    struct FakeClock(Cell<u64>);
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.get()
        }
    }
    struct FakeProbe;
    impl MediaProbe for FakeProbe {
        fn verify(&self, _: &Path) -> Result<MediaInfo, String> {
            Ok(MediaInfo {
                duration_ms: 100,
                video_streams: 1,
            })
        }
    }
    fn supervisor(clock: u64) -> Supervisor<FakeProcess, FakeFs, FakeClock, FakeProbe> {
        let mut config = Config::new(
            Launch::Native {
                program: "gpu-screen-recorder".into(),
                args: vec!["-w".into(), "screen".into()],
            },
            "/clips".into(),
        );
        config.max_stderr_bytes = 3;
        config.stable_after = Duration::from_secs(10);
        Supervisor::new(
            FakeProcess::default(),
            FakeFs {
                outputs: vec![],
                symlink: false,
                regular: true,
            },
            FakeClock(Cell::new(clock)),
            FakeProbe,
            config,
        )
    }

    #[test]
    fn argv_is_direct_for_native_and_flatpak() {
        assert_eq!(
            Launch::Native {
                program: "recorder".into(),
                args: vec![";rm".into()]
            }
            .argv(),
            vec![OsString::from("recorder"), OsString::from(";rm")]
        );
        assert_eq!(
            Launch::Flatpak {
                app_id: "com.example.Recorder".into(),
                args: vec!["--arg".into()]
            }
            .argv(),
            vec!["flatpak", "run", "com.example.Recorder", "--arg"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn only_one_follow_up_is_coalesced_and_provenance_survives() {
        let mut s = supervisor(5);
        s.start().unwrap();
        assert_eq!(
            s.request_save(SaveProvenance::ManualFlag),
            Ok(SaveDisposition::Signalled)
        );
        assert_eq!(
            s.request_save(SaveProvenance::AutoRoundEnd),
            Ok(SaveDisposition::Coalesced)
        );
        assert_eq!(
            s.request_save(SaveProvenance::ManualFlag),
            Ok(SaveDisposition::Coalesced)
        );
        let ack = s.acknowledge("/clips/one.mp4".into()).unwrap();
        assert_eq!(ack.provenance, SaveProvenance::ManualFlag);
        assert_eq!(s.process.signals, vec![Signal::User1, Signal::User1]);
        let follow_up = s.acknowledge("/clips/two.mp4".into()).unwrap();
        assert_eq!(follow_up.provenance, SaveProvenance::AutoRoundEnd);
    }
    #[test]
    fn rejects_escape_symlink_and_unverified_media() {
        let mut s = supervisor(0);
        s.start().unwrap();
        s.request_save(SaveProvenance::ManualFlag).unwrap();
        assert_eq!(
            s.acknowledge("/clips/../outside.mp4".into()),
            Err(Error::UnsafeOutputPath)
        );
        s.filesystem.symlink = true;
        assert_eq!(
            s.acknowledge("/clips/link.mp4".into()),
            Err(Error::SymlinkOutput)
        );
    }
    #[test]
    fn discovers_only_after_a_save_and_verifies_before_acknowledging() {
        let mut s = supervisor(0);
        s.start().unwrap();
        assert_eq!(s.discover_save(), Err(Error::NoSaveInFlight));
        s.request_save(SaveProvenance::AutoRoundEnd).unwrap();
        s.filesystem.outputs = vec!["/clips/final.mp4".into()];
        assert_eq!(
            s.discover_save().unwrap().unwrap().path,
            PathBuf::from("/clips/final.mp4")
        );
    }
    #[test]
    fn stderr_is_bounded_and_exit_uses_capped_backoff_then_stable_reset() {
        let mut s = supervisor(0);
        s.start().unwrap();
        s.process.stderr = b"abcdef".to_vec();
        s.process.exits = vec![Some(9)];
        assert_eq!(s.poll(), Ok(Some(9)));
        assert_eq!(s.stderr_tail(), b"def");
        assert_eq!(s.next_restart_ms(), Some(1000));
        s.clock.0.set(1000);
        s.poll().unwrap();
        s.process.exits = vec![Some(1)];
        s.poll().unwrap();
        assert_eq!(s.next_restart_ms(), Some(3000));
        s.clock.0.set(3000);
        s.poll().unwrap();
        s.clock.0.set(13_000);
        s.poll().unwrap();
        s.process.exits = vec![Some(1)];
        s.poll().unwrap();
        assert_eq!(s.next_restart_ms(), Some(14_000));
        assert_eq!(restart_delay(99), Duration::from_secs(30));
    }
    #[test]
    fn shutdown_sends_interrupt_and_never_restarts() {
        let mut s = supervisor(0);
        s.start().unwrap();
        s.shutdown().unwrap();
        s.process.exits = vec![Some(0)];
        assert_eq!(s.poll(), Ok(Some(0)));
        assert_eq!(s.process.signals, vec![Signal::Interrupt]);
        assert_eq!(s.next_restart_ms(), None);
    }
}

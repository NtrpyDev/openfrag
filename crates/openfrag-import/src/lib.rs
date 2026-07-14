//! Durable local Demo import primitives. This crate owns orchestration, not parser output.

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    time::{Duration, SystemTime},
};

pub const MAX_DEMO_BYTES: u64 = 2_147_483_648;
pub const CHUNK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    AwaitingImport,
    Validating,
    Hashing,
    Deduplicating,
    Copying,
    Parsing,
    Rating,
    LinkingClips,
    Ready,
    Error(ErrorCode),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Io,
    Corrupt,
    Unsupported,
    Size,
    Parse,
    Analysis,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub processed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub indeterminate: bool,
    pub work_done: u64,
    pub heartbeat: u64,
}
impl Progress {
    pub fn fraction_millionths(&self) -> Option<u64> {
        self.total_bytes
            .filter(|&t| t > 0)
            .map(|t| (self.processed_bytes.min(t) * 1_000_000) / t)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalculationIdentity {
    pub source_sha256: String,
    pub parser_commit: String,
    pub parser_build: String,
    pub generated_proto_build: String,
    pub requested_schema_hash: String,
    pub metric_definition_version: String,
    pub formula_version: String,
    pub evidence_semantics_epoch: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub id: u64,
    pub state: State,
    pub retries: u8,
    pub next_retry_after: Option<Duration>,
    pub progress: Progress,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<SystemTime>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DedupDecision {
    Copy,
    Ready,
    Resume,
    Parse,
    Rate,
    PriorFailure,
}

pub trait ParserAdapter {
    type Parsed;
    type Error: std::fmt::Display;
    fn parse(
        &self,
        demo: &Path,
        progress: &mut dyn FnMut(u64),
    ) -> Result<Self::Parsed, Self::Error>;
    fn rate(&self, parsed: &Self::Parsed) -> Result<(), Self::Error>;
}

pub fn validate(path: &Path, free_bytes: u64) -> Result<u64, ErrorCode> {
    let meta = fs::metadata(path).map_err(|_| ErrorCode::Io)?;
    if !meta.is_file() {
        return Err(ErrorCode::Corrupt);
    }
    if meta.len() > MAX_DEMO_BYTES {
        return Err(ErrorCode::Size);
    }
    if free_bytes < meta.len() + meta.len() / 10 {
        return Err(ErrorCode::Io);
    }
    if meta.len() < 8 {
        return Err(ErrorCode::Corrupt);
    }
    Ok(meta.len())
}

pub fn hash_and_copy(
    src: &Path,
    dst: &Path,
    total: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<String, ErrorCode> {
    let mut input = fs::File::open(src).map_err(|_| ErrorCode::Io)?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|_| ErrorCode::Io)?;
    }
    let tmp = dst.with_extension(format!("staging-{}", std::process::id()));
    let mut output = fs::File::create(&tmp).map_err(|_| ErrorCode::Io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut done = 0;
    let result = (|| {
        loop {
            let n = input.read(&mut buf).map_err(|_| ErrorCode::Io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            output.write_all(&buf[..n]).map_err(|_| ErrorCode::Io)?;
            done += n as u64;
            progress(Progress {
                processed_bytes: done,
                total_bytes: Some(total),
                indeterminate: false,
                work_done: 0,
                heartbeat: done,
            });
        }
        output.sync_all().map_err(|_| ErrorCode::Io)?;
        fs::rename(&tmp, dst).map_err(|_| ErrorCode::Io)?;
        Ok(format!("{:x}", hasher.finalize()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn retry_delay(automatic_retry: u8) -> Option<Duration> {
    match automatic_retry {
        0 => Some(Duration::from_secs(2)),
        1 => Some(Duration::from_secs(10)),
        2 => Some(Duration::from_secs(60)),
        _ => None,
    }
}

pub fn lease_available(attempt: &Attempt, now: SystemTime, owner: &str) -> bool {
    attempt
        .lease_owner
        .as_deref()
        .map(|x| x == owner)
        .unwrap_or(true)
        || attempt.lease_expires_at.map(|x| x <= now).unwrap_or(true)
}

pub fn choose_dedup(
    artifact_exists: bool,
    run_succeeded: bool,
    run_running: bool,
    parser_changed: bool,
    formula_changed: bool,
    prior_failed: bool,
) -> DedupDecision {
    if !artifact_exists {
        DedupDecision::Copy
    } else if run_succeeded && !parser_changed && !formula_changed {
        DedupDecision::Ready
    } else if run_running {
        DedupDecision::Resume
    } else if parser_changed {
        DedupDecision::Parse
    } else if formula_changed {
        DedupDecision::Rate
    } else if prior_failed {
        DedupDecision::PriorFailure
    } else {
        DedupDecision::Parse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    struct Fake;
    impl ParserAdapter for Fake {
        type Parsed = u8;
        type Error = &'static str;
        fn parse(&self, _: &Path, p: &mut dyn FnMut(u64)) -> Result<u8, Self::Error> {
            p(1);
            Ok(1)
        }
        fn rate(&self, _: &u8) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    #[test]
    fn validates_limit_and_headroom() {
        let d = tempdir().unwrap();
        let p = d.path().join("x.dem");
        fs::write(&p, b"12345678").unwrap();
        assert_eq!(validate(&p, 8), Err(ErrorCode::Io));
        assert_eq!(validate(&p, 9), Ok(8));
    }
    #[test]
    fn hashes_and_copies_atomically() {
        let d = tempdir().unwrap();
        let s = d.path().join("a.dem");
        let t = d.path().join("store/a.dem");
        fs::write(&s, b"demo bytes").unwrap();
        let mut seen = Vec::new();
        let h = hash_and_copy(&s, &t, 10, &mut |p| seen.push(p)).unwrap();
        assert!(t.is_file());
        assert_eq!(seen.last().unwrap().fraction_millionths(), Some(1_000_000));
        assert_eq!(h.len(), 64);
    }
    #[test]
    fn retries_are_exact() {
        assert_eq!(retry_delay(0), Some(Duration::from_secs(2)));
        assert_eq!(retry_delay(1), Some(Duration::from_secs(10)));
        assert_eq!(retry_delay(2), Some(Duration::from_secs(60)));
        assert_eq!(retry_delay(3), None);
    }
    #[test]
    fn dedup_branches_are_deterministic() {
        assert_eq!(
            choose_dedup(false, false, false, false, false, false),
            DedupDecision::Copy
        );
        assert_eq!(
            choose_dedup(true, true, false, false, false, false),
            DedupDecision::Ready
        );
        assert_eq!(
            choose_dedup(true, false, true, false, false, false),
            DedupDecision::Resume
        );
        assert_eq!(
            choose_dedup(true, false, false, true, false, false),
            DedupDecision::Parse
        );
        assert_eq!(
            choose_dedup(true, false, false, false, true, false),
            DedupDecision::Rate
        );
    }
    #[test]
    fn fake_adapter_is_not_production_parser() {
        let d = tempdir().unwrap();
        let p = d.path().join("x");
        fs::write(&p, b"x").unwrap();
        let f = Fake;
        let mut n = 0;
        let parsed = f.parse(&p, &mut |x| n = x).unwrap();
        f.rate(&parsed).unwrap();
        assert_eq!(n, 1);
    }
}

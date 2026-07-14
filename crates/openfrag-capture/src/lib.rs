use std::time::Duration;

pub const REPLAY_SECONDS: u32 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
}

pub fn replay_spec(output_dir: &str) -> LaunchSpec {
    LaunchSpec {
        program: "gpu-screen-recorder".into(),
        args: vec![
            "-w".into(),
            "screen".into(),
            "-r".into(),
            REPLAY_SECONDS.to_string(),
            "-c".into(),
            "mp4".into(),
            "-o".into(),
            output_dir.into(),
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    SaveReplay,
    Stop,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRequest {
    pub id: u64,
    pub signal: Signal,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ack {
    Path(String),
    Timeout,
    Exited,
}

#[derive(Debug, Default)]
pub struct Supervisor {
    next_id: u64,
    pending: Option<SaveRequest>,
}
impl Supervisor {
    pub fn request_save(&mut self) -> SaveRequest {
        self.next_id += 1;
        let request = SaveRequest {
            id: self.next_id,
            signal: Signal::SaveReplay,
        };
        self.pending = Some(request.clone());
        request
    }
    pub fn acknowledge(&mut self, path: String) -> Option<(u64, String)> {
        self.pending.take().map(|r| (r.id, path))
    }
    pub fn timeout(&mut self, _after: Duration) -> Option<u64> {
        self.pending.take().map(|r| r.id)
    }
}

pub fn parse_stdout_line(line: &str) -> Option<String> {
    let path = line.trim();
    (!path.is_empty() && !path.starts_with('[') && !path.contains('\0')).then(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spec_has_no_shell() {
        let s = replay_spec("/tmp/clips");
        assert_eq!(s.args[3], "60");
        assert!(!s.args.join(" ").contains("&&"));
    }
    #[test]
    fn saves_serialize() {
        let mut s = Supervisor::default();
        let a = s.request_save();
        let b = s.request_save();
        assert_eq!(a.id + 1, b.id);
        assert_eq!(
            s.acknowledge("/tmp/a.mp4".to_string()),
            Some((b.id, "/tmp/a.mp4".into()))
        );
    }
}

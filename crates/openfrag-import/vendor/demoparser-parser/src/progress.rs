#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParsePhase {
    FirstPass,
    SecondPass,
    Finalize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseProgress {
    pub phase: ParsePhase,
    pub bytes_consumed: u64,
    pub total_bytes: u64,
    pub frames: u64,
    pub events_emitted: u64,
}

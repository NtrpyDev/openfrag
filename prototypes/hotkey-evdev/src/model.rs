//! PROTOTYPE ONLY. Pure broker state used by both the live spike and simulator.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub serial: String,
    pub interface: String,
    pub name: String,
    pub input_id: String,
    pub physical_path: String,
    pub unique_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub event_path: String,
    pub fingerprint: Fingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    AwaitingDevice,
    Armed { event_path: String },
    Ambiguous { matches: usize },
    IdentityChanged,
    PermissionDenied(String),
    Disconnected(String),
    Suspended,
    Recovering { event_path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientState {
    Disconnected,
    Authorized { uid: u32 },
    Rejected { uid: u32 },
}

#[derive(Debug)]
pub struct BrokerModel {
    pub binding: u16,
    pub minimum_interval_ms: u128,
    pub authorized_device: Option<Fingerprint>,
    pub device: DeviceState,
    pub client: ClientState,
    pub flags_emitted: u64,
    pub last_decision: String,
    pressed: bool,
    last_emit_ms: Option<u128>,
}

impl BrokerModel {
    pub fn new(binding: u16, minimum_interval_ms: u128) -> Self {
        Self {
            binding,
            minimum_interval_ms,
            authorized_device: None,
            device: DeviceState::AwaitingDevice,
            client: ClientState::Disconnected,
            flags_emitted: 0,
            last_decision: "waiting for the selected stable device identity".into(),
            pressed: false,
            last_emit_ms: None,
        }
    }

    pub fn replace_candidates(&mut self, candidates: Vec<Candidate>) {
        if self.device == DeviceState::Suspended {
            self.last_decision = "ignored discovery while suspended".into();
            return;
        }
        self.pressed = false;
        match candidates.as_slice() {
            [] => {
                self.device = DeviceState::Disconnected("selected identity is absent".into());
                self.last_decision = "disarmed because the selected identity is absent".into();
            }
            [candidate] => match &self.authorized_device {
                None => {
                    self.authorized_device = Some(candidate.fingerprint.clone());
                    self.device = DeviceState::Armed {
                        event_path: candidate.event_path.clone(),
                    };
                    self.last_decision = "captured the user-selected identity and armed".into();
                }
                Some(authorized) if authorized == &candidate.fingerprint => {
                    self.device = DeviceState::Armed {
                        event_path: candidate.event_path.clone(),
                    };
                    self.last_decision = "same identity returned and re-armed".into();
                }
                Some(_) => {
                    self.device = DeviceState::IdentityChanged;
                    self.last_decision = "rejected unexpected identity metadata".into();
                }
            },
            many => {
                self.device = DeviceState::Ambiguous {
                    matches: many.len(),
                };
                self.last_decision = "rejected ambiguous stable identity".into();
            }
        }
    }

    pub fn permission_denied(&mut self, error: String) {
        self.pressed = false;
        self.device = DeviceState::PermissionDenied(error);
        self.last_decision =
            "disarmed because the broker lacks the narrow device permission".into();
    }

    pub fn identity_changed(&mut self, error: String) {
        self.pressed = false;
        self.device = DeviceState::IdentityChanged;
        self.last_decision = format!("rejected unexpected identity metadata: {error}");
    }

    pub fn disconnect(&mut self, reason: String) {
        self.pressed = false;
        self.device = DeviceState::Disconnected(reason);
        self.last_decision = "disarmed after device loss".into();
    }

    pub fn suspend(&mut self) {
        self.pressed = false;
        self.device = DeviceState::Suspended;
        self.last_decision = "closed and disarmed for suspend".into();
    }

    pub fn resume(&mut self) {
        self.pressed = false;
        self.device = DeviceState::AwaitingDevice;
        self.last_decision = "resumed disarmed and requires identity revalidation".into();
    }

    pub fn begin_sync_recovery(&mut self) {
        let event_path = match &self.device {
            DeviceState::Armed { event_path } => event_path.clone(),
            _ => return,
        };
        self.pressed = false;
        self.device = DeviceState::Recovering { event_path };
        self.last_decision =
            "discarding events until SYN_REPORT and state resynchronization".into();
    }

    pub fn finish_sync_recovery(&mut self, binding_is_held: bool) {
        let event_path = match &self.device {
            DeviceState::Recovering { event_path } => event_path.clone(),
            _ => return,
        };
        self.pressed = binding_is_held;
        self.device = DeviceState::Armed { event_path };
        self.last_decision = if binding_is_held {
            "recovered with the binding held; release is required before another flag".into()
        } else {
            "recovered with the binding released and re-armed".into()
        };
    }

    pub fn connect_client(&mut self, uid: u32, allowed_uid: u32) {
        self.client = if uid == allowed_uid {
            ClientState::Authorized { uid }
        } else {
            ClientState::Rejected { uid }
        };
        self.last_decision = if uid == allowed_uid {
            "accepted the configured daemon UID".into()
        } else {
            "rejected an unexpected IPC peer UID".into()
        };
    }

    pub fn disconnect_client(&mut self) {
        self.client = ClientState::Disconnected;
        self.last_decision = "IPC client disconnected".into();
    }

    pub fn observe_key(&mut self, code: u16, value: i32, now_ms: u128) -> bool {
        if code != self.binding {
            return false;
        }
        if value == 0 {
            self.pressed = false;
            self.last_decision = "binding released".into();
            return false;
        }
        if !matches!(self.device, DeviceState::Armed { .. })
            || !matches!(self.client, ClientState::Authorized { .. })
        {
            self.last_decision =
                "suppressed input while device or client was not authorized".into();
            return false;
        }
        if value != 1 || self.pressed {
            self.last_decision = "suppressed key repeat or duplicate press".into();
            return false;
        }
        self.pressed = true;
        if self
            .last_emit_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < self.minimum_interval_ms)
        {
            self.last_decision = "suppressed press inside the rate limit".into();
            return false;
        }
        self.last_emit_ms = Some(now_ms);
        self.flags_emitted += 1;
        self.last_decision = "emitted the literal one-bit FLAG message".into();
        true
    }
}

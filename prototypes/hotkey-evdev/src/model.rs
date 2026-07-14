//! PROTOTYPE ONLY. This is the state machine being exercised by the evdev spike.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding(pub u16);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    Reading,
    PermissionDenied(String),
    Disconnected(String),
    Conflict(String),
}

#[derive(Debug, Clone)]
pub struct DeviceView {
    pub path: String,
    pub name: String,
    pub physical_id: String,
    pub state: DeviceState,
}

#[derive(Debug, Clone)]
pub struct Press {
    pub physical_id: String,
    pub code: u16,
    pub value: i32,
    pub at_ms: u128,
}

#[derive(Debug)]
pub struct Model {
    pub binding: Binding,
    pub devices: Vec<DeviceView>,
    pub manual_flags: u64,
    pub last_flag: Option<String>,
    armed: bool,
    pressed: HashSet<(String, u16)>,
    last_physical_press: Option<(String, u16, u128)>,
}

impl Model {
    pub fn new(binding: Binding) -> Self {
        Self {
            binding,
            devices: Vec::new(),
            manual_flags: 0,
            last_flag: None,
            armed: false,
            pressed: HashSet::new(),
            last_physical_press: None,
        }
    }

    pub fn replace_devices(&mut self, mut devices: Vec<DeviceView>) {
        self.pressed.clear();
        let readable = devices
            .iter()
            .filter(|device| device.state == DeviceState::Reading)
            .count();
        if readable > 1 {
            for device in &mut devices {
                if device.state == DeviceState::Reading {
                    device.state = DeviceState::Conflict(
                        "multiple matching event nodes; duplicate filtering is heuristic".into(),
                    );
                }
            }
        }
        self.devices = devices;
        self.recompute_armed();
    }

    pub fn mark_disconnected(&mut self, path: &str, reason: String) {
        if let Some(device) = self.devices.iter_mut().find(|device| device.path == path) {
            device.state = DeviceState::Disconnected(reason);
        }
        self.pressed.clear();
        self.recompute_armed();
    }

    /// SYN_DROPPED recovery must never create a flag. A held key is seeded so it
    /// must be released before a later down transition can be considered new.
    pub fn recover_key_state(&mut self, physical_id: String, held: bool) {
        let key = (physical_id, self.binding.0);
        if held {
            self.pressed.insert(key);
        } else {
            self.pressed.remove(&key);
        }
    }

    /// Returns true exactly once for a physical down transition. value=2 is autorepeat.
    pub fn observe(&mut self, press: Press) -> bool {
        if press.code != self.binding.0 {
            return false;
        }
        let key = (press.physical_id.clone(), press.code);
        if press.value == 0 {
            self.pressed.remove(&key);
            return false;
        }
        if !self.armed {
            return false;
        }
        if press.value != 1 || !self.pressed.insert(key) {
            return false;
        }
        // Composite keyboards can expose one physical press through several event nodes.
        // Timestamps are the best cheap identity evdev exposes here, not a correctness proof.
        if let Some((physical_id, code, at_ms)) = &self.last_physical_press {
            if *physical_id == press.physical_id
                && *code == press.code
                && press.at_ms.saturating_sub(*at_ms) < 8
            {
                return false;
            }
        }
        self.last_physical_press = Some((press.physical_id.clone(), press.code, press.at_ms));
        self.manual_flags += 1;
        self.last_flag = Some(format!("{} at {}ms", press.physical_id, press.at_ms));
        true
    }

    fn recompute_armed(&mut self) {
        self.armed = self
            .devices
            .iter()
            .filter(|device| device.state == DeviceState::Reading)
            .count()
            == 1;
    }
}

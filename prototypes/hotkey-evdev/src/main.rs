//! PROTOTYPE ONLY. `cargo run` answers whether passive evdev reads reach a Wayland game.

mod model;

use evdev::raw_stream::RawDevice;
use evdev::{EventSummary, KeyCode, SynchronizationCode};
use model::{Binding, DeviceState, DeviceView, Model, Press};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Reader {
    path: PathBuf,
    physical_id: String,
    device: RawDevice,
    dropping: bool,
}

fn main() -> io::Result<()> {
    let binding = Binding(
        std::env::var("HOTKEY_CODE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(KeyCode::KEY_F8.0),
    );
    let device_filter = std::env::var("HOTKEY_DEVICE").ok();
    let mut model = Model::new(binding);
    let mut readers = HashMap::new();
    let mut notices = Vec::new();
    let (commands, command_rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            let _ = commands.send(line);
        }
    });
    let mut last_scan = SystemTime::UNIX_EPOCH;

    loop {
        let mut force_rescan = false;
        while let Ok(command) = command_rx.try_recv() {
            match command.trim() {
                "r" => force_rescan = true,
                "q" => return Ok(()),
                other => notices.push(format!("unknown shortcut {other:?}; use r or q")),
            }
        }
        if force_rescan || last_scan.elapsed().unwrap_or_default() > Duration::from_secs(2) {
            rescan(
                binding,
                device_filter.as_deref(),
                &mut readers,
                &mut model,
                &mut notices,
            );
            last_scan = SystemTime::now();
        }
        poll_events(&mut readers, &mut model, &mut notices);
        render(&model, &notices);
        notices.clear();
        thread::sleep(Duration::from_millis(40));
    }
}

fn rescan(
    binding: Binding,
    device_filter: Option<&str>,
    readers: &mut HashMap<PathBuf, Reader>,
    model: &mut Model,
    notices: &mut Vec<String>,
) {
    let mut views = Vec::new();
    let paths: Vec<PathBuf> = match fs::read_dir("/dev/input") {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("event"))
            })
            .collect(),
        Err(error) => {
            model.replace_devices(vec![DeviceView {
                path: "/dev/input".into(),
                name: "input directory".into(),
                physical_id: "none".into(),
                state: DeviceState::PermissionDenied(error.to_string()),
            }]);
            return;
        }
    };
    readers.retain(|path, _| paths.contains(path));
    for path in paths {
        if !readers.contains_key(&path) {
            // File::open is deliberately read-only. Do not use Device::open: it first asks
            // for write access, and this prototype never grabs, writes, or creates uinput.
            match fs::File::open(&path).and_then(|file| RawDevice::from_fd(file.into())) {
                Ok(device)
                    if device
                        .supported_keys()
                        .is_some_and(|keys| keys.contains(KeyCode(binding.0))) =>
                {
                    let name = device.name().unwrap_or("unnamed");
                    if device_filter.is_some_and(|filter| {
                        !name.contains(filter) && !path.to_string_lossy().contains(filter)
                    }) {
                        continue;
                    }
                    let physical_id = device
                        .physical_path()
                        .unwrap_or_else(|| path.to_str().unwrap_or("unknown"))
                        .to_owned();
                    if let Err(error) = set_nonblocking(&device) {
                        notices.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                    readers.insert(
                        path.clone(),
                        Reader {
                            path: path.clone(),
                            physical_id,
                            device,
                            dropping: false,
                        },
                    );
                }
                Ok(_) => {}
                Err(error) => views.push(DeviceView {
                    path: path.display().to_string(),
                    name: "unreadable".into(),
                    physical_id: path.display().to_string(),
                    state: DeviceState::PermissionDenied(error.to_string()),
                }),
            }
        }
    }
    for reader in readers.values() {
        views.push(DeviceView {
            path: reader.path.display().to_string(),
            name: reader.device.name().unwrap_or("unnamed").into(),
            physical_id: reader.physical_id.clone(),
            state: DeviceState::Reading,
        });
    }
    model.replace_devices(views);
}

fn poll_events(
    readers: &mut HashMap<PathBuf, Reader>,
    model: &mut Model,
    notices: &mut Vec<String>,
) {
    let mut dead = Vec::new();
    for (path, reader) in readers.iter_mut() {
        let events = match reader.device.fetch_events() {
            Ok(events) => events.collect::<Vec<_>>(),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => {
                dead.push(path.clone());
                model.mark_disconnected(&path.display().to_string(), error.to_string());
                notices.push(format!(
                    "{} disconnected or conflicted: {error}",
                    path.display()
                ));
                continue;
            }
        };
        for event in events {
            match event.destructure() {
                EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => {
                    reader.dropping = true;
                    notices.push(format!(
                        "{} dropped events; recovering current key state",
                        path.display()
                    ));
                }
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _)
                    if reader.dropping =>
                {
                    reader.dropping = false;
                    match reader.device.get_key_state() {
                        Ok(keys) => {
                            let held = keys.contains(KeyCode(model.binding.0));
                            model.recover_key_state(reader.physical_id.clone(), held);
                            notices.push(format!(
                                "{} recovery complete; binding is {}",
                                path.display(),
                                if held {
                                    "held, release required"
                                } else {
                                    "released"
                                }
                            ));
                        }
                        Err(error) => notices.push(format!(
                            "{} could not recover key state: {error}",
                            path.display()
                        )),
                    }
                }
                EventSummary::Key(_, code, value)
                    if !reader.dropping
                        && model.observe(Press {
                            physical_id: reader.physical_id.clone(),
                            code: code.0,
                            value,
                            at_ms: event
                                .timestamp()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis(),
                        }) =>
                {
                    notices.push(format!("MANUAL FLAG #{}", model.manual_flags));
                }
                _ => {}
            }
        }
    }
    for path in dead {
        readers.remove(&path);
    }
}

fn set_nonblocking(device: &RawDevice) -> io::Result<()> {
    let fd = device.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn render(model: &Model, notices: &[String]) {
    print!("\x1b[2J\x1b[H");
    println!("hotkey-evdev PROTOTYPE | passive read, no EVIOCGRAB");
    println!(
        "Binding: evdev code {} | Manual Flags: {}",
        model.binding.0, model.manual_flags
    );
    println!(
        "Filter: {}",
        std::env::var("HOTKEY_DEVICE")
            .unwrap_or_else(|_| "unset: exactly one matching device is required".into())
    );
    println!(
        "Shortcuts: r rescan, q quit. This small spike rescans every 2 seconds; Ctrl-C exits."
    );
    println!("Run: cargo run | HOTKEY_CODE=66 cargo run | HOTKEY_DEVICE=Wooting HOTKEY_CODE=66 cargo run");
    println!("Devices:");
    for device in &model.devices {
        println!(
            "  {} | {} | {} | {:?}",
            device.path, device.name, device.physical_id, device.state
        );
    }
    if let Some(last) = &model.last_flag {
        println!("Last flag: {last}");
    }
    for notice in notices {
        println!("NOTICE: {notice}");
    }
    println!("SYN_DROPPED recovery ignores queued input, reads current key state, and requires release if held.");
    println!(
        "\nWayland test: focus a fullscreen game, press the configured key, and watch the counter."
    );
}

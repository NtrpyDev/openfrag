//! PROTOTYPE ONLY. Proves a device-scoped evdev broker and one-bit IPC boundary.

mod model;

use evdev::raw_stream::RawDevice;
use evdev::{EventSummary, KeyCode, SynchronizationCode};
use model::{BrokerModel, Candidate, Fingerprint};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const RATE_LIMIT_MS: u128 = 750;
static SUSPEND_REQUESTED: AtomicBool = AtomicBool::new(false);
static RESUME_REQUESTED: AtomicBool = AtomicBool::new(false);

struct Reader {
    path: PathBuf,
    device: RawDevice,
}

fn main() -> io::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    match args.get(1).map(String::as_str) {
        Some("simulate") => simulate(),
        Some("client") if args.len() == 3 => client(Path::new(&args[2])),
        Some("broker") if args.len() == 8 => broker(BrokerConfig {
            serial: args[2].clone(),
            interface: args[3].clone(),
            binding: parse(&args[4], "key code")?,
            socket: PathBuf::from(&args[5]),
            allowed_client_uid: parse(&args[6], "client UID")?,
            expected_name: args[7].clone(),
        }),
        _ => {
            eprintln!(
                "usage:\n  hotkey-evdev-prototype simulate\n  hotkey-evdev-prototype client <socket>\n  hotkey-evdev-prototype broker <ID_SERIAL> <interface> <key-code> <socket> <client-uid> <device-name>"
            );
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid arguments",
            ))
        }
    }
}

fn parse<T: std::str::FromStr>(value: &str, name: &str) -> io::Result<T> {
    value.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {name}: {value}"),
        )
    })
}

struct BrokerConfig {
    serial: String,
    interface: String,
    binding: u16,
    socket: PathBuf,
    allowed_client_uid: u32,
    expected_name: String,
}

fn broker(config: BrokerConfig) -> io::Result<()> {
    install_signal_handlers()?;
    let listener = bind_socket(&config.socket)?;
    let mut model = BrokerModel::new(config.binding, RATE_LIMIT_MS);
    let mut reader: Option<Reader> = None;
    let mut client: Option<UnixStream> = None;
    let started = Instant::now();
    let mut last_scan = Instant::now() - Duration::from_secs(2);
    println!(
        "PROTOTYPE broker pid={} uid={}",
        std::process::id(),
        unsafe { libc::geteuid() }
    );
    println!(
        "selector ID_SERIAL={} interface={} expected_name={:?} code={} socket={} allowed_uid={}",
        config.serial,
        config.interface,
        config.expected_name,
        config.binding,
        config.socket.display(),
        config.allowed_client_uid
    );

    loop {
        if SUSPEND_REQUESTED.swap(false, Ordering::SeqCst) {
            reader = None;
            model.suspend();
            print_state(&model);
        }
        if RESUME_REQUESTED.swap(false, Ordering::SeqCst) {
            model.resume();
            last_scan = Instant::now() - Duration::from_secs(2);
            print_state(&model);
        }
        if last_scan.elapsed() >= Duration::from_secs(1) {
            refresh_device(&config, &mut model, &mut reader);
            last_scan = Instant::now();
        }
        accept_client(
            &listener,
            config.allowed_client_uid,
            &mut model,
            &mut client,
        );
        poll_events(
            &mut reader,
            &mut model,
            &mut client,
            started.elapsed().as_millis(),
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn bind_socket(path: &Path) -> io::Result<UnixListener> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "refusing to replace a non-socket path",
            ));
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o666))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn refresh_device(config: &BrokerConfig, model: &mut BrokerModel, reader: &mut Option<Reader>) {
    if model.device == model::DeviceState::Suspended {
        return;
    }
    let paths = match matching_event_paths(&config.serial, &config.interface) {
        Ok(paths) => paths,
        Err(error) => {
            model.disconnect(format!("udev discovery failed: {error}"));
            *reader = None;
            print_state(model);
            return;
        }
    };
    if paths.len() != 1 {
        let candidates = paths
            .into_iter()
            .map(|path| Candidate {
                event_path: path.display().to_string(),
                fingerprint: configured_placeholder(config),
            })
            .collect();
        model.replace_candidates(candidates);
        *reader = None;
        print_state(model);
        return;
    }
    let path = paths[0].clone();
    if reader.as_ref().is_some_and(|open| open.path == path) {
        return;
    }
    match open_candidate(config, &path) {
        Ok((candidate, device)) => {
            model.replace_candidates(vec![candidate]);
            if matches!(model.device, model::DeviceState::Armed { .. }) {
                *reader = Some(Reader { path, device });
            } else {
                *reader = None;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            model.permission_denied(format!("{}: {error}", path.display()));
            *reader = None;
        }
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            model.identity_changed(format!("{}: {error}", path.display()));
            *reader = None;
        }
        Err(error) => {
            model.disconnect(format!("{}: {error}", path.display()));
            *reader = None;
        }
    }
    print_state(model);
}

fn configured_placeholder(config: &BrokerConfig) -> Fingerprint {
    Fingerprint {
        serial: config.serial.clone(),
        interface: config.interface.clone(),
        name: "unopened ambiguous candidate".into(),
        input_id: "unopened".into(),
        physical_path: "unopened".into(),
        unique_name: "unopened".into(),
    }
}

fn open_candidate(config: &BrokerConfig, path: &Path) -> io::Result<(Candidate, RawDevice)> {
    let file = fs::File::open(path)?;
    let device = RawDevice::from_fd(file.into())?;
    if device.name() != Some(config.expected_name.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "device name changed from {:?} to {:?}",
                config.expected_name,
                device.name()
            ),
        ));
    }
    if !device
        .supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode(config.binding)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "selected device does not expose the allowlisted code",
        ));
    }
    set_nonblocking(&device)?;
    let fingerprint = Fingerprint {
        serial: config.serial.clone(),
        interface: config.interface.clone(),
        name: device.name().unwrap_or("unnamed").into(),
        input_id: format!("{:?}", device.input_id()),
        physical_path: device.physical_path().unwrap_or("missing").into(),
        unique_name: device.unique_name().unwrap_or("missing").into(),
    };
    Ok((
        Candidate {
            event_path: path.display().to_string(),
            fingerprint,
        },
        device,
    ))
}

fn matching_event_paths(serial: &str, interface: &str) -> io::Result<Vec<PathBuf>> {
    let output = Command::new("/usr/bin/udevadm")
        .args(["info", "--export-db"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("udevadm info --export-db failed"));
    }
    let database = String::from_utf8_lossy(&output.stdout);
    let mut matches = Vec::new();
    for record in database.split("\n\n") {
        let mut node = None;
        let mut properties = HashMap::new();
        for line in record.lines() {
            if let Some(value) = line.strip_prefix("N: ") {
                node = Some(value);
            } else if let Some(value) = line.strip_prefix("E: ") {
                if let Some((key, value)) = value.split_once('=') {
                    properties.insert(key, value);
                }
            }
        }
        if properties.get("ID_SERIAL") == Some(&serial)
            && properties.get("ID_USB_INTERFACE_NUM") == Some(&interface)
            && properties.get("ID_INPUT_KEYBOARD") == Some(&"1")
        {
            if let Some(node) = node.filter(|node| node.starts_with("input/event")) {
                matches.push(Path::new("/dev").join(node));
            }
        }
    }
    matches.sort();
    Ok(matches)
}

fn accept_client(
    listener: &UnixListener,
    allowed_uid: u32,
    model: &mut BrokerModel,
    current: &mut Option<UnixStream>,
) {
    match listener.accept() {
        Ok((stream, _)) => match peer_uid(&stream) {
            Ok(uid) => {
                model.connect_client(uid, allowed_uid);
                if uid == allowed_uid {
                    *current = Some(stream);
                    print_state(model);
                } else {
                    print_state(model);
                    if current.is_some() {
                        model.connect_client(allowed_uid, allowed_uid);
                    } else {
                        model.disconnect_client();
                    }
                }
            }
            Err(error) => eprintln!("peer credential failure: {error}"),
        },
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Err(error) => eprintln!("socket accept failure: {error}"),
    }
}

fn poll_events(
    reader: &mut Option<Reader>,
    model: &mut BrokerModel,
    client: &mut Option<UnixStream>,
    now_ms: u128,
) {
    let Some(mut open) = reader.take() else {
        return;
    };
    let fetched = open
        .device
        .fetch_events()
        .map(|events| events.collect::<Vec<_>>());
    let events = match fetched {
        Ok(events) => events,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            *reader = Some(open);
            return;
        }
        Err(error) => {
            model.disconnect(error.to_string());
            print_state(model);
            return;
        }
    };
    for event in events {
        match event.destructure() {
            EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => {
                model.begin_sync_recovery();
                print_state(model);
            }
            EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _)
                if matches!(model.device, model::DeviceState::Recovering { .. }) =>
            {
                match open.device.get_key_state() {
                    Ok(keys) => {
                        model.finish_sync_recovery(keys.contains(KeyCode(model.binding)));
                        print_state(model);
                    }
                    Err(error) => {
                        model.disconnect(format!("key-state recovery failed: {error}"));
                        print_state(model);
                        return;
                    }
                }
            }
            EventSummary::Key(_, code, value) if model.observe_key(code.0, value, now_ms) => {
                let delivered = client
                    .as_mut()
                    .is_some_and(|stream| stream.write_all(b"FLAG\n").is_ok());
                if !delivered {
                    *client = None;
                    model.disconnect_client();
                }
                print_state(model);
            }
            _ => {}
        }
    }
    *reader = Some(open);
}

fn client(path: &Path) -> io::Result<()> {
    let stream = UnixStream::connect(path)?;
    let mut flags = 0_u64;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line != "FLAG" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "broker crossed the one-bit protocol boundary",
            ));
        }
        flags += 1;
        println!("MANUAL FLAG #{flags}");
    }
    Ok(())
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(credentials.uid)
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

fn install_signal_handlers() -> io::Result<()> {
    extern "C" fn suspend(_: libc::c_int) {
        SUSPEND_REQUESTED.store(true, Ordering::SeqCst);
    }
    extern "C" fn resume(_: libc::c_int) {
        RESUME_REQUESTED.store(true, Ordering::SeqCst);
    }
    if unsafe { libc::signal(libc::SIGUSR1, suspend as *const () as libc::sighandler_t) }
        == libc::SIG_ERR
        || unsafe { libc::signal(libc::SIGUSR2, resume as *const () as libc::sighandler_t) }
            == libc::SIG_ERR
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn simulate() -> io::Result<()> {
    let mut model = BrokerModel::new(KeyCode::KEY_F8.0, RATE_LIMIT_MS);
    let mut now = 1_000_u128;
    let base = sample_fingerprint("Wooting Two HE (ARM)");
    loop {
        print!("\x1b[2J\x1b[H");
        println!("evdev broker boundary PROTOTYPE simulator\n");
        println!("{model:#?}");
        println!(
            "\n[1] one device  [a] ambiguous  [x] changed identity  [d] disconnect\n[p] permission denied  [c] connect allowed client  [j] reject client\n[k] key down  [u] key up  [r] repeat  [h] SYN_DROPPED held  [n] SYN_DROPPED clear\n[s] suspend  [w] resume  [t] advance 1 second  [q] quit"
        );
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Ok(());
        }
        match input.trim() {
            "1" => model.replace_candidates(vec![candidate("/dev/input/event4", &base)]),
            "a" => model.replace_candidates(vec![
                candidate("/dev/input/event4", &base),
                candidate("/dev/input/event9", &base),
            ]),
            "x" => model.replace_candidates(vec![candidate(
                "/dev/input/event9",
                &sample_fingerprint("unexpected replacement"),
            )]),
            "d" => model.replace_candidates(vec![]),
            "p" => model.permission_denied("EACCES on selected event node".into()),
            "c" => model.connect_client(1000, 1000),
            "j" => model.connect_client(65534, 1000),
            "k" => {
                model.observe_key(KeyCode::KEY_F8.0, 1, now);
            }
            "u" => {
                model.observe_key(KeyCode::KEY_F8.0, 0, now);
            }
            "r" => {
                model.observe_key(KeyCode::KEY_F8.0, 2, now);
            }
            "h" => {
                model.begin_sync_recovery();
                model.finish_sync_recovery(true);
            }
            "n" => {
                model.begin_sync_recovery();
                model.finish_sync_recovery(false);
            }
            "s" => model.suspend(),
            "w" => model.resume(),
            "t" => now += 1_000,
            "q" => return Ok(()),
            _ => model.last_decision = "unknown simulator command".into(),
        }
    }
}

fn sample_fingerprint(name: &str) -> Fingerprint {
    Fingerprint {
        serial: "Wooting_Wooting_Two_HE__ARM__A02B2442W043H25541".into(),
        interface: "01".into(),
        name: name.into(),
        input_id: "InputId { bus_type: BUS_USB, vendor: 12771, product: 4658 }".into(),
        physical_path: "usb-0000:00:14.0-6.1/input1".into(),
        unique_name: "A02B2442W043H25541".into(),
    }
}

fn candidate(path: &str, fingerprint: &Fingerprint) -> Candidate {
    Candidate {
        event_path: path.into(),
        fingerprint: fingerprint.clone(),
    }
}

fn print_state(model: &BrokerModel) {
    println!(
        "STATE device={:?} client={:?} flags={} decision={}",
        model.device, model.client, model.flags_emitted, model.last_decision
    );
}

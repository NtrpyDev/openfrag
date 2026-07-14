//! PROTOTYPE ONLY. Captures real CS2 GSI POSTs; delete after the listener question is answered.

mod model;

use model::Model;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Response, Server, StatusCode};

const ADDRESS: &str = "127.0.0.1:27100";
const MAX_BODY_BYTES: u64 = 128 * 1024;
const CAPTURE_DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/captures");

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = std::env::var("GSI_PATH").unwrap_or_else(|_| "/gsi".into());
    let token = std::env::var("GSI_TOKEN")
        .map_err(|_| "GSI_TOKEN is required; use the same random value in the local CS2 cfg")?;
    let capture_path = Path::new(CAPTURE_DIRECTORY).join("gsi-listener-capture.jsonl");
    let server = Server::http(ADDRESS)?;
    let quit = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&quit);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;

    let started = Instant::now();
    let mut model = Model::default();
    let mut last_frame = Instant::now() - Duration::from_secs(1);
    while !quit.load(Ordering::SeqCst) {
        if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(100)) {
            if let Err(error) = handle(request, &path, &token, &capture_path, &started, &mut model)
            {
                model.request_failed(error.to_string());
            }
        }
        if last_frame.elapsed() >= Duration::from_millis(250) {
            render(&model, started.elapsed().as_millis(), &path, &capture_path);
            last_frame = Instant::now();
        }
    }
    println!(
        "\nPrototype stopped. Capture remains at {}",
        capture_path.display()
    );
    Ok(())
}

fn handle(
    mut request: tiny_http::Request,
    path: &str,
    token: &str,
    capture_path: &Path,
    started: &Instant,
    model: &mut Model,
) -> io::Result<()> {
    if request.url() != path {
        request.respond(Response::empty(StatusCode(404)))?;
        return Ok(());
    }
    if request.method() != &Method::Post {
        let response = Response::empty(StatusCode(405));
        let response = match Header::from_bytes("Allow", "POST") {
            Ok(header) => response.with_header(header),
            Err(_) => response,
        };
        request.respond(response)?;
        return Ok(());
    }
    let is_json = request.headers().iter().any(|header| {
        header.field.equiv("Content-Type")
            && header
                .value
                .as_str()
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    });
    if !is_json {
        model.reject();
        request.respond(Response::empty(StatusCode(415)))?;
        return Ok(());
    }

    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut body)?;
    if body.len() as u64 > MAX_BODY_BYTES {
        model.reject();
        request.respond(Response::empty(StatusCode(413)))?;
        return Ok(());
    }
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(_) => {
            model.reject();
            request.respond(Response::empty(StatusCode(400)))?;
            return Ok(());
        }
    };
    if payload.pointer("/auth/token").and_then(Value::as_str) != Some(token) {
        model.reject();
        request.respond(Response::empty(StatusCode(403)))?;
        return Ok(());
    }

    let received_at_ms = started.elapsed().as_millis();
    let sanitized = sanitize(&payload);
    let hash = hex_hash(&serde_json::to_vec(&sanitized)?);
    model.observe(received_at_ms, &payload, hash.clone());
    match append_capture(capture_path, received_at_ms, hash, sanitized) {
        Ok(()) => model.capture_succeeded(),
        Err(error) => model.capture_failed(error.to_string()),
    }
    request.respond(Response::empty(StatusCode(204)))?;
    Ok(())
}

fn append_capture(
    path: &Path,
    received_at_ms: u128,
    hash: String,
    sanitized: Value,
) -> io::Result<()> {
    let capture_path = constrained_capture_path(path)?;
    let line = json!({
        "prototype": "gsi-listener",
        "receive_monotonic_ms": received_at_ms,
        "payload_sha256": hash,
        "payload": sanitized,
    });
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(capture_path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut file, &line)?;
    file.write_all(b"\n")
}

fn sanitize(payload: &Value) -> Value {
    let mut sanitized = payload.clone();
    if let Some(token) = sanitized.pointer_mut("/auth/token") {
        *token = Value::String("<redacted>".into());
    }
    sanitized
}

fn constrained_capture_path(path: &Path) -> io::Result<PathBuf> {
    let parent = Path::new(CAPTURE_DIRECTORY);
    if parent.exists() {
        if std::fs::symlink_metadata(parent)?.file_type().is_symlink() {
            return Err(io::Error::other("capture parent must not be a symlink"));
        }
    } else {
        std::fs::create_dir(parent)?;
    }
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    if parent.canonicalize()? != parent {
        return Err(io::Error::other(
            "capture parent escaped the prototype directory",
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("capture file name missing"))?;
    Ok(parent.join(name))
}

fn hex_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn render(model: &Model, now_ms: u128, path: &str, capture_path: &Path) {
    print!("\x1b[2J\x1b[H");
    println!("GSI LISTENER PROTOTYPE | loopback-only POST capture");
    println!("Endpoint: http://{ADDRESS}{path} | body limit: {MAX_BODY_BYTES} bytes");
    println!(
        "Capture: {} (0700 parent, 0600 file) | Ctrl-C exits cleanly",
        capture_path.display()
    );
    println!(
        "Requests: received={} accepted={} rejected={}",
        model.received, model.accepted, model.rejected
    );
    println!(
        "Receive monotonic ms: {} | provider timestamp: {}",
        show(&model.receive_monotonic_ms),
        show(&model.provider_timestamp)
    );
    println!(
        "Provider/observed player: {} / {} | identity relation: {}",
        show(&model.provider_steamid),
        show(&model.player_steamid),
        model.identity_relation
    );
    println!(
        "Map/mode/phase/round: {} / {} / {} / {}",
        show(&model.map),
        show(&model.mode),
        show(&model.phase),
        show(&model.round)
    );
    println!(
        "Observed kills/assists/round kills: {} / {} / {}",
        show(&model.observed_kills),
        show(&model.observed_assists),
        show(&model.observed_round_kills)
    );
    println!(
        "Delta objects: previously={} added={} | payload SHA-256: {}",
        model.saw_previously,
        model.saw_added,
        show(&model.payload_hash)
    );
    println!(
        "Warning: {}",
        model.warning(now_ms).unwrap_or_else(|| "none".into())
    );
    println!("\nRun: GSI_TOKEN=... cargo run. Configure CS2 uri to this endpoint and match GSI_TOKEN. Token is never displayed or captured.");
    println!("Prototype limitation: tiny_http does not impose a slow-client body-read timeout.");
    let _ = io::stdout().flush();
}

fn show<T: std::fmt::Display>(value: &Option<T>) -> String {
    value
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "<missing>".into())
}

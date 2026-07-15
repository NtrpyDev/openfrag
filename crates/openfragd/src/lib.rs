//! Loopback HTTP application for the openfrag daemon and local dashboard.
#![allow(clippy::missing_errors_doc)]

pub mod api;
pub mod capture_runtime;
pub mod clip_ports;
pub mod live_runtime;
pub mod service;
pub mod storage_clip_repository;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use openfrag_gsi::{Clock, EventSink, EvidenceReceipt, GsiConfig, GsiService};
use openfrag_setup::{
    CaptureConfigError, SetupFact, SetupFacts, SetupStepId, SetupStepStatus, evaluate_setup_flow,
    read_capture_configuration,
};
use openfrag_storage::{CaptureSessionId, Layout, Storage, StoredRatingAvailability};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const DASHBOARD: &str = include_str!("dashboard.html");

#[derive(Clone, Debug)]
pub struct AppConfig {
    data_directory: PathBuf,
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
}

impl AppConfig {
    pub fn new(data_directory: impl Into<PathBuf>) -> Self {
        Self {
            data_directory: data_directory.into(),
            ffmpeg: None,
            ffprobe: None,
        }
    }

    #[must_use]
    pub fn with_clip_tools(mut self, ffmpeg: PathBuf, ffprobe: PathBuf) -> Self {
        self.ffmpeg = Some(ffmpeg);
        self.ffprobe = Some(ffprobe);
        self
    }

    #[cfg(test)]
    fn for_test(data_directory: impl Into<PathBuf>) -> Self {
        Self::new(data_directory)
    }
}

#[derive(Debug)]
pub enum AppError {
    Storage(String),
    Configuration(String),
}

#[derive(Clone)]
struct AppState {
    #[allow(dead_code)]
    storage: Arc<Mutex<Storage>>,
    #[allow(dead_code)]
    runtime_worker: Option<Arc<live_runtime::RuntimeWorker>>,
    gsi_configured: bool,
}

#[derive(Serialize)]
struct Health {
    version: &'static str,
    binding: &'static str,
    telemetry: bool,
    upload_path: bool,
    database: &'static str,
    gsi: &'static str,
    rating_state: &'static str,
    rating: Option<String>,
}

pub fn app(config: &AppConfig) -> Result<Router, AppError> {
    let credentials = read_gsi_credentials(&config.data_directory)?;
    let gsi_configured = credentials.is_some();
    let local_steam_id = credentials
        .as_ref()
        .and_then(|(_, steam_id)| steam_id.parse::<u64>().ok());
    let storage = Storage::open(Layout::at(&config.data_directory))
        .map_err(|error| AppError::Storage(format!("{error:?}")))?;
    let storage = Arc::new(Mutex::new(storage));
    let import_ports: Arc<dyn service::MutationPorts> = Arc::new(service::PipelinePorts::new(
        storage.clone(),
        config.data_directory.clone(),
        local_steam_id,
    ));
    let clip_ports: Arc<dyn service::MutationPorts> = Arc::new(
        clip_ports::production_clip_ports(
            storage.clone(),
            &config.data_directory,
            config
                .ffmpeg
                .clone()
                .unwrap_or_else(|| PathBuf::from("ffmpeg")),
            configured_ffprobe(config),
        )
        .map_err(|error| {
            AppError::Configuration(format!("cannot compose local clip ports: {error:?}"))
        })?,
    );
    let ports = service::CompositePorts::new(
        import_ports,
        clip_ports,
        Arc::new(service::UnavailablePorts),
    );
    let local_api = service::StorageApi::new(
        storage.clone(),
        ports,
        setup_response(
            &config.data_directory,
            credentials.is_some(),
            local_steam_id.is_some(),
        ),
    );
    let mut gsi_service = None;
    let mut runtime_worker = None;
    if let Some((token, steam_id)) = &credentials {
        let (sink, driver): (
            Arc<dyn EventSink>,
            Option<live_runtime::ProductionRuntimeDriver>,
        ) = if let Ok(driver) = live_runtime::production_runtime(&config.data_directory, steam_id) {
            (driver.runtime(), Some(driver))
        } else {
            let session = storage
                .lock()
                .map_err(|_| AppError::Storage("storage lock poisoned".into()))?
                .create_capture_session(steam_id)
                .map_err(|error| AppError::Storage(format!("{error:?}")))?;
            (
                Arc::new(StorageGsiSink {
                    storage: storage.clone(),
                    session,
                }),
                None,
            )
        };
        let service = GsiService::new(
            GsiConfig::new(token, steam_id, Duration::from_secs(10)),
            Arc::new(SystemClock::default()),
            sink,
        );
        if let Some(driver) = driver {
            runtime_worker = Some(Arc::new(
                live_runtime::RuntimeWorker::start(driver, service.clone()).map_err(|error| {
                    AppError::Configuration(format!("cannot start live runtime worker: {error}"))
                })?,
            ));
        }
        gsi_service = Some(service);
    }
    let state = AppState {
        storage: storage.clone(),
        runtime_worker,
        gsi_configured,
    };
    let mut router = Router::new()
        .route("/", get(dashboard))
        .route("/api/health", get(health))
        .with_state(state);
    router = router.merge(api::router_without_health(Arc::new(local_api)));
    if let Some(service) = gsi_service {
        router = router.merge(openfrag_gsi::router(service));
    } else {
        router = router.route("/gsi/router", post(gsi_unavailable));
    }
    Ok(router)
}

fn configured_ffprobe(config: &AppConfig) -> PathBuf {
    config
        .ffprobe
        .clone()
        .or_else(|| {
            read_capture_configuration(&config.data_directory)
                .ok()
                .map(|configuration| configuration.ffprobe_path().to_path_buf())
        })
        .unwrap_or_else(|| PathBuf::from("ffprobe"))
}

async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD)
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    let rating = state
        .storage
        .lock()
        .ok()
        .and_then(|storage| storage.list_matches().ok())
        .and_then(|matches| matches.into_iter().next())
        .map(|record| record.rating);
    let (rating_state, rating) = match rating {
        Some(StoredRatingAvailability::Available { rating_bp, .. }) => (
            "rated",
            Some(format!(
                "{}.{:02}",
                rating_bp / 100,
                rating_bp.unsigned_abs() % 100
            )),
        ),
        Some(StoredRatingAvailability::Pending) => ("preview", None),
        Some(StoredRatingAvailability::Unavailable { .. }) | None => ("unavailable", None),
    };
    Json(Health {
        version: env!("CARGO_PKG_VERSION"),
        binding: "loopback",
        telemetry: false,
        upload_path: false,
        database: "ready",
        gsi: if state.gsi_configured {
            "ready"
        } else {
            "setup_required"
        },
        rating_state,
        rating,
    })
}

fn setup_response(
    data_directory: &Path,
    gsi_configured: bool,
    identity_configured: bool,
) -> api::SetupResponse {
    let (capture_recorder, ffprobe) = capture_setup_facts(data_directory);
    let flow = evaluate_setup_flow(&SetupFacts {
        storage: SetupFact::ready("Private local storage is ready"),
        local_steam_identity: if identity_configured {
            SetupFact::ready("Local Steam identity is configured")
        } else {
            SetupFact::blocked(
                "Local Steam identity is missing",
                "Run setup-gsi with the local Steam ID before importing a Demo",
            )
        },
        gsi_config: if gsi_configured {
            SetupFact::ready("Private loopback GSI is configured")
        } else {
            SetupFact::blocked(
                "Private loopback GSI is not configured",
                "Run setup-gsi before using live evidence",
            )
        },
        capture_recorder,
        ffprobe,
        test_capture: SetupFact::blocked(
            "A replay test capture has not been verified",
            "Run the bounded headless replay test before enabling live capture",
        ),
        local_demo_validation: if identity_configured {
            SetupFact::ready("Local Demo validation is available")
        } else {
            SetupFact::blocked(
                "Local Demo validation needs the player identity",
                "Configure the local Steam identity before selecting a Demo",
            )
        },
        manual_flag: SetupFact::blocked(
            "Manual Flag is not approved",
            "Complete the replay test, then approve the Global Shortcuts portal binding",
        ),
    });
    api::SetupResponse {
        checks: flow
            .steps
            .into_iter()
            .map(|step| {
                let summary = match step.remediation {
                    Some(remediation) => format!("{}. {remediation}", step.summary),
                    None => step.summary,
                };
                api::SetupCheck {
                    id: setup_step_id(step.id).into(),
                    status: match step.status {
                        SetupStepStatus::Ready => "ready",
                        SetupStepStatus::Blocked => "blocked",
                        SetupStepStatus::Skipped => "skipped",
                    }
                    .into(),
                    summary,
                }
            })
            .collect(),
    }
}

fn capture_setup_facts(data_directory: &Path) -> (SetupFact, SetupFact) {
    match read_capture_configuration(data_directory) {
        Ok(configuration) if configuration.enabled() => (
            SetupFact::ready("Replay capture is explicitly enabled"),
            SetupFact::ready("The configured ffprobe executable is valid"),
        ),
        Ok(_) => (
            SetupFact::blocked(
                "Replay capture is explicitly disabled",
                "Re-run setup-capture with --enabled true after the headless test passes",
            ),
            SetupFact::ready("The configured ffprobe executable is valid"),
        ),
        Err(CaptureConfigError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => (
            SetupFact::blocked(
                "Replay capture is not configured",
                "Run Doctor, then run setup-capture with validated local paths",
            ),
            SetupFact::blocked(
                "ffprobe is not configured",
                "Run Doctor and configure an absolute ffprobe executable path",
            ),
        ),
        Err(error) => (
            SetupFact::blocked(
                format!("Replay capture configuration is invalid: {error:?}"),
                "Re-run setup-capture with validated local paths",
            ),
            SetupFact::blocked(
                "ffprobe cannot be trusted from the current capture configuration",
                "Re-run setup-capture with an absolute executable ffprobe path",
            ),
        ),
    }
}

const fn setup_step_id(id: SetupStepId) -> &'static str {
    match id {
        SetupStepId::Storage => "storage",
        SetupStepId::LocalSteamIdentity => "local_steam_identity",
        SetupStepId::GsiConfig => "gsi",
        SetupStepId::CaptureRecorder => "capture",
        SetupStepId::Ffprobe => "ffprobe",
        SetupStepId::TestCapture => "test_capture",
        SetupStepId::LocalDemoValidation => "demo_import",
        SetupStepId::ManualFlag => "manual_flag",
    }
}

async fn gsi_unavailable() -> (StatusCode, &'static str) {
    (StatusCode::SERVICE_UNAVAILABLE, "gsi-setup-required")
}

fn read_gsi_credentials(data_directory: &Path) -> Result<Option<(String, String)>, AppError> {
    let token_path = data_directory.join("gsi-token");
    let steam_id_path = data_directory.join("local-steam-id");
    match (token_path.exists(), steam_id_path.exists()) {
        (false, false) => Ok(None),
        (true, true) => {
            let token = read_private_line(&token_path)?;
            let steam_id = read_private_line(&steam_id_path)?;
            Ok(Some((token, steam_id)))
        }
        _ => Err(AppError::Configuration(
            "GSI token and local Steam ID must be configured together".into(),
        )),
    }
}

fn read_private_line(path: &Path) -> Result<String, AppError> {
    let value = fs::read_to_string(path)
        .map_err(|error| AppError::Configuration(format!("{}: {error}", path.display())))?;
    let value = value.trim();
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(AppError::Configuration(format!(
            "{} contains an invalid value",
            path.display()
        )));
    }
    Ok(value.into())
}

struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

struct StorageGsiSink {
    storage: Arc<Mutex<Storage>>,
    session: CaptureSessionId,
}

impl EventSink for StorageGsiSink {
    fn emit(&self, receipt: EvidenceReceipt) -> Result<(), String> {
        let bytes = serde_json::to_vec(&receipt).map_err(|error| error.to_string())?;
        let fields = serde_json::to_string(&receipt.presence).map_err(|error| error.to_string())?;
        let ordinal = i64::try_from(receipt.sequence).map_err(|error| error.to_string())?;
        let received_at_ms =
            i64::try_from(receipt.received_at.as_millis()).map_err(|error| error.to_string())?;
        let storage = self
            .storage
            .lock()
            .map_err(|_| "storage lock poisoned".to_owned())?;
        let staged = storage
            .stage_artifact(&bytes)
            .map_err(|error| format!("{error:?}"))?;
        let artifact = storage
            .commit_artifact(staged, "json", Some("application/json"))
            .map_err(|error| format!("{error:?}"))?;
        storage
            .record_gsi_snapshot(
                &self.session,
                &artifact,
                ordinal,
                received_at_ms,
                &fields,
                env!("CARGO_PKG_VERSION"),
            )
            .map_err(|error| format!("{error:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, app};
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::Value;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_reports_private_local_v1() {
        let directory = tempfile::tempdir().expect("temporary data directory");
        let response = app(&AppConfig::for_test(directory.path()))
            .expect("application starts")
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("health response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("health body");
        let health: Value = serde_json::from_slice(&body).expect("health json");
        assert_eq!(health["version"], "1.0.0");
        assert_eq!(health["binding"], "loopback");
        assert_eq!(health["telemetry"], false);
        assert_eq!(health["upload_path"], false);
        assert_eq!(health["database"], "ready");
    }

    #[tokio::test]
    async fn setup_reports_missing_identity_and_gsi_without_guessing() {
        let directory = tempfile::tempdir().expect("temporary data directory");
        let response = app(&AppConfig::for_test(directory.path()))
            .expect("application starts")
            .oneshot(
                Request::builder()
                    .uri("/api/setup")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("setup response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("setup body");
        let setup: Value = serde_json::from_slice(&body).expect("setup json");
        let checks = setup["checks"].as_array().expect("setup checks");
        let status = |id: &str| {
            checks
                .iter()
                .find(|check| check["id"] == id)
                .and_then(|check| check["status"].as_str())
        };
        assert_eq!(status("storage"), Some("ready"));
        assert_eq!(status("local_steam_identity"), Some("blocked"));
        assert_eq!(status("gsi"), Some("skipped"));
        assert_eq!(status("capture"), Some("blocked"));
    }

    #[tokio::test]
    async fn dashboard_is_embedded_and_has_no_remote_assets() {
        let directory = tempfile::tempdir().expect("temporary data directory");
        let response = app(&AppConfig::for_test(directory.path()))
            .expect("application starts")
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("dashboard response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("dashboard body");
        let html = String::from_utf8(body.to_vec()).expect("UTF-8 dashboard");
        assert!(html.contains("Tonight"));
        assert!(html.contains("Import a local Demo"));
        assert!(html.contains("Local match evidence and clip review."));
        assert!(!html.contains("https://"));
        assert!(!html.contains("http://"));
    }

    #[tokio::test]
    async fn configured_gsi_route_persists_only_sanitized_local_evidence() {
        let directory = tempfile::tempdir().expect("temporary data directory");
        std::fs::write(directory.path().join("gsi-token"), "private-test-token\n")
            .expect("test token");
        std::fs::write(
            directory.path().join("local-steam-id"),
            "76561198000000001\n",
        )
        .expect("test Steam ID");
        let router = app(&AppConfig::for_test(directory.path())).expect("application starts");
        let payload = r#"{"provider":{"appid":730,"timestamp":1},"map":{"name":"de_mirage","mode":"competitive","round":1},"player":{"steamid":"76561198000000001","state":{"health":100},"match_stats":{"kills":0,"deaths":0}},"auth":{"token":"private-test-token"}}"#;

        let response = router
            .oneshot(
                Request::post("/gsi/router")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .expect("GSI request"),
            )
            .await
            .expect("GSI response");

        assert_eq!(response.status(), StatusCode::OK);
        let artifacts = directory.path().join("artifacts/sha256");
        let retained = std::fs::read_dir(artifacts)
            .expect("artifact directory")
            .filter_map(Result::ok)
            .filter_map(|prefix| std::fs::read_dir(prefix.path()).ok())
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .flatten()
            .collect::<Vec<_>>();
        assert!(!retained.is_empty());
        assert!(!String::from_utf8_lossy(&retained).contains("private-test-token"));
        assert!(!String::from_utf8_lossy(&retained).contains("76561198000000001"));
    }

    #[tokio::test]
    async fn unconfigured_gsi_route_is_explicitly_unavailable() {
        let directory = tempfile::tempdir().expect("temporary data directory");
        let response = app(&AppConfig::for_test(directory.path()))
            .expect("application starts")
            .oneshot(
                Request::post("/gsi/router")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("GSI request"),
            )
            .await
            .expect("GSI response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}

//! Loopback HTTP application for the openfrag daemon and local dashboard.
#![allow(clippy::missing_errors_doc)]

pub mod api;
pub mod service;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use openfrag_gsi::{Clock, EventSink, EvidenceReceipt, GsiConfig, GsiService};
use openfrag_storage::{CaptureSessionId, Layout, Storage};
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
}

impl AppConfig {
    pub fn new(data_directory: impl Into<PathBuf>) -> Self {
        Self {
            data_directory: data_directory.into(),
        }
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
    let local_steam_id = credentials
        .as_ref()
        .and_then(|(_, steam_id)| steam_id.parse::<u64>().ok());
    let storage = Storage::open(Layout::at(&config.data_directory))
        .map_err(|error| AppError::Storage(format!("{error:?}")))?;
    let storage = Arc::new(Mutex::new(storage));
    let state = AppState {
        storage: storage.clone(),
        gsi_configured: credentials.is_some(),
    };
    let mut router = Router::new()
        .route("/", get(dashboard))
        .route("/api/health", get(health))
        .with_state(state);
    let local_api = service::StorageApi::new(
        storage.clone(),
        service::PipelinePorts::new(
            storage.clone(),
            config.data_directory.clone(),
            local_steam_id,
        ),
        api::SetupResponse { checks: Vec::new() },
    );
    router = router.merge(api::router_without_health(Arc::new(local_api)));
    if let Some((token, steam_id)) = credentials {
        let session = storage
            .lock()
            .map_err(|_| AppError::Storage("storage lock poisoned".into()))?
            .create_capture_session(&steam_id)
            .map_err(|error| AppError::Storage(format!("{error:?}")))?;
        let sink = Arc::new(StorageGsiSink { storage, session });
        let service = GsiService::new(
            GsiConfig::new(&token, &steam_id, Duration::from_secs(10)),
            Arc::new(SystemClock::default()),
            sink,
        );
        router = router.merge(openfrag_gsi::router(service));
    } else {
        router = router.route("/gsi/router", post(gsi_unavailable));
    }
    Ok(router)
}

async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD)
}

async fn health(State(state): State<AppState>) -> Json<Health> {
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
        rating_state: "unavailable",
        rating: None,
    })
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

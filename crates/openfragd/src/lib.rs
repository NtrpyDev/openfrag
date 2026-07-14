//! Loopback HTTP application for the openfrag daemon and local dashboard.
#![allow(clippy::missing_errors_doc)]

use axum::{Json, Router, response::Html, routing::get};
use openfrag_storage::{Layout, Storage};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

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
}

#[derive(Clone)]
struct AppState {
    #[allow(dead_code)]
    storage: Arc<Mutex<Storage>>,
}

#[derive(Serialize)]
struct Health {
    version: &'static str,
    binding: &'static str,
    telemetry: bool,
    upload_path: bool,
    database: &'static str,
}

pub fn app(config: AppConfig) -> Result<Router, AppError> {
    let storage = Storage::open(Layout::at(config.data_directory))
        .map_err(|error| AppError::Storage(format!("{error:?}")))?;
    let state = AppState {
        storage: Arc::new(Mutex::new(storage)),
    };
    Ok(Router::new()
        .route("/", get(dashboard))
        .route("/api/health", get(health))
        .with_state(state))
}

async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD)
}

async fn health() -> Json<Health> {
    Json(Health {
        version: env!("CARGO_PKG_VERSION"),
        binding: "loopback",
        telemetry: false,
        upload_path: false,
        database: "ready",
    })
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
        let response = app(AppConfig::for_test(directory.path()))
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
        let response = app(AppConfig::for_test(directory.path()))
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
        assert!(html.contains("Import local Demo"));
        assert!(html.contains("No account. No telemetry. No uploads."));
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
        let router = app(AppConfig::for_test(directory.path())).expect("application starts");
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
        let response = app(AppConfig::for_test(directory.path()))
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

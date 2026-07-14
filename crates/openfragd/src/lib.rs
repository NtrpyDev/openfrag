//! Loopback HTTP application for the openfrag daemon and local dashboard.
#![allow(clippy::missing_errors_doc)]

use axum::{Json, Router, routing::get};
use openfrag_storage::{Layout, Storage};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

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

pub async fn app(config: AppConfig) -> Result<Router, AppError> {
    let storage = Storage::open(Layout::at(config.data_directory))
        .map_err(|error| AppError::Storage(format!("{error:?}")))?;
    let state = AppState {
        storage: Arc::new(Mutex::new(storage)),
    };
    Ok(Router::new()
        .route("/api/health", get(health))
        .with_state(state))
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
            .await
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
}

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

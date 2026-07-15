use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use openfragd::api::{EmptyLocalApi, router};
use openfragd::{AppConfig, app};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn empty_local_api_is_honest_about_unavailable_work() {
    let app = router(Arc::new(EmptyLocalApi));
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(health.into_body(), 4096).await.unwrap(),
        "{\"rating_state\":\"unavailable\",\"rating\":null}"
    );
    let flag = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/manual-flag")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(flag.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn trim_route_rejects_an_empty_range_before_clip_lookup() {
    let response = router(Arc::new(EmptyLocalApi))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/clips/clip-7/trim")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"start_ms":5000,"end_ms":5000}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap(),
        r#"{"message":"trim end must be greater than trim start"}"#
    );
}

#[tokio::test]
async fn fresh_home_can_install_discovered_gsi_and_defer_optional_setup() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let data = temp.path().join("data");
    let steam = home.join(".local/share/Steam/steamapps");
    let external = temp.path().join("games");
    let cfg = external.join("steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg");
    std::fs::create_dir_all(&steam).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        steam.join("libraryfolders.vdf"),
        format!(
            r#""libraryfolders" {{ "1" {{ "path" "{}" }} }}"#,
            external.display()
        ),
    )
    .unwrap();
    std::fs::write(
        external.join("steamapps/appmanifest_730.acf"),
        r#""AppState" { "installdir" "Counter-Strike Global Offensive" }"#,
    )
    .unwrap();
    std::fs::write(
        external.join("steamapps/common/Counter-Strike Global Offensive/game/csgo/gameinfo.gi"),
        "gameinfo",
    )
    .unwrap();

    let application = app(&AppConfig::new(&data).with_setup_home(&home)).unwrap();
    let initial = json_response(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/setup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(initial["complete"], false);
    assert_eq!(
        initial["cs2_cfg_candidates"][0]["path"],
        cfg.display().to_string()
    );
    let fingerprint = initial["fingerprint"].as_str().unwrap();

    let refused = setup_action(
        application.clone(),
        serde_json::json!({
            "action": "install_gsi",
            "expected_fingerprint": fingerprint,
            "path": cfg,
            "consent": false
        }),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(!cfg.join("gamestate_integration_openfrag.cfg").exists());

    let installed = json_response(
        setup_action(
            application.clone(),
            serde_json::json!({
                "action": "install_gsi",
                "expected_fingerprint": fingerprint,
                "path": cfg,
                "consent": true
            }),
        )
        .await,
    )
    .await;
    assert_eq!(installed["checks"][1]["status"], "ready");
    let contents = std::fs::read_to_string(cfg.join("gamestate_integration_openfrag.cfg")).unwrap();
    assert!(contents.contains("http://127.0.0.1:7130/gsi/router"));
    assert!(!contents.contains("0.0.0.0"));

    let stale = setup_action(
        application.clone(),
        serde_json::json!({
            "action": "skip",
            "step": "capture",
            "expected_fingerprint": fingerprint,
            "consent": false
        }),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let current = installed["fingerprint"].as_str().unwrap();
    let mut deferred = json_response(
        setup_action(
            application.clone(),
            serde_json::json!({
                "action": "skip",
                "step": "capture",
                "expected_fingerprint": current,
                "consent": false
            }),
        )
        .await,
    )
    .await;
    assert!(
        deferred["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| { check["id"] == "capture" && check["status"] == "not_tested" })
    );
    for step in ["local_identity", "test_capture", "manual_flag"] {
        let fingerprint = deferred["fingerprint"].as_str().unwrap();
        deferred = json_response(
            setup_action(
                application.clone(),
                serde_json::json!({
                    "action": "skip",
                    "step": step,
                    "expected_fingerprint": fingerprint,
                    "consent": false
                }),
            )
            .await,
        )
        .await;
    }
    let fingerprint = deferred["fingerprint"].as_str().unwrap();
    let completed = json_response(
        setup_action(
            application,
            serde_json::json!({
                "action": "set_background",
                "expected_fingerprint": fingerprint,
                "consent": false
            }),
        )
        .await,
    )
    .await;
    assert_eq!(completed["complete"], true);
    let state_path = home.join(".config/openfrag/setup-state.json");
    assert_eq!(
        std::fs::metadata(&state_path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let restarted = app(&AppConfig::new(&data).with_setup_home(&home)).unwrap();
    let restored = json_response(
        restarted
            .oneshot(
                Request::builder()
                    .uri("/api/setup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(restored["complete"], true);
    assert!(
        restored["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| { check["id"] == "capture" && check["status"] == "not_tested" })
    );
}

async fn setup_action(application: axum::Router, body: Value) -> axum::response::Response {
    application
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/setup/actions")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_response(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap()
}

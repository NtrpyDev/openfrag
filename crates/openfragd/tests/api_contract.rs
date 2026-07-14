use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use openfragd::api::{EmptyLocalApi, router};
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

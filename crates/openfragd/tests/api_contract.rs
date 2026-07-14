#[path = "../src/api.rs"]
mod api;
use api::{EmptyLocalApi, router};
use axum::{body::{Body, to_bytes}, http::{Request, StatusCode}};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn empty_local_api_is_honest_about_unavailable_work() {
    let app = router(Arc::new(EmptyLocalApi));
    let health = app.clone().oneshot(Request::builder().uri("/api/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(to_bytes(health.into_body(), 4096).await.unwrap(), "{\"rating_state\":\"unavailable\",\"rating\":null}");
    let flag = app.oneshot(Request::builder().method("POST").uri("/api/manual-flag").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(flag.status(), StatusCode::SERVICE_UNAVAILABLE);
}

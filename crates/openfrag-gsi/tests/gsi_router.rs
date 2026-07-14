use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use openfrag_gsi::{Clock, EventSink, EvidenceReceipt, GsiConfig, GsiService, router};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tower::ServiceExt;

const TOKEN: &str = "test-token-that-must-never-escape";
const STEAM_ID: &str = "76561198000000001";

#[derive(Default)]
struct ManualClock(AtomicU64);

impl ManualClock {
    fn advance(&self, elapsed: Duration) {
        self.0.fetch_add(
            u64::try_from(elapsed.as_millis()).expect("test duration fits in u64"),
            Ordering::SeqCst,
        );
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::SeqCst))
    }
}

#[derive(Default)]
struct RecordingSink {
    receipts: Mutex<Vec<EvidenceReceipt>>,
    failures_remaining: AtomicU64,
}

impl RecordingSink {
    fn receipts(&self) -> Vec<EvidenceReceipt> {
        self.receipts.lock().expect("receipts lock").clone()
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, receipt: EvidenceReceipt) -> Result<(), String> {
        if self
            .failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err("test sink failure".into());
        }
        self.receipts.lock().expect("receipts lock").push(receipt);
        Ok(())
    }
}

fn harness() -> (GsiService, Arc<ManualClock>, Arc<RecordingSink>) {
    let clock = Arc::new(ManualClock::default());
    let sink = Arc::new(RecordingSink::default());
    let service = GsiService::new(
        GsiConfig::new(TOKEN, STEAM_ID, Duration::from_secs(10)),
        clock.clone(),
        sink.clone(),
    );
    (service, clock, sink)
}

fn payload(token: &str, steam_id: &str) -> String {
    format!(
        r#"{{"provider":{{"appid":730,"timestamp":1}},"map":{{"name":"de_dust2","mode":"competitive","phase":"live","round":3}},"player":{{"steamid":"{steam_id}","activity":"playing","state":{{"health":100,"armor":50}},"match_stats":{{"kills":4,"deaths":2,"round_kills":1}}}},"auth":{{"token":"{token}"}}}}"#
    )
}

async fn post(service: &GsiService, content_type: &str, body: impl Into<Body>) -> axum::response::Response {
    router(service.clone())
        .oneshot(
            Request::post("/gsi")
                .header("content-type", content_type)
                .body(body.into())
                .expect("request"),
        )
        .await
        .expect("router response")
}

#[tokio::test]
async fn rejects_an_invalid_auth_token_without_emitting_evidence() {
    let (service, _clock, sink) = harness();

    let response = post(&service, "application/json", payload("wrong-token", STEAM_ID)).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(sink.receipts().is_empty());
}

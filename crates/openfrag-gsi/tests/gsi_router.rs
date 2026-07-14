use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use openfrag_gsi::{
    Clock, EventSink, EvidenceReceipt, GsiConfig, GsiService, StateOutput, router,
};
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

#[tokio::test]
async fn seeds_sanitized_evidence_and_accepts_json_content_type_parameters() {
    let (service, _clock, sink) = harness();

    let response = post(
        &service,
        "application/json; charset=utf-8",
        payload(TOKEN, STEAM_ID),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let receipts = sink.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt.sequence, 1);
    assert_eq!(receipt.received_at, Duration::ZERO);
    assert_eq!(receipt.output, StateOutput::Seeded);
    assert!(receipt.facts.is_empty());
    assert_eq!(
        receipt.payload_hash,
        "0272ef1d6ec9074916277a9d13500f4195b3c69486ae75509928666f52d724e9"
    );
    assert!(receipt.presence.provider);
    assert!(receipt.presence.provider_timestamp);
    assert!(receipt.presence.map);
    assert!(receipt.presence.map_round);
    assert!(receipt.presence.player);
    assert!(receipt.presence.player_state);
    assert!(receipt.presence.match_stats);
    assert!(receipt.presence.auth);
    assert!(receipt.presence.auth_token);
    assert!(receipt.presence.player_steamid);
    let debug_receipt = format!("{receipt:?}");
    assert!(!debug_receipt.contains(TOKEN));
    assert!(!debug_receipt.contains(STEAM_ID));
    assert!(!debug_receipt.contains("de_dust2"));
}

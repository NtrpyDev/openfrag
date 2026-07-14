use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use openfrag_gsi::{
    Clock, EventSink, EvidenceReceipt, GsiConfig, GsiService, PollOutcome, StateOutput,
    TransitionFact, router,
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

    fn fail_next(&self) {
        self.failures_remaining.store(1, Ordering::SeqCst);
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
    snapshot(token, steam_id, "de_dust2", 3, 4, 2, 100)
}

fn snapshot(
    token: &str,
    steam_id: &str,
    map: &str,
    round: u64,
    kills: i64,
    deaths: i64,
    health: i64,
) -> String {
    format!(
        r#"{{"provider":{{"appid":730,"timestamp":1}},"map":{{"name":"{map}","mode":"competitive","phase":"live","round":{round}}},"player":{{"steamid":"{steam_id}","activity":"playing","state":{{"health":{health},"armor":50}},"match_stats":{{"kills":{kills},"deaths":{deaths},"round_kills":1}}}},"auth":{{"token":"{token}"}}}}"#
    )
}

async fn post(
    service: &GsiService,
    content_type: &str,
    body: impl Into<Body>,
) -> axum::response::Response {
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

    let response = post(
        &service,
        "application/json",
        payload("wrong-token", STEAM_ID),
    )
    .await;

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

#[tokio::test]
async fn rejects_a_mismatched_local_player_identity() {
    let (service, _clock, sink) = harness();

    let response = post(&service, "application/json", payload(TOKEN, "other-player")).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(sink.receipts().is_empty());
}

#[tokio::test]
async fn rejects_oversized_json_before_the_handler_buffers_it() {
    let (service, _clock, sink) = harness();

    let response = post(
        &service,
        "application/json",
        vec![b'x'; openfrag_gsi::MAX_BODY_BYTES + 1],
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(sink.receipts().is_empty());
}

#[tokio::test]
async fn rejects_malformed_json_without_emitting_evidence() {
    let (service, _clock, sink) = harness();

    let response = post(&service, "application/json", "{").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(sink.receipts().is_empty());
}

#[tokio::test]
async fn rejects_a_non_json_content_type() {
    let (service, _clock, sink) = harness();

    let response = post(&service, "text/plain", payload(TOKEN, STEAM_ID)).await;

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(sink.receipts().is_empty());
}

#[tokio::test]
async fn treats_a_repeated_snapshot_as_idempotent() {
    let (service, _clock, sink) = harness();
    let body = payload(TOKEN, STEAM_ID);

    let first = post(&service, "application/json", body.clone()).await;
    let repeated = post(&service, "application/json", body).await;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(sink.receipts().len(), 1);
}

#[tokio::test]
async fn derives_a_trusted_kill_fact_from_consecutive_snapshots() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;

    let response = post(
        &service,
        "application/json",
        snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 5, 2, 100),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let receipts = sink.receipts();
    assert_eq!(receipts[1].output, StateOutput::Healthy);
    assert_eq!(
        receipts[1].facts,
        vec![TransitionFact::Kill {
            previous: 4,
            current: 5
        }]
    );
}

#[tokio::test]
async fn derives_a_trusted_death_fact_from_consecutive_snapshots() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;

    let response = post(
        &service,
        "application/json",
        snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 4, 3, 0),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let receipts = sink.receipts();
    assert_eq!(receipts[1].output, StateOutput::Healthy);
    assert_eq!(
        receipts[1].facts,
        vec![TransitionFact::Death {
            previous: 2,
            current: 3
        }]
    );
}

#[tokio::test]
async fn derives_a_trusted_round_end_fact_from_consecutive_snapshots() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;

    let response = post(
        &service,
        "application/json",
        snapshot(TOKEN, STEAM_ID, "de_dust2", 4, 4, 2, 100),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let receipts = sink.receipts();
    assert_eq!(receipts[1].output, StateOutput::Healthy);
    assert_eq!(
        receipts[1].facts,
        vec![TransitionFact::RoundEnd {
            completed: 3,
            next: 4
        }]
    );
}

#[tokio::test]
async fn stale_polling_emits_once_and_the_next_snapshot_recovers() {
    let (service, clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;

    clock.advance(Duration::from_secs(29));
    assert_eq!(service.poll_stale(), Ok(PollOutcome::NoChange));
    clock.advance(Duration::from_secs(1));
    assert_eq!(service.poll_stale(), Ok(PollOutcome::Stale));
    assert_eq!(service.poll_stale(), Ok(PollOutcome::NoChange));
    assert_eq!(sink.receipts()[1].output, StateOutput::Stale);

    clock.advance(Duration::from_secs(1));
    let response = post(
        &service,
        "application/json",
        snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 5, 2, 80),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let receipts = sink.receipts();
    assert_eq!(receipts[2].output, StateOutput::Recovered);
    assert!(receipts[2].facts.is_empty());
}

#[tokio::test]
async fn reports_a_session_reset_without_cross_session_transition_facts() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;

    let response = post(
        &service,
        "application/json",
        snapshot(TOKEN, STEAM_ID, "de_inferno", 1, 0, 0, 100),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let receipts = sink.receipts();
    assert_eq!(receipts[1].output, StateOutput::SessionReset);
    assert!(receipts[1].facts.is_empty());
}

#[tokio::test]
async fn sink_failure_is_diagnostic_transactional_and_does_not_stop_the_listener() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;
    sink.fail_next();
    let changed = snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 5, 2, 100);

    let failed = post(&service, "application/json", changed.clone()).await;
    let failed_body = axum::body::to_bytes(failed.into_body(), 1024)
        .await
        .expect("diagnostic body");
    assert_eq!(&failed_body[..], b"sink-failure");
    assert_eq!(sink.receipts().len(), 1);

    let retried = post(&service, "application/json", changed).await;

    assert_eq!(retried.status(), StatusCode::OK);
    let receipts = sink.receipts();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[1].sequence, 2);
    assert_eq!(
        receipts[1].facts,
        vec![TransitionFact::Kill {
            previous: 4,
            current: 5
        }]
    );
}

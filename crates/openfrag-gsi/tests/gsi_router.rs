use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use openfrag_gsi::{
    Clock, DUPLICATE_WINDOW, EventSink, EvidenceReceipt, GsiConfig, GsiService, PollOutcome,
    StateOutput, TransitionFact, router,
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
        r#"{{"provider":{{"appid":730,"timestamp":1}},"map":{{"name":"{map}","mode":"competitive","phase":"live","round":{round}}},"player":{{"steamid":"{steam_id}","activity":"playing","state":{{"health":{health},"armor":50,"round_kills":1}},"match_stats":{{"kills":{kills},"deaths":{deaths}}}}},"auth":{{"token":"{token}"}}}}"#
    )
}

fn with_round_kills(body: impl AsRef<str>, round_kills: i64) -> String {
    let mut payload: serde_json::Value = serde_json::from_str(body.as_ref()).expect("test payload");
    payload["player"]["state"]["round_kills"] = round_kills.into();
    payload.to_string()
}

fn with_weapons(body: impl AsRef<str>) -> String {
    let mut payload: serde_json::Value = serde_json::from_str(body.as_ref()).expect("test payload");
    payload["player"]["weapons"] = serde_json::json!({
        "weapon_0": { "name": "weapon_ak47", "state": "active" }
    });
    payload.to_string()
}

fn with_provider_timestamp(body: impl AsRef<str>, timestamp: i64) -> String {
    let mut payload: serde_json::Value = serde_json::from_str(body.as_ref()).expect("test payload");
    payload["provider"]["timestamp"] = timestamp.into();
    payload.to_string()
}

async fn post(
    service: &GsiService,
    content_type: &str,
    body: impl Into<Body>,
) -> axum::response::Response {
    router(service.clone())
        .oneshot(
            Request::post("/gsi/router")
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
        "927150fcbff815195a283cc526feefaf26fee4cac9e5105433223958b1476012"
    );
    assert!(receipt.presence.provider);
    assert!(receipt.presence.provider_timestamp);
    assert!(receipt.presence.map);
    assert!(receipt.presence.map_round);
    assert!(receipt.presence.player);
    assert!(receipt.presence.player_state);
    assert!(receipt.presence.player_health);
    assert!(receipt.presence.player_round_kills);
    assert!(!receipt.presence.player_weapons);
    assert!(receipt.presence.match_stats);
    assert!(receipt.presence.auth);
    assert!(receipt.presence.auth_token);
    assert!(receipt.presence.player_steamid);
    let debug_receipt = format!("{receipt:?}");
    assert!(!debug_receipt.contains(TOKEN));
    assert!(!debug_receipt.contains(STEAM_ID));
    assert!(!debug_receipt.contains("de_dust2"));
    assert_eq!(
        receipt.context.map_hash.as_deref(),
        Some("f99f33e2882aa01d4d3060562bd3f088bc74086e344c1dbbf1862d0239ef4954")
    );
    assert_eq!(receipt.context.observed_round, Some(3));
    assert_eq!(receipt.context.provider_timestamp, Some(1));
    assert_eq!(receipt.context.round_kills, Some(1));
    assert_eq!(receipt.context.health, Some(100));
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
        vec![TransitionFact::CumulativeKillDelta {
            previous: 4,
            current: 5,
            delta: 1,
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
        snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 4, 3, 100),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let receipts = sink.receipts();
    assert_eq!(receipts[1].output, StateOutput::Healthy);
    assert_eq!(
        receipts[1].facts,
        vec![TransitionFact::CumulativeDeathDelta {
            previous: 2,
            current: 3,
            delta: 1,
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
        vec![TransitionFact::CumulativeKillDelta {
            previous: 4,
            current: 5,
            delta: 1,
        }]
    );
}

#[tokio::test]
async fn records_weapons_presence_without_retaining_weapon_payloads() {
    let (service, _clock, sink) = harness();

    post(
        &service,
        "application/json",
        with_weapons(payload(TOKEN, STEAM_ID)),
    )
    .await;

    let receipt = &sink.receipts()[0];
    assert!(receipt.presence.player_weapons);
    assert!(!format!("{receipt:?}").contains("weapon_ak47"));
}

#[tokio::test]
async fn round_kill_delta_exposes_the_current_three_kill_rule_threshold() {
    let (service, _clock, sink) = harness();
    post(
        &service,
        "application/json",
        with_round_kills(payload(TOKEN, STEAM_ID), 2),
    )
    .await;

    post(
        &service,
        "application/json",
        with_round_kills(payload(TOKEN, STEAM_ID), 3),
    )
    .await;

    let receipt = &sink.receipts()[1];
    assert_eq!(receipt.context.round_kills, Some(3));
    assert_eq!(
        receipt.facts,
        vec![TransitionFact::RoundKillDelta {
            previous: 2,
            current: 3,
            delta: 1,
        }]
    );
}

#[tokio::test]
async fn round_kill_reset_seeds_the_new_round_without_a_false_kill() {
    let (service, _clock, sink) = harness();
    post(
        &service,
        "application/json",
        with_round_kills(payload(TOKEN, STEAM_ID), 3),
    )
    .await;
    post(
        &service,
        "application/json",
        with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 4, 4, 2, 100), 0),
    )
    .await;
    post(
        &service,
        "application/json",
        with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 4, 4, 2, 100), 1),
    )
    .await;

    let receipts = sink.receipts();
    assert_eq!(
        receipts[1].facts,
        vec![TransitionFact::RoundEnd {
            completed: 3,
            next: 4,
        }]
    );
    assert_eq!(
        receipts[2].facts,
        vec![TransitionFact::RoundKillDelta {
            previous: 0,
            current: 1,
            delta: 1,
        }]
    );
}

#[tokio::test]
async fn emits_health_depleted_only_on_a_positive_to_zero_transition() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;

    post(
        &service,
        "application/json",
        snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 4, 2, 0),
    )
    .await;

    let receipt = &sink.receipts()[1];
    assert_eq!(receipt.context.health, Some(0));
    assert_eq!(
        receipt.facts,
        vec![TransitionFact::HealthDepleted {
            previous: 100,
            current: 0,
        }]
    );
}

#[tokio::test]
async fn multi_deltas_are_one_co_observed_batch_without_invented_event_order() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;

    post(
        &service,
        "application/json",
        with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 6, 3, 0), 3),
    )
    .await;

    assert_eq!(
        sink.receipts()[1].facts,
        vec![
            TransitionFact::CumulativeKillDelta {
                previous: 4,
                current: 6,
                delta: 2,
            },
            TransitionFact::CumulativeDeathDelta {
                previous: 2,
                current: 3,
                delta: 1,
            },
            TransitionFact::RoundKillDelta {
                previous: 1,
                current: 3,
                delta: 2,
            },
            TransitionFact::HealthDepleted {
                previous: 100,
                current: 0,
            },
        ]
    );
}

#[tokio::test]
async fn identical_payload_is_accepted_again_after_the_duplicate_window() {
    let (service, clock, sink) = harness();
    let body = payload(TOKEN, STEAM_ID);
    post(&service, "application/json", body.clone()).await;
    clock.advance(DUPLICATE_WINDOW + Duration::from_millis(1));

    post(&service, "application/json", body).await;

    let receipts = sink.receipts();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[1].sequence, 2);
    assert_eq!(receipts[1].output, StateOutput::Healthy);
}

#[tokio::test]
async fn distinct_snapshots_with_equal_provider_timestamps_keep_arrival_order() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;
    post(
        &service,
        "application/json",
        snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 4, 2, 90),
    )
    .await;

    let receipts = sink.receipts();
    assert_eq!(receipts[0].context.provider_timestamp, Some(1));
    assert_eq!(receipts[1].context.provider_timestamp, Some(1));
    assert_eq!((receipts[0].sequence, receipts[1].sequence), (1, 2));
    assert_eq!(receipts[1].context.health, Some(90));
}

#[tokio::test]
async fn valid_token_identity_mismatch_clears_transition_baselines() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;
    let mismatch = with_round_kills(
        snapshot(TOKEN, "other-player", "de_dust2", 3, 99, 2, 100),
        99,
    );
    assert_eq!(
        post(&service, "application/json", mismatch).await.status(),
        StatusCode::UNAUTHORIZED
    );

    post(
        &service,
        "application/json",
        with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 5, 2, 100), 2),
    )
    .await;

    let receipts = sink.receipts();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[1].output, StateOutput::Seeded);
    assert!(receipts[1].facts.is_empty());
}

#[tokio::test]
async fn invalid_token_cannot_reset_a_valid_transition_baseline() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;
    post(
        &service,
        "application/json",
        snapshot("attacker", "other-player", "de_dust2", 3, 99, 2, 100),
    )
    .await;

    post(
        &service,
        "application/json",
        with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 5, 2, 100), 2),
    )
    .await;

    assert_eq!(
        sink.receipts()[1].facts,
        vec![
            TransitionFact::CumulativeKillDelta {
                previous: 4,
                current: 5,
                delta: 1,
            },
            TransitionFact::RoundKillDelta {
                previous: 1,
                current: 2,
                delta: 1,
            },
        ]
    );
}

#[tokio::test]
async fn recovery_reseeds_before_later_round_kill_progress() {
    let (service, clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;
    clock.advance(Duration::from_secs(30));
    assert_eq!(service.poll_stale(), Ok(PollOutcome::Stale));
    let recovered = with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 5, 2, 80), 2);
    post(&service, "application/json", recovered.clone()).await;
    post(&service, "application/json", recovered).await;
    post(
        &service,
        "application/json",
        with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 6, 2, 80), 3),
    )
    .await;

    let receipts = sink.receipts();
    assert_eq!(receipts.len(), 4);
    assert_eq!(receipts[2].output, StateOutput::Recovered);
    assert!(receipts[2].facts.is_empty());
    assert_eq!(
        receipts[3].facts,
        vec![
            TransitionFact::CumulativeKillDelta {
                previous: 5,
                current: 6,
                delta: 1,
            },
            TransitionFact::RoundKillDelta {
                previous: 2,
                current: 3,
                delta: 1,
            },
        ]
    );
}

#[tokio::test]
async fn round_jump_records_each_implied_round_end() {
    let (service, _clock, sink) = harness();
    post(&service, "application/json", payload(TOKEN, STEAM_ID)).await;

    post(
        &service,
        "application/json",
        with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 5, 4, 2, 100), 0),
    )
    .await;

    assert_eq!(
        sink.receipts()[1].facts,
        vec![
            TransitionFact::RoundEnd {
                completed: 3,
                next: 4,
            },
            TransitionFact::RoundEnd {
                completed: 4,
                next: 5,
            },
        ]
    );
}

#[tokio::test]
async fn detected_provider_session_restart_reseeds_all_transition_baselines() {
    let (service, _clock, sink) = harness();
    post(
        &service,
        "application/json",
        with_provider_timestamp(payload(TOKEN, STEAM_ID), 100),
    )
    .await;
    post(
        &service,
        "application/json",
        with_provider_timestamp(
            with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 10, 2, 100), 0),
            1,
        ),
    )
    .await;
    post(
        &service,
        "application/json",
        with_provider_timestamp(
            with_round_kills(snapshot(TOKEN, STEAM_ID, "de_dust2", 3, 11, 2, 100), 1),
            2,
        ),
    )
    .await;

    let receipts = sink.receipts();
    assert_eq!(receipts[1].output, StateOutput::SessionReset);
    assert!(receipts[1].facts.is_empty());
    assert_eq!(
        receipts[2].facts,
        vec![
            TransitionFact::CumulativeKillDelta {
                previous: 10,
                current: 11,
                delta: 1,
            },
            TransitionFact::RoundKillDelta {
                previous: 0,
                current: 1,
                delta: 1,
            },
        ]
    );
}

#[tokio::test]
async fn session_restart_clears_recent_hashes_from_the_previous_session() {
    let (service, _clock, sink) = harness();
    let original = with_provider_timestamp(payload(TOKEN, STEAM_ID), 100);
    post(&service, "application/json", original.clone()).await;
    post(
        &service,
        "application/json",
        with_provider_timestamp(snapshot(TOKEN, STEAM_ID, "de_inferno", 1, 0, 0, 100), 1),
    )
    .await;

    post(&service, "application/json", original).await;

    let receipts = sink.receipts();
    assert_eq!(receipts.len(), 3);
    assert_eq!(receipts[1].output, StateOutput::SessionReset);
    assert_eq!(receipts[2].output, StateOutput::SessionReset);
    assert!(receipts[2].facts.is_empty());
}

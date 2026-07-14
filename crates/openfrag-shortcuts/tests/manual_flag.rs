use async_trait::async_trait;
use openfrag_shortcuts::{
    MANUAL_FLAG_ID, ManualFlagService, ManualFlagSink, OpenedSession, PollOutcome, PortalBackend,
    PortalSignal, RestoreToken, RestoreTokenStore, SessionHandle, ShortcutDiagnostic,
    ShortcutRequest,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct FakePortal {
    opens: Mutex<Vec<(Option<RestoreToken>, Vec<ShortcutRequest>)>>,
    open_results: Mutex<VecDeque<Result<OpenedSession, ShortcutDiagnostic>>>,
    signals: Mutex<VecDeque<Result<PortalSignal, ShortcutDiagnostic>>>,
    closes: Mutex<Vec<SessionHandle>>,
}

impl FakePortal {
    fn push_signal(&self, signal: PortalSignal) {
        self.signals.lock().expect("signals lock").push_back(Ok(signal));
    }

    fn push_open_error(&self, error: ShortcutDiagnostic) {
        self.open_results
            .lock()
            .expect("open results lock")
            .push_back(Err(error));
    }

    fn opens(&self) -> Vec<(Option<RestoreToken>, Vec<ShortcutRequest>)> {
        self.opens.lock().expect("opens lock").clone()
    }
}

#[async_trait]
impl PortalBackend for FakePortal {
    async fn open(
        &self,
        restore_token: Option<&RestoreToken>,
        shortcuts: &[ShortcutRequest],
    ) -> Result<OpenedSession, ShortcutDiagnostic> {
        self.opens
            .lock()
            .expect("opens lock")
            .push((restore_token.cloned(), shortcuts.to_vec()));
        if let Some(result) = self
            .open_results
            .lock()
            .expect("open results lock")
            .pop_front()
        {
            return result;
        }
        let index = self.opens.lock().expect("opens lock").len();
        Ok(OpenedSession {
            session: SessionHandle::new(format!("session-{index}")),
            restore_token: restore_token
                .cloned()
                .unwrap_or_else(|| RestoreToken::new("restore-1")),
        })
    }

    async fn next_signal(
        &self,
        _session: &SessionHandle,
    ) -> Result<PortalSignal, ShortcutDiagnostic> {
        self.signals
            .lock()
            .expect("signals lock")
            .pop_front()
            .expect("test queued a signal")
    }

    async fn close(&self, session: &SessionHandle) -> Result<(), ShortcutDiagnostic> {
        self.closes.lock().expect("closes lock").push(session.clone());
        Ok(())
    }
}

#[derive(Default)]
struct MemoryStore {
    token: Mutex<Option<RestoreToken>>,
    saves: Mutex<Vec<RestoreToken>>,
}

impl RestoreTokenStore for MemoryStore {
    fn load(&self) -> Result<Option<RestoreToken>, ShortcutDiagnostic> {
        Ok(self.token.lock().expect("token lock").clone())
    }

    fn save(&self, token: &RestoreToken) -> Result<(), ShortcutDiagnostic> {
        *self.token.lock().expect("token lock") = Some(token.clone());
        self.saves.lock().expect("saves lock").push(token.clone());
        Ok(())
    }
}

#[derive(Default)]
struct RecordingSink(Mutex<u64>);

impl ManualFlagSink for RecordingSink {
    fn manual_flag(&self) -> Result<(), ShortcutDiagnostic> {
        *self.0.lock().expect("sink lock") += 1;
        Ok(())
    }
}

fn harness() -> (
    ManualFlagService<FakePortal, RecordingSink, MemoryStore>,
    Arc<FakePortal>,
    Arc<RecordingSink>,
    Arc<MemoryStore>,
) {
    let portal = Arc::new(FakePortal::default());
    let sink = Arc::new(RecordingSink::default());
    let store = Arc::new(MemoryStore::default());
    let service = ManualFlagService::new(
        portal.clone(),
        sink.clone(),
        store.clone(),
        Some("CTRL+ALT+F10".into()),
    );
    (service, portal, sink, store)
}

#[tokio::test]
async fn creates_one_user_approved_manual_flag_shortcut_and_persists_only_its_token() {
    let (mut service, portal, _sink, store) = harness();

    service.start().await.expect("shortcut starts");

    let opens = portal.opens();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0].0, None);
    assert_eq!(opens[0].1.len(), 1);
    assert_eq!(opens[0].1[0].id, MANUAL_FLAG_ID);
    assert_eq!(opens[0].1[0].description, "Save the preceding play");
    assert_eq!(opens[0].1[0].preferred_trigger.as_deref(), Some("CTRL+ALT+F10"));
    assert_eq!(
        store.saves.lock().expect("saves lock").as_slice(),
        &[RestoreToken::new("restore-1")]
    );
}

#[tokio::test]
async fn restores_the_persisted_session_token() {
    let (mut service, portal, _sink, store) = harness();
    *store.token.lock().expect("token lock") = Some(RestoreToken::new("existing-token"));

    service.start().await.expect("shortcut restores");

    assert_eq!(portal.opens()[0].0, Some(RestoreToken::new("existing-token")));
    assert!(store.saves.lock().expect("saves lock").is_empty());
}

#[tokio::test]
async fn emits_once_while_held_and_rearms_on_deactivation() {
    let (mut service, portal, sink, _store) = harness();
    service.start().await.expect("shortcut starts");
    portal.push_signal(PortalSignal::Activated(MANUAL_FLAG_ID.into()));
    portal.push_signal(PortalSignal::Activated(MANUAL_FLAG_ID.into()));
    portal.push_signal(PortalSignal::Deactivated(MANUAL_FLAG_ID.into()));
    portal.push_signal(PortalSignal::Activated(MANUAL_FLAG_ID.into()));

    assert_eq!(service.poll().await, Ok(PollOutcome::Flagged));
    assert_eq!(service.poll().await, Ok(PollOutcome::Ignored));
    assert_eq!(service.poll().await, Ok(PollOutcome::Released));
    assert_eq!(service.poll().await, Ok(PollOutcome::Flagged));
    assert_eq!(*sink.0.lock().expect("sink lock"), 2);
}

#[tokio::test]
async fn reconnects_with_the_restore_token_after_portal_or_session_loss() {
    let (mut service, portal, _sink, _store) = harness();
    service.start().await.expect("shortcut starts");
    portal.push_signal(PortalSignal::PortalLost);

    assert_eq!(service.poll().await, Ok(PollOutcome::Reconnected));
    assert_eq!(portal.opens().len(), 2);
    assert_eq!(portal.opens()[1].0, Some(RestoreToken::new("restore-1")));

    portal.push_signal(PortalSignal::SessionLost);
    assert_eq!(service.poll().await, Ok(PollOutcome::Reconnected));
    assert_eq!(portal.opens().len(), 3);
    assert_eq!(portal.opens()[2].0, Some(RestoreToken::new("restore-1")));
}

#[tokio::test]
async fn closes_the_live_session_cleanly_without_deleting_the_restore_token() {
    let (mut service, portal, _sink, store) = harness();
    service.start().await.expect("shortcut starts");

    service.shutdown().await.expect("shortcut shuts down");

    assert_eq!(portal.closes.lock().expect("closes lock").as_slice(), &[SessionHandle::new("session-1")]);
    assert_eq!(store.load().expect("token loads"), Some(RestoreToken::new("restore-1")));
}

#[tokio::test]
async fn exposes_typed_portal_diagnostics() {
    for expected in [
        ShortcutDiagnostic::Unavailable,
        ShortcutDiagnostic::Conflict,
        ShortcutDiagnostic::PermissionDenied,
        ShortcutDiagnostic::SessionLost,
    ] {
        let (mut service, portal, _sink, _store) = harness();
        portal.push_open_error(expected.clone());
        assert_eq!(service.start().await, Err(expected));
    }
}

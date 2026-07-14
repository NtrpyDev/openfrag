//! Linux adapter for `org.freedesktop.portal.GlobalShortcuts` version 1+.
//!
//! Global Shortcuts v2 has no restore-token option. A new session discovers
//! application bindings through `ListShortcuts`; `BindShortcuts` is called
//! exactly once only when the Manual Flag shortcut is not already present.

use crate::{
    OpenedSession, PortalBackend, PortalSignal, RestoreToken, SessionHandle, ShortcutDiagnostic,
    ShortcutRequest,
};
use ashpd::{
    Error as AshpdError, PortalError,
    desktop::{
        CreateSessionOptions, ResponseError, Session,
        global_shortcuts::{
            Activated, BindShortcutsOptions, Deactivated, GlobalShortcuts, ListShortcutsOptions,
            NewShortcut,
        },
    },
};
use async_trait::async_trait;
use futures_util::{StreamExt, stream::BoxStream};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use tokio::{sync::Mutex, task::JoinHandle};

struct ActiveSession {
    handle: SessionHandle,
    portal_session: Arc<Session<GlobalShortcuts>>,
    signals: tokio::sync::mpsc::Receiver<PortalSignal>,
    signal_task: JoinHandle<()>,
}

pub struct LinuxPortalBackend {
    active: Mutex<Option<ActiveSession>>,
    next_token: AtomicU64,
}

impl Default for LinuxPortalBackend {
    fn default() -> Self {
        Self {
            active: Mutex::new(None),
            next_token: AtomicU64::new(1),
        }
    }
}

impl LinuxPortalBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    async fn discard_previous(&self) {
        if let Some(previous) = self.active.lock().await.take() {
            previous.signal_task.abort();
            let _ = previous.portal_session.close().await;
        }
    }

    fn generated_restore_token(&self) -> RestoreToken {
        let ordinal = self.next_token.fetch_add(1, Ordering::Relaxed);
        RestoreToken::new(format!("openfrag_{}_{}", std::process::id(), ordinal))
    }
}

#[async_trait]
impl PortalBackend for LinuxPortalBackend {
    async fn open(
        &self,
        restore_token: Option<&RestoreToken>,
        shortcuts: &[ShortcutRequest],
    ) -> Result<OpenedSession, ShortcutDiagnostic> {
        let [requested] = shortcuts else {
            return Err(ShortcutDiagnostic::Portal(
                "the Linux adapter requires exactly one shortcut".into(),
            ));
        };
        self.discard_previous().await;

        let portal = GlobalShortcuts::new().await.map_err(map_open_error)?;
        if portal.version() < 1 {
            return Err(ShortcutDiagnostic::Unavailable);
        }
        let activated = portal.receive_activated().await.map_err(map_open_error)?;
        let deactivated = portal
            .receive_deactivated()
            .await
            .map_err(map_open_error)?;
        let portal_session = Arc::new(
            portal
                .create_session(CreateSessionOptions::default())
                .await
                .map_err(map_open_error)?,
        );

        let listed = portal
            .list_shortcuts(&portal_session, ListShortcutsOptions::default())
            .await
            .map_err(map_open_error)?
            .response()
            .map_err(map_open_error)?;
        if !listed
            .shortcuts()
            .iter()
            .any(|shortcut| shortcut.id() == requested.id)
        {
            let shortcut = NewShortcut::new(&requested.id, &requested.description)
                .preferred_trigger(requested.preferred_trigger.as_deref());
            let bound = portal
                .bind_shortcuts(
                    &portal_session,
                    &[shortcut],
                    None,
                    BindShortcutsOptions::default(),
                )
                .await
                .map_err(map_open_error)?
                .response()
                .map_err(map_open_error)?;
            if !bound
                .shortcuts()
                .iter()
                .any(|shortcut| shortcut.id() == requested.id)
            {
                return Err(ShortcutDiagnostic::PermissionDenied);
            }
        }

        let ordinal = self.next_token.fetch_add(1, Ordering::Relaxed);
        let handle = SessionHandle::new(format!("linux-session-{ordinal}"));
        let token = restore_token
            .cloned()
            .unwrap_or_else(|| self.generated_restore_token());
        let (sender, signals) = tokio::sync::mpsc::channel(16);
        let signal_task = tokio::spawn(forward_signals(
            portal_session.clone(),
            activated.boxed(),
            deactivated.boxed(),
            sender,
        ));
        *self.active.lock().await = Some(ActiveSession {
            handle: handle.clone(),
            portal_session,
            signals,
            signal_task,
        });
        Ok(OpenedSession {
            session: handle,
            restore_token: token,
        })
    }

    async fn next_signal(
        &self,
        session: &SessionHandle,
    ) -> Result<PortalSignal, ShortcutDiagnostic> {
        let mut active = self.active.lock().await;
        let active = active
            .as_mut()
            .filter(|active| active.handle == *session)
            .ok_or(ShortcutDiagnostic::SessionLost)?;
        active
            .signals
            .recv()
            .await
            .ok_or(ShortcutDiagnostic::SessionLost)
    }

    async fn close(&self, session: &SessionHandle) -> Result<(), ShortcutDiagnostic> {
        let mut active = self.active.lock().await;
        let Some(active_session) = active.take() else {
            return Ok(());
        };
        if active_session.handle != *session {
            *active = Some(active_session);
            return Err(ShortcutDiagnostic::SessionLost);
        }
        drop(active);
        active_session.signal_task.abort();
        active_session
            .portal_session
            .close()
            .await
            .map_err(map_session_error)
    }
}

async fn forward_signals(
    portal_session: Arc<Session<GlobalShortcuts>>,
    mut activated: BoxStream<'static, Activated>,
    mut deactivated: BoxStream<'static, Deactivated>,
    sender: tokio::sync::mpsc::Sender<PortalSignal>,
) {
    let Ok(mut closed) = portal_session.receive_closed().await else {
        let _ = sender.send(PortalSignal::PortalLost).await;
        return;
    };
    loop {
        let signal = tokio::select! {
            activation = activated.next() => activation
                .map(|activation| PortalSignal::Activated(activation.shortcut_id().into())),
            deactivation = deactivated.next() => deactivation
                .map(|deactivation| PortalSignal::Deactivated(deactivation.shortcut_id().into())),
            session_closed = closed.next() => {
                let _ = session_closed;
                Some(PortalSignal::SessionLost)
            },
        };
        let Some(signal) = signal else {
            let _ = sender.send(PortalSignal::PortalLost).await;
            return;
        };
        if sender.send(signal).await.is_err() {
            return;
        }
    }
}

fn map_open_error(error: AshpdError) -> ShortcutDiagnostic {
    match error {
        AshpdError::PortalNotFound(_) | AshpdError::RequiresVersion(_, _) | AshpdError::Zbus(_) => {
            ShortcutDiagnostic::Unavailable
        }
        AshpdError::Portal(PortalError::Exist(_)) => ShortcutDiagnostic::Conflict,
        AshpdError::Portal(PortalError::NotAllowed(_) | PortalError::Cancelled(_))
        | AshpdError::Response(ResponseError::Cancelled) => ShortcutDiagnostic::PermissionDenied,
        AshpdError::NoResponse => ShortcutDiagnostic::SessionLost,
        other => ShortcutDiagnostic::Portal(other.to_string()),
    }
}

fn map_session_error(error: AshpdError) -> ShortcutDiagnostic {
    match error {
        AshpdError::NoResponse | AshpdError::Zbus(_) => ShortcutDiagnostic::SessionLost,
        other => map_open_error(other),
    }
}

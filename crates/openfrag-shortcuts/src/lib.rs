#![forbid(unsafe_code)]

use async_trait::async_trait;
use std::{fmt, sync::Arc};

pub mod linux_portal;

pub const MANUAL_FLAG_ID: &str = "openfrag.manual-flag";
const MANUAL_FLAG_DESCRIPTION: &str = "Save the preceding play";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreToken(String);

impl RestoreToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandle(String);

impl SessionHandle {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutRequest {
    pub id: String,
    pub description: String,
    pub preferred_trigger: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedSession {
    pub session: SessionHandle,
    pub restore_token: RestoreToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalSignal {
    Activated(String),
    Deactivated(String),
    SessionLost,
    PortalLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    Flagged,
    Released,
    Ignored,
    Reconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutDiagnostic {
    Unavailable,
    Conflict,
    PermissionDenied,
    SessionLost,
    Persistence(String),
    Sink(String),
    Portal(String),
}

impl fmt::Display for ShortcutDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("global shortcuts portal unavailable"),
            Self::Conflict => formatter.write_str("manual flag shortcut conflicts"),
            Self::PermissionDenied => formatter.write_str("global shortcut permission denied"),
            Self::SessionLost => formatter.write_str("global shortcut session lost"),
            Self::Persistence(message) => write!(formatter, "restore token persistence: {message}"),
            Self::Sink(message) => write!(formatter, "manual flag sink: {message}"),
            Self::Portal(message) => write!(formatter, "global shortcuts portal: {message}"),
        }
    }
}

impl std::error::Error for ShortcutDiagnostic {}

#[async_trait]
pub trait PortalBackend: Send + Sync {
    async fn open(
        &self,
        restore_token: Option<&RestoreToken>,
        shortcuts: &[ShortcutRequest],
    ) -> Result<OpenedSession, ShortcutDiagnostic>;

    async fn next_signal(
        &self,
        session: &SessionHandle,
    ) -> Result<PortalSignal, ShortcutDiagnostic>;

    async fn close(&self, session: &SessionHandle) -> Result<(), ShortcutDiagnostic>;
}

pub trait ManualFlagSink: Send + Sync {
    fn manual_flag(&self) -> Result<(), ShortcutDiagnostic>;
}

pub trait RestoreTokenStore: Send + Sync {
    fn load(&self) -> Result<Option<RestoreToken>, ShortcutDiagnostic>;
    fn save(&self, token: &RestoreToken) -> Result<(), ShortcutDiagnostic>;
}

pub struct ManualFlagService<B, S, T> {
    backend: Arc<B>,
    sink: Arc<S>,
    store: Arc<T>,
    preferred_trigger: Option<String>,
    session: Option<SessionHandle>,
    restore_token: Option<RestoreToken>,
    held: bool,
}

impl<B, S, T> ManualFlagService<B, S, T>
where
    B: PortalBackend,
    S: ManualFlagSink,
    T: RestoreTokenStore,
{
    #[must_use]
    pub fn new(
        backend: Arc<B>,
        sink: Arc<S>,
        store: Arc<T>,
        preferred_trigger: Option<String>,
    ) -> Self {
        Self {
            backend,
            sink,
            store,
            preferred_trigger,
            session: None,
            restore_token: None,
            held: false,
        }
    }

    /// Opens or restores the single Manual Flag shortcut session.
    ///
    /// # Errors
    ///
    /// Returns a typed portal or restore-token diagnostic.
    pub async fn start(&mut self) -> Result<(), ShortcutDiagnostic> {
        if self.session.is_some() {
            return Ok(());
        }
        let restore_token = self.store.load()?;
        self.open(restore_token.as_ref()).await
    }

    /// Consumes one portal signal and applies the Manual Flag hold state.
    ///
    /// # Errors
    ///
    /// Returns a typed portal, persistence, or Manual Flag sink diagnostic.
    pub async fn poll(&mut self) -> Result<PollOutcome, ShortcutDiagnostic> {
        let Some(session) = self.session.clone() else {
            return Err(ShortcutDiagnostic::SessionLost);
        };
        let signal = match self.backend.next_signal(&session).await {
            Ok(signal) => signal,
            Err(ShortcutDiagnostic::SessionLost) => return self.reconnect().await,
            Err(error) => return Err(error),
        };
        match signal {
            PortalSignal::PortalLost | PortalSignal::SessionLost => self.reconnect().await,
            PortalSignal::Activated(id) if id == MANUAL_FLAG_ID => {
                if self.held {
                    return Ok(PollOutcome::Ignored);
                }
                self.sink.manual_flag()?;
                self.held = true;
                Ok(PollOutcome::Flagged)
            }
            PortalSignal::Deactivated(id) if id == MANUAL_FLAG_ID => {
                if !self.held {
                    return Ok(PollOutcome::Ignored);
                }
                self.held = false;
                Ok(PollOutcome::Released)
            }
            PortalSignal::Activated(_) | PortalSignal::Deactivated(_) => {
                Ok(PollOutcome::Ignored)
            }
        }
    }

    /// Closes the portal session, which unregisters all shortcuts in it.
    ///
    /// # Errors
    ///
    /// Returns a typed portal diagnostic if the session cannot be closed.
    pub async fn shutdown(&mut self) -> Result<(), ShortcutDiagnostic> {
        self.held = false;
        if let Some(session) = self.session.take() {
            self.backend.close(&session).await?;
        }
        Ok(())
    }

    async fn reconnect(&mut self) -> Result<PollOutcome, ShortcutDiagnostic> {
        self.held = false;
        self.session = None;
        let token = self.restore_token.clone();
        self.open(token.as_ref()).await?;
        Ok(PollOutcome::Reconnected)
    }

    async fn open(
        &mut self,
        restore_token: Option<&RestoreToken>,
    ) -> Result<(), ShortcutDiagnostic> {
        let shortcuts = [ShortcutRequest {
            id: MANUAL_FLAG_ID.into(),
            description: MANUAL_FLAG_DESCRIPTION.into(),
            preferred_trigger: self.preferred_trigger.clone(),
        }];
        let opened = self.backend.open(restore_token, &shortcuts).await?;
        if restore_token != Some(&opened.restore_token) {
            self.store.save(&opened.restore_token)?;
        }
        self.restore_token = Some(opened.restore_token);
        self.session = Some(opened.session);
        Ok(())
    }
}

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStepId {
    Storage,
    LocalSteamIdentity,
    GsiConfig,
    CaptureRecorder,
    Ffprobe,
    TestCapture,
    LocalDemoValidation,
    ManualFlag,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStepStatus {
    Ready,
    Blocked,
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetupFact {
    Ready {
        summary: String,
    },
    Blocked {
        summary: String,
        remediation: String,
    },
}

impl SetupFact {
    #[must_use]
    pub fn ready(summary: impl Into<String>) -> Self {
        Self::Ready {
            summary: summary.into(),
        }
    }

    #[must_use]
    pub fn blocked(summary: impl Into<String>, remediation: impl Into<String>) -> Self {
        Self::Blocked {
            summary: summary.into(),
            remediation: remediation.into(),
        }
    }

    fn summary(&self) -> &str {
        match self {
            Self::Ready { summary } | Self::Blocked { summary, .. } => summary,
        }
    }

    const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    fn remediation(&self) -> Option<&str> {
        match self {
            Self::Ready { .. } => None,
            Self::Blocked { remediation, .. } => Some(remediation),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupFacts {
    pub storage: SetupFact,
    pub local_steam_identity: SetupFact,
    pub gsi_config: SetupFact,
    pub capture_recorder: SetupFact,
    pub ffprobe: SetupFact,
    pub test_capture: SetupFact,
    pub local_demo_validation: SetupFact,
    pub manual_flag: SetupFact,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SetupStep {
    pub id: SetupStepId,
    pub status: SetupStepStatus,
    pub summary: String,
    pub remediation: Option<String>,
    pub blocked_by: Vec<SetupStepId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcludedCapabilityId {
    SteamSignIn,
    ShareCodes,
    GameCoordinator,
    AutomaticDemoDiscovery,
    PublicWebsite,
    UiOrProcessSideEffects,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExcludedCapability {
    pub id: ExcludedCapabilityId,
    pub statement: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SetupFlow {
    pub steps: Vec<SetupStep>,
    pub excluded_capabilities: Vec<ExcludedCapability>,
}

impl SetupFlow {
    #[must_use]
    pub fn step(&self, id: SetupStepId) -> Option<&SetupStep> {
        self.steps.iter().find(|step| step.id == id)
    }

    #[must_use]
    pub fn ready(&self) -> bool {
        self.step(SetupStepId::Storage)
            .is_some_and(|step| step.status == SetupStepStatus::Ready)
    }
}

/// Evaluates the complete v1 setup flow without filesystem, UI, or process side effects.
#[must_use]
pub fn evaluate_setup_flow(facts: &SetupFacts) -> SetupFlow {
    let mut steps = Vec::with_capacity(8);
    push_step(&mut steps, SetupStepId::Storage, &facts.storage, &[]);
    push_step(
        &mut steps,
        SetupStepId::GsiConfig,
        &facts.gsi_config,
        &[SetupStepId::Storage],
    );
    push_step(
        &mut steps,
        SetupStepId::LocalSteamIdentity,
        &facts.local_steam_identity,
        &[SetupStepId::Storage],
    );
    push_step(
        &mut steps,
        SetupStepId::CaptureRecorder,
        &facts.capture_recorder,
        &[SetupStepId::Storage],
    );
    push_step(&mut steps, SetupStepId::Ffprobe, &facts.ffprobe, &[]);
    push_step(
        &mut steps,
        SetupStepId::TestCapture,
        &facts.test_capture,
        &[
            SetupStepId::Storage,
            SetupStepId::CaptureRecorder,
            SetupStepId::Ffprobe,
        ],
    );
    push_step(
        &mut steps,
        SetupStepId::LocalDemoValidation,
        &facts.local_demo_validation,
        &[SetupStepId::Storage],
    );
    push_step(
        &mut steps,
        SetupStepId::ManualFlag,
        &facts.manual_flag,
        &[SetupStepId::TestCapture],
    );
    SetupFlow {
        steps,
        excluded_capabilities: excluded_capabilities(),
    }
}

fn push_step(
    steps: &mut Vec<SetupStep>,
    id: SetupStepId,
    fact: &SetupFact,
    dependencies: &[SetupStepId],
) {
    let blocked_by = dependencies
        .iter()
        .copied()
        .filter(|dependency| {
            steps
                .iter()
                .find(|step| step.id == *dependency)
                .is_some_and(|step| step.status != SetupStepStatus::Ready)
        })
        .collect::<Vec<_>>();
    let (status, remediation) = if blocked_by.is_empty() {
        if fact.is_ready() {
            (SetupStepStatus::Ready, None)
        } else {
            (
                SetupStepStatus::Blocked,
                fact.remediation().map(str::to_owned),
            )
        }
    } else {
        let remediation = blocked_by.iter().find_map(|dependency| {
            steps
                .iter()
                .find(|step| step.id == *dependency)
                .and_then(|step| step.remediation.clone())
        });
        (SetupStepStatus::Skipped, remediation)
    };
    steps.push(SetupStep {
        id,
        status,
        summary: fact.summary().to_owned(),
        remediation,
        blocked_by,
    });
}

fn excluded_capabilities() -> Vec<ExcludedCapability> {
    [
        (
            ExcludedCapabilityId::SteamSignIn,
            "Setup never asks for or performs Steam sign-in.",
        ),
        (
            ExcludedCapabilityId::ShareCodes,
            "Setup does not request or process Share Codes.",
        ),
        (
            ExcludedCapabilityId::GameCoordinator,
            "Setup does not connect to the Steam Game Coordinator.",
        ),
        (
            ExcludedCapabilityId::AutomaticDemoDiscovery,
            "Setup does not discover or download Demos automatically.",
        ),
        (
            ExcludedCapabilityId::PublicWebsite,
            "Setup does not require or publish a public website.",
        ),
        (
            ExcludedCapabilityId::UiOrProcessSideEffects,
            "Flow evaluation does not open UI or launch processes.",
        ),
    ]
    .into_iter()
    .map(|(id, statement)| ExcludedCapability {
        id,
        statement: statement.to_owned(),
    })
    .collect()
}

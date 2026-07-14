use openfrag_setup::{
    ExcludedCapabilityId, SetupFact, SetupFacts, SetupStepId, SetupStepStatus, evaluate_setup_flow,
};

fn all_ready() -> SetupFacts {
    SetupFacts {
        storage: SetupFact::ready("storage ready"),
        local_steam_identity: SetupFact::ready("identity ready"),
        gsi_config: SetupFact::ready("GSI ready"),
        capture_recorder: SetupFact::ready("recorder ready"),
        ffprobe: SetupFact::ready("ffprobe ready"),
        test_capture: SetupFact::ready("test capture passed"),
        local_demo_validation: SetupFact::ready("Demo validated"),
        manual_flag: SetupFact::ready("Manual Flag ready"),
    }
}

#[test]
fn emits_the_complete_ordered_v1_flow_and_explicit_exclusions() {
    let flow = evaluate_setup_flow(&all_ready());
    assert!(flow.ready());
    assert_eq!(
        flow.steps.iter().map(|step| step.id).collect::<Vec<_>>(),
        vec![
            SetupStepId::Storage,
            SetupStepId::LocalSteamIdentity,
            SetupStepId::GsiConfig,
            SetupStepId::CaptureRecorder,
            SetupStepId::Ffprobe,
            SetupStepId::TestCapture,
            SetupStepId::LocalDemoValidation,
            SetupStepId::ManualFlag,
        ]
    );
    assert_eq!(
        flow.excluded_capabilities
            .iter()
            .map(|excluded| excluded.id)
            .collect::<Vec<_>>(),
        vec![
            ExcludedCapabilityId::SteamSignIn,
            ExcludedCapabilityId::ShareCodes,
            ExcludedCapabilityId::GameCoordinator,
            ExcludedCapabilityId::AutomaticDemoDiscovery,
            ExcludedCapabilityId::PublicWebsite,
            ExcludedCapabilityId::UiOrProcessSideEffects,
        ]
    );
    let statements = flow
        .excluded_capabilities
        .iter()
        .map(|excluded| excluded.statement.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    for term in [
        "Steam sign-in",
        "Share Codes",
        "Game Coordinator",
        "automatically",
        "public website",
        "launch processes",
    ] {
        assert!(
            statements.contains(term),
            "missing explicit exclusion: {term}"
        );
    }
}

#[test]
fn dependency_failures_skip_downstream_steps_with_exact_injected_remediation() {
    let mut facts = all_ready();
    let exact = "Choose /mnt/private/openfrag; preserve punctuation: []{}!?";
    facts.storage = SetupFact::blocked("storage blocked", exact);
    let flow = evaluate_setup_flow(&facts);

    let storage = flow.step(SetupStepId::Storage).expect("storage step");
    assert_eq!(storage.status, SetupStepStatus::Blocked);
    assert_eq!(storage.remediation.as_deref(), Some(exact));
    for id in [
        SetupStepId::LocalSteamIdentity,
        SetupStepId::GsiConfig,
        SetupStepId::CaptureRecorder,
        SetupStepId::TestCapture,
        SetupStepId::LocalDemoValidation,
        SetupStepId::ManualFlag,
    ] {
        let step = flow.step(id).expect("dependent step");
        assert_eq!(step.status, SetupStepStatus::Skipped, "{id:?}");
        assert_eq!(step.remediation.as_deref(), Some(exact), "{id:?}");
    }
    assert_eq!(
        flow.step(SetupStepId::Ffprobe).expect("ffprobe").status,
        SetupStepStatus::Ready
    );
}

#[test]
fn independent_gsi_demo_and_capture_failures_keep_their_own_exact_actions() {
    let mut facts = all_ready();
    facts.gsi_config = SetupFact::blocked("GSI missing", "Install exactly this local cfg.");
    facts.test_capture = SetupFact::blocked(
        "capture test failed",
        "Select monitor DP-2, then rerun Test Capture.",
    );
    facts.local_demo_validation = SetupFact::blocked(
        "Demo invalid",
        "Choose a local .dem file with a valid HL2DEMO header.",
    );
    let flow = evaluate_setup_flow(&facts);
    assert_eq!(
        flow.step(SetupStepId::GsiConfig)
            .expect("GSI")
            .remediation
            .as_deref(),
        Some("Install exactly this local cfg.")
    );
    assert_eq!(
        flow.step(SetupStepId::LocalDemoValidation)
            .expect("Demo")
            .remediation
            .as_deref(),
        Some("Choose a local .dem file with a valid HL2DEMO header.")
    );
    let manual = flow.step(SetupStepId::ManualFlag).expect("Manual Flag");
    assert_eq!(manual.status, SetupStepStatus::Skipped);
    assert_eq!(
        manual.remediation.as_deref(),
        Some("Select monitor DP-2, then rerun Test Capture.")
    );
}

#[test]
fn reevaluation_is_deterministic_and_has_no_external_actions() {
    let facts = all_ready();
    assert_eq!(evaluate_setup_flow(&facts), evaluate_setup_flow(&facts));
}

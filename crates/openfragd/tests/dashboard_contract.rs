#[test]
fn dashboard_wires_local_contracts_accessibly_without_remote_state() {
    let page = include_str!("../src/dashboard.html");
    for endpoint in [
        "/api/health",
        "/api/setup",
        "/api/imports",
        "/api/matches",
        "/api/receipts",
        "/api/clips",
        "/api/manual-flag",
        "/api/diagnostics",
    ] {
        assert!(page.contains(endpoint), "missing {endpoint}");
    }
    for attribute in [
        "role=\"status\"",
        "aria-live=\"polite\"",
        "aria-label=\"Dashboard sections\"",
        "for=\"dem-file\"",
    ] {
        assert!(page.contains(attribute), "missing {attribute}");
    }
    for forbidden in [
        "http://",
        "https://",
        "localStorage",
        "sessionStorage",
        "telemetry",
        "analytics",
        "cloud",
        "account",
        "upload",
        "fetch('http",
        "<script src=",
        "<link rel=",
        "window.open",
        "child_process",
        "\u{2014}",
    ] {
        assert!(
            !page
                .to_ascii_lowercase()
                .contains(&forbidden.to_ascii_lowercase()),
            "forbidden {forbidden}"
        );
    }
    assert!(!page.contains("No local Demo has been imported"));
}

#[test]
fn dashboard_exposes_complete_local_flows_and_honest_failure_states() {
    let page = include_str!("../src/dashboard.html");
    for required in [
        "loadRecentMatches()",
        "loadMatch(match.id)",
        "loadClips()",
        "loadDiagnostics()",
        "loadSetup()",
        "pollImport(job.id)",
        "['completed', 'failed', 'cancelled']",
        "method: 'PATCH'",
        "void trimClip()",
        "id=\"clip-trim-start\"",
        "id=\"clip-trim-end\"",
        "JSON.stringify({ start_ms, end_ms })",
        "Trim end must be greater than trim start",
        "clipAction('export', 'Export')",
        "method: 'POST'",
        "Local API unavailable",
    ] {
        assert!(page.contains(required), "missing flow marker {required}");
    }
    assert!(!page.contains("EvidenceUnavailable"));
    assert!(page.matches("catch (error)").count() >= 10);
    assert!(page.contains("aria-busy"));
    assert!(page.contains("Setup status unavailable"));
    assert!(page.contains("unavailable(output, 'Import status'"));
    assert!(page.contains("Select a clip first."));
}

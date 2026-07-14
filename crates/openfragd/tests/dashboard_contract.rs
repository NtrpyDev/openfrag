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

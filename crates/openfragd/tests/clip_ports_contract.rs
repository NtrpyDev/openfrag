#[path = "../src/api.rs"] mod api;
#[path = "../src/service.rs"] mod service;
#[path = "../src/clip_ports.rs"] mod clip_ports;
#[test]
fn clip_port_source_mentions_only_local_direct_workflows() {
    let source=include_str!("../src/clip_ports.rs");
    for required in ["review_clip", "reject_clip", "export_clip", "DirectCommand", "-i"] { assert!(source.contains(required)); }
    for forbidden in ["http://", "https://", "curl", "wget"] { assert!(!source.contains(forbidden)); }
}

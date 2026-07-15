use openfragd::api::{ApiError, ImportedFile, LocalApi, SetupResponse};
use openfragd::service::{MutationPorts, PipelinePorts, StorageApi, UnavailablePorts};
use std::sync::{Arc, Mutex};

#[test]
fn unavailable_mutations_are_explicit_without_a_pipeline() {
    let directory = tempfile::tempdir().unwrap();
    let storage =
        openfrag_storage::Storage::open(openfrag_storage::Layout::at(directory.path())).unwrap();
    let api = StorageApi::new(
        Arc::new(Mutex::new(storage)),
        UnavailablePorts,
        SetupResponse {
            fingerprint: "test".into(),
            complete: false,
            checks: vec![],
            cs2_cfg_candidates: vec![],
        },
    );
    assert!(matches!(api.manual_flag(), Err(ApiError::Unavailable(_))));
    assert!(api.matches().unwrap().is_empty());
}

#[test]
fn pipeline_port_requires_identity_and_rejects_non_demo_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(Mutex::new(
        openfrag_storage::Storage::open(openfrag_storage::Layout::at(directory.path())).unwrap(),
    ));
    let file = ImportedFile {
        filename: "bad.dem".into(),
        bytes: b"not a Source 2 Demo".to_vec(),
    };
    let unconfigured = PipelinePorts::new(storage.clone(), directory.path().into(), None);
    assert!(matches!(
        unconfigured.import(file.clone()),
        Err(ApiError::Unavailable(_))
    ));
    let configured = PipelinePorts::new(storage, directory.path().into(), Some(765));
    assert!(matches!(configured.import(file), Err(ApiError::Invalid(_))));
}

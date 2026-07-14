#[path = "../src/api.rs"] mod api;
#[path = "../src/service.rs"] mod service;
use api::{ApiError, LocalApi, SetupResponse};
use service::{StorageApi, UnavailablePorts};
use std::sync::{Arc, Mutex};

#[test]
fn unavailable_mutations_are_explicit_without_a_pipeline() {
    let directory=tempfile::tempdir().unwrap(); let storage=openfrag_storage::Storage::open(openfrag_storage::Layout::at(directory.path())).unwrap();
    let api=StorageApi::new(Arc::new(Mutex::new(storage)), UnavailablePorts, SetupResponse { checks: vec![] });
    assert!(matches!(api.manual_flag(), Err(ApiError::Unavailable(_))));
    assert!(api.matches().unwrap().is_empty());
}

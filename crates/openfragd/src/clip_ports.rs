//! Local-only clip mutation ports that the daemon can wire into `MutationPorts`.
#![allow(clippy::missing_errors_doc)]

use crate::{api::{ApiError, Clip as ApiClip, ClipDecision, ClipUpdate, ImportedFile, ImportJob}, service::MutationPorts};
use openfrag_clips::{Clip, ClipRepository, LocalFileSystem, ReviewEdit, export_clip, reject_clip, review_clip};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::{Path, PathBuf}, sync::Mutex};

pub trait ClipMetadata: ClipRepository { fn load(&mut self, id: &str) -> Result<Clip, ApiError>; fn now_ms(&self) -> u64; }
pub trait DirectCommand { fn run(&mut self, program: &Path, args: &[String]) -> Result<(), ApiError>; }
pub struct ClipPorts<M, F, C> { metadata: Mutex<M>, files: Mutex<F>, command: Mutex<C>, ffmpeg: PathBuf, export_directory: PathBuf }
impl<M, F, C> ClipPorts<M, F, C> { #[must_use] pub fn new(metadata: M, files: F, command: C, ffmpeg: PathBuf, export_directory: PathBuf) -> Self { Self { metadata:Mutex::new(metadata), files:Mutex::new(files), command:Mutex::new(command), ffmpeg, export_directory } } }
impl<M: ClipMetadata + Send + Sync + 'static, F: LocalFileSystem + Send + Sync + 'static, C: DirectCommand + Send + Sync + 'static> MutationPorts for ClipPorts<M, F, C> {
    fn import(&self, _: ImportedFile) -> Result<ImportJob, ApiError> { Err(ApiError::Unavailable("local import pipeline is not connected".into())) }
    fn update_clip(&self, id: &str, update: ClipUpdate) -> Result<ApiClip, ApiError> { let mut metadata=self.metadata.lock().map_err(|_| ApiError::Unavailable("clip metadata lock".into()))?; let clip=metadata.load(id)?; let now=metadata.now_ms(); let revised=match update.decision { ClipDecision::Keep => review_clip(&mut *metadata, &clip, ReviewEdit { title:update.title, note:update.note, tags:update.tags.into_iter().collect::<BTreeSet<_>>(), favorite:false, derivative:None }, now), ClipDecision::Reject => reject_clip(&mut *metadata, &clip, now) }.map_err(|error| ApiError::Invalid(format!("clip review rejected: {error:?}")))?; Ok(ApiClip { id:revised.id().as_str().into(), title:Some(revised.title().into()), note:Some(revised.note().into()), tags:revised.tags().iter().cloned().collect() }) }
    fn trim(&self, id: &str) -> Result<Value, ApiError> { let clip=self.metadata.lock().map_err(|_| ApiError::Unavailable("clip metadata lock".into()))?.load(id)?; let source=clip.source_capture(); let args=vec!["-y".into(), "-i".into(), source.path().display().to_string(), "-c".into(), "copy".into(), self.export_directory.join(format!("{}.trim.mp4", clip.id().as_str())).display().to_string()]; self.command.lock().map_err(|_| ApiError::Unavailable("trim command lock".into()))?.run(&self.ffmpeg, &args)?; Ok(json!({"clip":id,"status":"trim_requested"})) }
    fn export(&self, id: &str) -> Result<Value, ApiError> { let clip=self.metadata.lock().map_err(|_| ApiError::Unavailable("clip metadata lock".into()))?.load(id)?; let receipt=export_clip(&mut *self.files.lock().map_err(|_| ApiError::Unavailable("export filesystem lock".into()))?, &clip, &self.export_directory).map_err(|error| ApiError::Invalid(format!("local export failed: {error:?}")))?; Ok(json!({"clip":id,"path":receipt.destination_path()})) }
    fn manual_flag(&self) -> Result<Value, ApiError> { Err(ApiError::Unavailable("capture pipeline is not connected".into())) }
}

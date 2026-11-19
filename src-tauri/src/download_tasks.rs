use crate::{
    download::{DownloadArtifact, DownloadManager, ProgressCallback},
    error::{AppError, AppResult},
    sources::resolutions::{ResolutionScope, Resolutions},
    transaction::FileEngine,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct DownloadTasks(Arc<Mutex<BTreeMap<String, CancellationToken>>>);
struct Guard {
    tasks: DownloadTasks,
    id: String,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.tasks
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
impl DownloadTasks {
    pub fn cancel(&self, id: &str) -> AppResult<()> {
        let tasks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        tasks
            .get(id)
            .ok_or_else(|| AppError::NotFound("下载任务".into()))?
            .cancel();
        Ok(())
    }
    pub(crate) async fn run(
        &self,
        engine: &FileEngine,
        resolutions: &Resolutions,
        id: String,
        resolution_id: uuid::Uuid,
        progress: ProgressCallback,
    ) -> AppResult<DownloadArtifact> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(AppError::Invalid("下载任务ID无效".into()));
        }
        let cancel = CancellationToken::new();
        {
            let mut tasks = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if tasks.contains_key(&id) {
                return Err(AppError::Conflict("下载任务已运行".into()));
            }
            tasks.insert(id.clone(), cancel.clone());
        }
        let _guard = Guard {
            tasks: self.clone(),
            id,
        };
        let resolved = resolutions.take(&ResolutionScope::Desktop, resolution_id)?;
        let artifact = DownloadManager::new(&engine.paths)?
            .download_mod(&resolved, cancel, progress)
            .await?;
        crate::artifacts::record(engine, &artifact, &resolved)?;
        Ok(artifact)
    }
}

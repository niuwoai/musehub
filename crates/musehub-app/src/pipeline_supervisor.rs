//! Track pipeline adapter PIDs for cancel / cleanup.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agent_supervisor::kill_process_group;

#[derive(Default)]
pub struct PipelineSupervisor {
    inner: Mutex<HashMap<Uuid, u32>>,
}

impl PipelineSupervisor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn register(&self, run_id: Uuid, pid: u32) {
        self.inner.lock().await.insert(run_id, pid);
    }

    pub async fn cancel(&self, run_id: Uuid) -> bool {
        let pid = self.inner.lock().await.remove(&run_id);
        if let Some(pid) = pid {
            kill_process_group(pid);
            return true;
        }
        false
    }

    pub async fn finish(&self, run_id: Uuid) {
        self.inner.lock().await.remove(&run_id);
    }
}

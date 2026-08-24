use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(not(feature = "qa-mode"))]
use crate::profile::ExecutorConfigs;
use crate::{
    actions::Executable,
    approvals::ExecutorApprovalService,
    env::ExecutionEnv,
    executors::{BaseCodingAgent, ExecutorError, SpawnedChild, StandardCodingAgentExecutor},
    profile::ExecutorConfig,
    role::Role,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct CodingAgentInitialRequest {
    pub prompt: String,
    /// Unified executor identity + overrides
    #[serde(alias = "executor_profile_id", alias = "profile_variant_label")]
    pub executor_config: ExecutorConfig,
    /// Optional relative path to execute the agent in (relative to container_ref).
    /// If None, uses the container_ref directory directly.
    #[serde(default)]
    pub working_dir: Option<String>,
}

impl CodingAgentInitialRequest {
    pub fn base_executor(&self) -> BaseCodingAgent {
        self.executor_config.executor
    }

    pub fn effective_dir(&self, current_dir: &Path) -> std::path::PathBuf {
        match &self.working_dir {
            Some(rel_path) => current_dir.join(rel_path),
            None => current_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl Executable for CodingAgentInitialRequest {
    #[cfg_attr(feature = "qa-mode", allow(unused_variables))]
    async fn spawn(
        &self,
        current_dir: &Path,
        approvals: Arc<dyn ExecutorApprovalService>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let effective_dir = self.effective_dir(current_dir);

        #[cfg(feature = "qa-mode")]
        {
            tracing::info!("QA mode: using mock executor instead of real agent");
            let executor = crate::executors::qa_mock::QaMockExecutor;
            return executor.spawn(&effective_dir, &self.prompt, env).await;
        }

        #[cfg(not(feature = "qa-mode"))]
        {
            // A role is resolved against the worktree, so it is versioned with the
            // code it governs. It fills in unset overrides and leads the prompt;
            // the prompt half is what makes it work on every executor.
            let mut executor_config = self.executor_config.clone();
            let role = Role::resolve(&effective_dir, &mut executor_config)?;
            let prompt = match &role {
                Some(role) => role.apply_to_prompt(&self.prompt),
                None => self.prompt.clone(),
            };

            let profile_id = executor_config.profile_id();
            let mut agent = ExecutorConfigs::get_cached()
                .get_coding_agent(&profile_id)
                .ok_or(ExecutorError::UnknownExecutorType(profile_id.to_string()))?;

            if executor_config.has_overrides() {
                agent.apply_overrides(&executor_config);
            }
            agent.use_approvals(approvals.clone());

            agent.spawn(&effective_dir, &prompt, env).await
        }
    }
}

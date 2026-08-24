//! Roles: a named bundle of instructions and executor defaults, committed to the
//! repo it applies to.
//!
//! A Role is deliberately *not* an executor preset. `agent_id` is Claude-specific,
//! so a Role that only mapped onto it would be a Claude Code preset with extra
//! steps. The portable mechanism is the instruction block, which every executor
//! honours because it is just part of the prompt. Executor-specific fields are a
//! bonus applied where they exist.
//!
//! Roles live at `.agent-deck/roles/<name>.md` in the worktree, so they are
//! reviewed and versioned with the code they govern.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::{
    executors::BaseCodingAgent, model_selector::PermissionPolicy, profile::ExecutorConfig,
};

/// Directory, relative to the worktree root, that holds role definitions.
pub const ROLES_DIR: &str = ".agent-deck/roles";

/// Frontmatter delimiter. TOML rather than YAML because `toml` is already a
/// dependency and parses lists and enums correctly, where a hand-rolled YAML
/// subset would not.
const FRONTMATTER_FENCE: &str = "+++";

#[derive(Error, Debug)]
pub enum RoleError {
    #[error("Role '{0}' not found in {ROLES_DIR}")]
    NotFound(String),

    #[error("Role '{name}' is malformed: {reason}")]
    Malformed { name: String, reason: String },

    #[error(
        "Role '{role}' does not permit executor {executor}. Permitted: {permitted}. \
         Either run this role on a permitted executor, or add {executor} to its `executors` list."
    )]
    ExecutorNotPermitted {
        role: String,
        executor: BaseCodingAgent,
        permitted: String,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The frontmatter half of a role file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
pub struct RoleMeta {
    /// Human-readable summary, shown when picking a role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Executors this role may run on. Empty means every executor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executors: Vec<BaseCodingAgent>,
    /// Default model, used only when the caller did not pick one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Claude Code subagent to run as. Ignored on other executors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_agent: Option<String>,
    /// Default reasoning effort, used only when the caller did not pick one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Default permission policy, used only when the caller did not pick one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<PermissionPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct Role {
    /// Filename stem. This is what `ExecutorConfig.role_id` refers to.
    pub name: String,
    #[serde(flatten)]
    pub meta: RoleMeta,
    /// The markdown body, prepended to the opening prompt.
    pub instructions: String,
}

impl Role {
    pub fn dir(worktree: &Path) -> PathBuf {
        worktree.join(ROLES_DIR)
    }

    pub fn path(worktree: &Path, name: &str) -> PathBuf {
        Self::dir(worktree).join(format!("{name}.md"))
    }

    /// Split a role file into its frontmatter and body.
    ///
    /// Frontmatter is optional: a role that is only instructions is valid, and is
    /// the shape most people will write first.
    pub fn parse(name: &str, content: &str) -> Result<Self, RoleError> {
        let content = content.strip_prefix('\u{feff}').unwrap_or(content);
        let trimmed = content.trim_start();

        let (meta, body) = match trimmed.strip_prefix(FRONTMATTER_FENCE) {
            None => (RoleMeta::default(), content),
            Some(rest) => {
                // The opening fence runs to end of line; the body starts after the
                // closing fence's line.
                let rest = rest.strip_prefix('\n').unwrap_or(rest);
                let end = rest
                    .find(FRONTMATTER_FENCE)
                    .ok_or_else(|| RoleError::Malformed {
                        name: name.to_string(),
                        reason: format!(
                            "frontmatter opens with `{FRONTMATTER_FENCE}` but never closes"
                        ),
                    })?;
                let frontmatter = &rest[..end];
                let body = &rest[end + FRONTMATTER_FENCE.len()..];
                let meta: RoleMeta =
                    toml::from_str(frontmatter).map_err(|e| RoleError::Malformed {
                        name: name.to_string(),
                        reason: e.to_string(),
                    })?;
                (meta, body)
            }
        };

        Ok(Self {
            name: name.to_string(),
            meta,
            instructions: body.trim().to_string(),
        })
    }

    pub fn load(worktree: &Path, name: &str) -> Result<Self, RoleError> {
        let path = Self::path(worktree, name);
        let content =
            std::fs::read_to_string(&path).map_err(|_| RoleError::NotFound(name.to_string()))?;
        Self::parse(name, &content)
    }

    /// Every role defined in this worktree, sorted by name.
    ///
    /// A malformed role is logged and skipped rather than failing the whole list:
    /// one bad file should not make the others unselectable.
    pub fn list(worktree: &Path) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(Self::dir(worktree)) else {
            return Vec::new();
        };

        let mut roles: Vec<Self> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension()? != "md" {
                    return None;
                }
                let name = path.file_stem()?.to_str()?.to_string();
                match std::fs::read_to_string(&path)
                    .ok()
                    .map(|c| Self::parse(&name, &c))
                {
                    Some(Ok(role)) => Some(role),
                    Some(Err(e)) => {
                        tracing::warn!("Skipping role {:?}: {}", path, e);
                        None
                    }
                    None => None,
                }
            })
            .collect();

        roles.sort_by(|a, b| a.name.cmp(&b.name));
        roles
    }

    pub fn permits(&self, executor: BaseCodingAgent) -> bool {
        self.meta.executors.is_empty() || self.meta.executors.contains(&executor)
    }

    /// Fold this role's defaults into `config`.
    ///
    /// An explicit choice always beats the role's default: the role fills blanks,
    /// it does not overwrite. The one thing it does enforce is the executor
    /// allowlist, which is the point of having a role at all.
    pub fn apply(&self, config: &mut ExecutorConfig) -> Result<(), RoleError> {
        if !self.permits(config.executor) {
            return Err(RoleError::ExecutorNotPermitted {
                role: self.name.clone(),
                executor: config.executor,
                permitted: self
                    .meta
                    .executors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }

        if config.model_id.is_none() {
            config.model_id = self.meta.model.clone();
        }
        if config.reasoning_id.is_none() {
            config.reasoning_id = self.meta.reasoning.clone();
        }
        if config.permission_policy.is_none() {
            config.permission_policy = self.meta.permission_policy.clone();
        }
        // agent_id is Claude-only. Setting it elsewhere would be silently ignored
        // at best, so keep the executor-specific field on the executor it belongs to.
        if config.agent_id.is_none() && config.executor == BaseCodingAgent::ClaudeCode {
            config.agent_id = self.meta.claude_agent.clone();
        }

        Ok(())
    }

    /// Prepend the role's instructions to the opening prompt.
    ///
    /// This is the part that works on every executor, and the only part that does.
    pub fn apply_to_prompt(&self, prompt: &str) -> String {
        if self.instructions.is_empty() {
            return prompt.to_string();
        }
        format!(
            "You are acting in the role of `{}`.\n\n{}\n\n---\n\n{}",
            self.name, self.instructions, prompt
        )
    }

    /// Load and apply the role named by `config.role_id`, if there is one.
    ///
    /// Returns the instructions to prepend, or None when no role is selected.
    /// A named role that cannot be loaded is an error, not a silent no-op: a
    /// product whose thesis is enforcement must not quietly drop the constraint.
    pub fn resolve(
        worktree: &Path,
        config: &mut ExecutorConfig,
    ) -> Result<Option<Self>, RoleError> {
        let Some(role_id) = config.role_id.clone() else {
            return Ok(None);
        };
        let role = Self::load(worktree, &role_id)?;
        role.apply(config)?;
        Ok(Some(role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"+++
description = "Owns deploys, infra and CI"
executors = ["CLAUDE_CODE", "CODEX"]
model = "opus"
claude_agent = "cloud-engineer"
permission_policy = "SUPERVISED"
+++

Prefer boring infrastructure.
"#;

    fn config(executor: BaseCodingAgent) -> ExecutorConfig {
        ExecutorConfig::new(executor)
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let role = Role::parse("cloud-engineer", FULL).unwrap();
        assert_eq!(
            role.meta.description.as_deref(),
            Some("Owns deploys, infra and CI")
        );
        assert_eq!(
            role.meta.executors,
            vec![BaseCodingAgent::ClaudeCode, BaseCodingAgent::Codex]
        );
        assert_eq!(
            role.meta.permission_policy,
            Some(PermissionPolicy::Supervised)
        );
        assert_eq!(role.instructions, "Prefer boring infrastructure.");
    }

    #[test]
    fn frontmatter_is_optional() {
        let role = Role::parse("plain", "Just do the work.").unwrap();
        assert_eq!(role.meta, RoleMeta::default());
        assert_eq!(role.instructions, "Just do the work.");
        // No executor list means every executor is permitted.
        assert!(role.permits(BaseCodingAgent::Gemini));
    }

    #[test]
    fn unclosed_frontmatter_is_an_error_not_a_body() {
        let err = Role::parse("bad", "+++\nmodel = \"opus\"\n").unwrap_err();
        assert!(matches!(err, RoleError::Malformed { .. }));
    }

    #[test]
    fn fills_blanks_but_never_overwrites_an_explicit_choice() {
        let role = Role::parse("cloud-engineer", FULL).unwrap();

        let mut blank = config(BaseCodingAgent::ClaudeCode);
        role.apply(&mut blank).unwrap();
        assert_eq!(blank.model_id.as_deref(), Some("opus"));
        assert_eq!(blank.agent_id.as_deref(), Some("cloud-engineer"));

        let mut explicit = config(BaseCodingAgent::ClaudeCode);
        explicit.model_id = Some("haiku".to_string());
        role.apply(&mut explicit).unwrap();
        assert_eq!(explicit.model_id.as_deref(), Some("haiku"));
    }

    #[test]
    fn claude_agent_does_not_leak_onto_other_executors() {
        let role = Role::parse("cloud-engineer", FULL).unwrap();
        let mut cfg = config(BaseCodingAgent::Codex);
        role.apply(&mut cfg).unwrap();
        // Codex has no notion of a subagent id; only the portable fields carry over.
        assert_eq!(cfg.agent_id, None);
        assert_eq!(cfg.model_id.as_deref(), Some("opus"));
    }

    #[test]
    fn a_disallowed_executor_is_refused() {
        let role = Role::parse("cloud-engineer", FULL).unwrap();
        let err = role
            .apply(&mut config(BaseCodingAgent::Gemini))
            .unwrap_err();
        assert!(matches!(err, RoleError::ExecutorNotPermitted { .. }));
    }

    #[test]
    fn instructions_lead_the_prompt() {
        let role = Role::parse("cloud-engineer", FULL).unwrap();
        let prompt = role.apply_to_prompt("Ship the deploy.");
        assert!(prompt.starts_with("You are acting in the role of `cloud-engineer`."));
        assert!(prompt.contains("Prefer boring infrastructure."));
        assert!(prompt.ends_with("Ship the deploy."));
    }

    #[test]
    fn loads_lists_and_resolves_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let roles = Role::dir(dir.path());
        std::fs::create_dir_all(&roles).unwrap();
        std::fs::write(roles.join("cloud-engineer.md"), FULL).unwrap();
        std::fs::write(roles.join("reviewer.md"), "Review, do not write.").unwrap();
        std::fs::write(roles.join("notes.txt"), "ignored").unwrap();

        let listed = Role::list(dir.path());
        assert_eq!(
            listed.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["cloud-engineer", "reviewer"]
        );

        let mut cfg = config(BaseCodingAgent::ClaudeCode);
        cfg.role_id = Some("cloud-engineer".to_string());
        let resolved = Role::resolve(dir.path(), &mut cfg).unwrap().unwrap();
        assert_eq!(resolved.name, "cloud-engineer");
        assert_eq!(cfg.agent_id.as_deref(), Some("cloud-engineer"));
    }

    #[test]
    fn a_named_role_that_is_missing_fails_loudly() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(BaseCodingAgent::ClaudeCode);
        cfg.role_id = Some("nope".to_string());
        let err = Role::resolve(dir.path(), &mut cfg).unwrap_err();
        assert!(matches!(err, RoleError::NotFound(_)));
    }

    #[test]
    fn no_role_selected_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(BaseCodingAgent::ClaudeCode);
        assert!(Role::resolve(dir.path(), &mut cfg).unwrap().is_none());
    }
}

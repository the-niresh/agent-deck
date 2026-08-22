use std::{
    borrow::Cow,
    collections::BTreeSet,
    sync::{Arc, OnceLock},
};

use directories::ProjectDirs;
use regex::Regex;
use sentry::protocol::Event;
use sentry_tracing::{EventFilter, SentryLayer};
use serde_json::Value;
use tracing::Level;

static INIT_GUARD: OnceLock<sentry::ClientInitGuard> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub enum SentrySource {
    Backend,
    Desktop,
    Mcp,
    Remote,
}

impl SentrySource {
    fn tag(self) -> &'static str {
        match self {
            SentrySource::Backend => "backend",
            SentrySource::Desktop => "desktop",
            SentrySource::Mcp => "mcp",
            SentrySource::Remote => "remote",
        }
    }

    fn dsn(self) -> Option<String> {
        let value = match self {
            SentrySource::Remote => option_env!("SENTRY_DSN_REMOTE")
                .map(|s| s.to_string())
                .or_else(|| std::env::var("SENTRY_DSN_REMOTE").ok()),
            _ => option_env!("SENTRY_DSN")
                .map(|s| s.to_string())
                .or_else(|| std::env::var("SENTRY_DSN").ok()),
        };
        value.filter(|s| !s.is_empty())
    }
}

fn environment() -> Cow<'static, str> {
    option_env!("SENTRY_ENVIRONMENT")
        .map(Cow::Borrowed)
        .or_else(|| std::env::var("SENTRY_ENVIRONMENT").ok().map(Cow::Owned))
        .filter(|value| !value.is_empty())
        .unwrap_or(if cfg!(debug_assertions) {
            Cow::Borrowed("dev")
        } else {
            Cow::Borrowed("production")
        })
}

pub fn init_once(source: SentrySource) {
    let Some(dsn) = source.dsn() else {
        return;
    };

    INIT_GUARD.get_or_init(|| {
        sentry::init((
            dsn,
            sentry::ClientOptions {
                release: sentry::release_name!(),
                environment: Some(environment()),
                before_send: Some(Arc::new(|event| Some(scrub_event(event)))),
                ..Default::default()
            },
        ))
    });

    sentry::configure_scope(|scope| {
        scope.set_tag("source", source.tag());
    });
}

pub fn configure_user_scope(user_id: &str, username: Option<&str>, email: Option<&str>) {
    let mut sentry_user = sentry::User {
        id: Some(user_id.to_string()),
        ..Default::default()
    };

    if let Some(username) = username {
        sentry_user.username = Some(username.to_string());
    }

    if let Some(email) = email {
        sentry_user.email = Some(email.to_string());
    }

    sentry::configure_scope(|scope| {
        scope.set_user(Some(sentry_user));
    });
}

pub fn sentry_layer<S>() -> SentryLayer<S>
where
    S: tracing::Subscriber,
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    SentryLayer::default()
        .span_filter(|meta| {
            matches!(
                *meta.level(),
                Level::DEBUG | Level::INFO | Level::WARN | Level::ERROR
            )
        })
        .event_filter(|meta| match *meta.level() {
            Level::ERROR => EventFilter::Event,
            Level::DEBUG | Level::INFO | Level::WARN => EventFilter::Breadcrumb,
            Level::TRACE => EventFilter::Ignore,
        })
}

fn scrub_event(event: Event<'static>) -> Event<'static> {
    let context = ScrubContext::from_runtime();
    scrub_event_with_context(event, &context)
}

fn scrub_event_with_context(event: Event<'static>, context: &ScrubContext) -> Event<'static> {
    let Ok(mut value) = serde_json::to_value(&event) else {
        return event;
    };

    scrub_json_value(&mut value, context);
    serde_json::from_value(value).unwrap_or(event)
}

#[derive(Debug, Default)]
struct ScrubContext {
    sensitive_values: Vec<String>,
    repository_paths: Vec<String>,
    branch_names: Vec<String>,
}

impl ScrubContext {
    fn from_runtime() -> Self {
        let mut context = Self::default();

        for (key, value) in std::env::vars() {
            let key = key.to_ascii_uppercase();
            if is_sensitive_env_key(&key) {
                context.add_sensitive_value(value.clone());
            } else if key.contains("BRANCH") {
                context.add_branch_name(value.clone());
            }

            if is_repository_path_key(&key) {
                context.add_repository_path(value);
            }
        }

        if let Ok(current_dir) = std::env::current_dir() {
            context.add_repository_path(current_dir.display().to_string());
        }

        if let Some(project_dirs) = ProjectDirs::from("ai", "bloop", "agent-deck") {
            context.add_sensitive_values_from_json_file(
                project_dirs.data_dir().join("credentials.json"),
            );
        }

        if let Some(project_dirs) = ProjectDirs::from("ai", "bloop", "vibe-kanban") {
            context.add_sensitive_values_from_json_file(
                project_dirs.data_dir().join("credentials.json"),
            );
        }

        context.deduplicate();
        context
    }

    #[cfg(test)]
    fn with_sensitive_values(values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut context = Self::default();
        for value in values {
            context.add_sensitive_value(value.into());
        }
        context.deduplicate();
        context
    }

    fn add_sensitive_values_from_json_file(&mut self, path: impl AsRef<std::path::Path>) {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return;
        };

        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return;
        };

        self.add_sensitive_values_from_json(&value);
    }

    fn add_sensitive_values_from_json(&mut self, value: &Value) {
        match value {
            Value::String(value) => self.add_sensitive_value(value.clone()),
            Value::Array(values) => {
                for value in values {
                    self.add_sensitive_values_from_json(value);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    self.add_sensitive_values_from_json(value);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    fn add_sensitive_value(&mut self, value: String) {
        if should_redact_literal(&value) {
            self.sensitive_values.push(value);
        }
    }

    fn add_repository_path(&mut self, value: String) {
        if value.starts_with('/') && value.len() >= 8 {
            self.repository_paths.push(value);
        }
    }

    fn add_branch_name(&mut self, value: String) {
        if should_redact_literal(&value) {
            self.branch_names.push(value);
        }
    }

    fn deduplicate(&mut self) {
        self.sensitive_values = deduplicated(self.sensitive_values.drain(..));
        self.repository_paths = deduplicated(self.repository_paths.drain(..));
        self.branch_names = deduplicated(self.branch_names.drain(..));
    }
}

fn scrub_json_value(value: &mut Value, context: &ScrubContext) {
    match value {
        Value::String(raw) => {
            *raw = scrub_string(raw, context);
        }
        Value::Array(values) => {
            for value in values {
                scrub_json_value(value, context);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                scrub_json_value(value, context);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn scrub_string(value: &str, context: &ScrubContext) -> String {
    let mut scrubbed = redact_home_paths(value);

    for path in &context.repository_paths {
        scrubbed = scrubbed.replace(path, "[REDACTED_REPOSITORY_PATH]");
    }

    for branch_name in &context.branch_names {
        scrubbed = scrubbed.replace(branch_name, "[REDACTED_BRANCH]");
    }

    for sensitive_value in &context.sensitive_values {
        scrubbed = scrubbed.replace(sensitive_value, "[REDACTED]");
    }

    scrubbed
}

fn redact_home_paths(value: &str) -> String {
    static HOME_PATH_RE: OnceLock<Regex> = OnceLock::new();
    let regex = HOME_PATH_RE.get_or_init(|| {
        Regex::new(r#"(?:/home/[^/\s"'<>]+|/Users/[^/\s"'<>]+|/root)(?:/[^\s"'<>]*)?"#)
            .expect("home path regex must compile")
    });

    regex
        .replace_all(value, "[REDACTED_HOME_PATH]")
        .into_owned()
}

fn is_sensitive_env_key(key: &str) -> bool {
    static SENSITIVE_ENV_KEY_RE: OnceLock<Regex> = OnceLock::new();
    let regex = SENSITIVE_ENV_KEY_RE.get_or_init(|| {
        Regex::new(r"(?i)(?:TOKEN|SECRET|PASSWORD|PASS|KEY|DSN|CREDENTIAL|AUTH|COOKIE)")
            .expect("sensitive env key regex must compile")
    });

    regex.is_match(key)
}

fn is_repository_path_key(key: &str) -> bool {
    matches!(
        key,
        "PWD" | "OLDPWD" | "CARGO_MANIFEST_DIR" | "GIT_DIR" | "GIT_WORK_TREE"
    ) || key.ends_with("_REPO")
        || key.ends_with("_REPO_PATH")
        || key.ends_with("_REPOSITORY")
        || key.ends_with("_REPOSITORY_PATH")
}

fn should_redact_literal(value: &str) -> bool {
    let value = value.trim();
    value.len() >= 8
}

fn deduplicated(values: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut deduplicated = Vec::new();

    for value in values {
        if seen.insert(value.clone()) {
            deduplicated.push(value);
        }
    }

    deduplicated.sort_by_key(|value| std::cmp::Reverse(value.len()));
    deduplicated
}

#[cfg(test)]
mod tests {
    use sentry::protocol::Event;
    use serde_json::{Value, json};

    use super::{ScrubContext, scrub_event_with_context};

    #[test]
    fn scrub_event_removes_home_paths_and_tokens() {
        let mut context =
            ScrubContext::with_sensitive_values(["secret-token-value", "customer/main"]);
        context
            .repository_paths
            .push("/srv/customer/repo".to_string());
        context.branch_names.push("customer/main".to_string());
        context.deduplicate();

        let mut event = Event {
            message: Some("failed at /home/niresh/work/agent-deck on /srv/customer/repo".into()),
            ..Default::default()
        };
        event.extra.insert(
            "detail".into(),
            json!({
                "token": "secret-token-value",
                "branch": "customer/main",
                "path": "/Users/niresh/src/agent-deck/file.rs",
            }),
        );

        let scrubbed = scrub_event_with_context(event, &context);
        let scrubbed_json = serde_json::to_value(&scrubbed).expect("event must serialize");

        assert!(
            !json_contains(&scrubbed_json, "secret-token-value"),
            "token should be redacted: {scrubbed_json}"
        );
        assert!(
            !json_contains(&scrubbed_json, "/home/niresh"),
            "home path should be redacted: {scrubbed_json}"
        );
        assert!(
            !json_contains(&scrubbed_json, "/Users/niresh"),
            "macOS home path should be redacted: {scrubbed_json}"
        );
        assert!(
            !json_contains(&scrubbed_json, "/srv/customer/repo"),
            "repository path should be redacted: {scrubbed_json}"
        );
        assert!(
            !json_contains(&scrubbed_json, "customer/main"),
            "branch name should be redacted: {scrubbed_json}"
        );
    }

    fn json_contains(value: &Value, needle: &str) -> bool {
        serde_json::to_string(value)
            .expect("value must serialize")
            .contains(needle)
    }
}

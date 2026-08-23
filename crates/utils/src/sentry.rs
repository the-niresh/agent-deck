use std::{
    borrow::Cow,
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
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
    let context = cached_scrub_context();
    scrub_event_with_context(event, &context)
}

fn scrub_event_with_context(event: Event<'static>, context: &ScrubContext) -> Event<'static> {
    let Ok(mut value) = serde_json::to_value(&event) else {
        return event;
    };

    scrub_json_value(&mut value, context);
    serde_json::from_value(value).unwrap_or(event)
}

// The scrub context is invoked from Sentry's `before_send`, which fires on
// every captured event - including during error storms when the process is
// already unhealthy. The env-var and cwd portion of the context is fixed for
// the lifetime of the process (the only `set_var` calls we make happen at
// startup), so we build it once. The `credentials.json` files DO change at
// runtime (they are written the first time a user authenticates), so we
// re-read them - but only when the file's mtime or length has actually
// changed, which we detect with a cheap `metadata` syscall instead of a
// full `read_to_string`.
//
// We deliberately avoid a TTL cache here: any window between a token being
// written to disk and the cache expiring would let that token be sent to
// Sentry in plaintext. Comparing fs metadata closes that window entirely
// AND avoids the disk read on the common (unchanged) path. Do not
// "simplify" this into a time-based or permanently-cached scheme.
static CONTEXT_CACHE: OnceLock<Mutex<ContextCache>> = OnceLock::new();

fn cached_scrub_context() -> Arc<ScrubContext> {
    let cache = CONTEXT_CACHE.get_or_init(|| Mutex::new(ContextCache::from_runtime()));
    let mut guard = cache.lock().expect("scrub context cache mutex poisoned");
    guard.refresh();
    Arc::clone(&guard.full_context)
}

#[derive(Debug)]
struct ContextCache {
    static_context: ScrubContext,
    credentials_entries: Vec<CredentialsEntry>,
    full_context: Arc<ScrubContext>,
}

#[derive(Debug)]
struct CredentialsEntry {
    path: PathBuf,
    fingerprint: Option<FileFingerprint>,
    sensitive_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    modified: Option<SystemTime>,
    len: u64,
}

impl ContextCache {
    fn from_runtime() -> Self {
        Self::new(
            ScrubContext::from_static_environment(),
            default_credentials_paths(),
        )
    }

    fn new(static_context: ScrubContext, credentials_paths: Vec<PathBuf>) -> Self {
        let credentials_entries = credentials_paths
            .into_iter()
            .map(|path| CredentialsEntry {
                path,
                fingerprint: None,
                sensitive_values: Vec::new(),
            })
            .collect();

        let mut cache = Self {
            static_context,
            credentials_entries,
            full_context: Arc::new(ScrubContext::default()),
        };
        for entry in &mut cache.credentials_entries {
            entry.fingerprint = read_file_fingerprint(&entry.path);
            entry.sensitive_values = load_credentials_sensitive_values(&entry.path);
        }
        cache.rebuild_full_context();
        cache
    }

    fn refresh(&mut self) {
        let mut changed = false;
        for entry in &mut self.credentials_entries {
            let current = read_file_fingerprint(&entry.path);
            if current != entry.fingerprint {
                entry.fingerprint = current;
                entry.sensitive_values = load_credentials_sensitive_values(&entry.path);
                changed = true;
            }
        }
        if changed {
            self.rebuild_full_context();
        }
    }

    fn rebuild_full_context(&mut self) {
        let mut context = self.static_context.clone();
        for entry in &self.credentials_entries {
            for value in &entry.sensitive_values {
                context.sensitive_values.push(value.clone());
            }
        }
        context.deduplicate();
        self.full_context = Arc::new(context);
    }
}

fn default_credentials_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(project_dirs) = ProjectDirs::from("ai", "bloop", "agent-deck") {
        paths.push(project_dirs.data_dir().join("credentials.json"));
    }
    if let Some(project_dirs) = ProjectDirs::from("ai", "bloop", "vibe-kanban") {
        paths.push(project_dirs.data_dir().join("credentials.json"));
    }
    paths
}

fn read_file_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileFingerprint {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

fn load_credentials_sensitive_values(path: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let mut helper = ScrubContext::default();
    helper.add_sensitive_values_from_json(&value);
    helper.sensitive_values
}

#[derive(Debug, Default, Clone)]
struct ScrubContext {
    sensitive_values: Vec<String>,
    repository_paths: Vec<String>,
    branch_names: Vec<String>,
}

impl ScrubContext {
    fn from_static_environment() -> Self {
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

    use super::{ContextCache, ScrubContext, scrub_event_with_context};

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

    #[test]
    fn scrub_picks_up_credentials_written_after_first_scrub() {
        let dir = tempfile::tempdir().expect("tempdir must be created");
        let path = dir.path().join("credentials.json");

        // First scrub: credentials file does not exist yet. The token in
        // the event stays in plaintext because we don't know about it.
        let mut cache = ContextCache::new(ScrubContext::default(), vec![path.clone()]);
        let event = Event {
            message: Some("token: later-written-secret-token-42".into()),
            ..Default::default()
        };
        let scrubbed_before = scrub_event_with_context(event.clone(), &cache.full_context);
        let json_before = serde_json::to_value(&scrubbed_before).expect("event must serialize");
        assert!(
            json_contains(&json_before, "later-written-secret-token-42"),
            "sanity check: token is present before credentials file is written"
        );

        // User "authenticates" - credentials file appears with the secret.
        std::fs::write(&path, r#"{"token": "later-written-secret-token-42"}"#)
            .expect("credentials file must be written");

        // Next event: the scrubber must notice the file change and redact
        // the secret. This is the whole point of using an mtime/length
        // fingerprint rather than a TTL - the redaction window closes the
        // instant the file is written, not after some expiry.
        cache.refresh();
        let scrubbed_after = scrub_event_with_context(event, &cache.full_context);
        let json_after = serde_json::to_value(&scrubbed_after).expect("event must serialize");
        assert!(
            !json_contains(&json_after, "later-written-secret-token-42"),
            "credential written after first scrub must be redacted next time: {json_after}"
        );
    }

    fn json_contains(value: &Value, needle: &str) -> bool {
        serde_json::to_string(value)
            .expect("value must serialize")
            .contains(needle)
    }
}

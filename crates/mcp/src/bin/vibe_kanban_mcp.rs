use mcp::task_server::McpServer;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, prelude::*};
use utils::{
    port_file::{PortInfo, read_port_info},
    sentry::{self as sentry_utils, SentrySource, sentry_layer},
};

const HOST_ENV: &str = "MCP_HOST";
const PORT_ENV: &str = "MCP_PORT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpLaunchMode {
    Global,
    Orchestrator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchConfig {
    mode: McpLaunchMode,
}

fn main() -> anyhow::Result<()> {
    let launch_config = resolve_launch_config()?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let version = env!("CARGO_PKG_VERSION");
            init_process_logging("vibe-kanban-mcp", version);

            let base_url = resolve_base_url("vibe-kanban-mcp").await?;
            let LaunchConfig { mode } = launch_config;

            let server = match mode {
                McpLaunchMode::Global => McpServer::new_global(&base_url),
                McpLaunchMode::Orchestrator => McpServer::new_orchestrator(&base_url),
            };

            let service = server.init().await?.serve(stdio()).await.map_err(|error| {
                tracing::error!("serving error: {:?}", error);
                error
            })?;

            service.waiting().await?;
            Ok(())
        })
}

fn resolve_launch_config() -> anyhow::Result<LaunchConfig> {
    resolve_launch_config_from_iter(std::env::args().skip(1))
}

fn resolve_launch_config_from_iter<I>(mut args: I) -> anyhow::Result<LaunchConfig>
where
    I: Iterator<Item = String>,
{
    let mut mode = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => {
                mode = Some(args.next().ok_or_else(|| {
                    anyhow::anyhow!("Missing value for --mode. Expected 'global' or 'orchestrator'")
                })?);
            }
            "-h" | "--help" => {
                println!("Usage: vibe-kanban-mcp --mode <global|orchestrator>");
                std::process::exit(0);
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown argument '{arg}'. Usage: vibe-kanban-mcp --mode <global|orchestrator>"
                ));
            }
        }
    }

    let mode = match mode
        .as_deref()
        .unwrap_or("global")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "global" => McpLaunchMode::Global,
        "orchestrator" => McpLaunchMode::Orchestrator,
        value => {
            return Err(anyhow::anyhow!(
                "Invalid MCP mode '{value}'. Expected 'global' or 'orchestrator'"
            ));
        }
    };

    Ok(LaunchConfig { mode })
}

/// Probe a candidate backend URL by sending a lightweight GET request.
/// Returns `true` if the backend responds (any HTTP status), `false` on
/// connection failure or timeout.
async fn probe_backend(url: &str, log_prefix: &str) -> bool {
    let probe_url = format!("{}/api/health", url.trim_end_matches('/'));
    tracing::debug!("[{}] Probing backend at {}", log_prefix, probe_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap_or_default();
    match client.get(&probe_url).send().await {
        Ok(_) => {
            tracing::debug!("[{}] Backend alive at {}", log_prefix, url);
            true
        }
        Err(e) => {
            tracing::debug!("[{}] Backend probe failed at {}: {}", log_prefix, url, e);
            false
        }
    }
}

async fn resolve_base_url(log_prefix: &str) -> anyhow::Result<String> {
    let backend_url = std::env::var("VIBE_BACKEND_URL").ok();
    let host = std::env::var(HOST_ENV)
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "127.0.0.1".to_string());

    let explicit_port =
        match std::env::var(PORT_ENV)
            .or_else(|_| std::env::var("BACKEND_PORT"))
            .or_else(|_| std::env::var("PORT"))
        {
            Ok(port_str) => Some(port_str.parse::<u16>().map_err(|error| {
                anyhow::anyhow!("Invalid port value '{}': {}", port_str, error)
            })?),
            Err(_) => None,
        };

    let port_info = match explicit_port {
        Some(_) => None,
        None => Some(read_port_info("vibe-kanban").await?),
    };

    // An authoritative URL is one the operator or the server itself stated
    // verbatim (VIBE_BACKEND_URL, or the canonical URL the server persisted).
    // Those are taken at face value. Only a URL we *reconstructed* from a
    // host + port guess is worth probing, because that is the case where the
    // loopback spelling can disagree with what the server actually bound to.
    let is_authoritative = backend_url.is_some()
        || port_info
            .as_ref()
            .is_some_and(|info| info.backend_url.is_some());
    let reconstructed_port = explicit_port.or_else(|| port_info.as_ref().map(|i| i.main_port));

    let resolved = resolve_base_url_from_sources(
        log_prefix,
        backend_url,
        host.clone(),
        explicit_port,
        port_info,
    )?;

    match (is_authoritative, reconstructed_port) {
        (false, Some(port)) => {
            Ok(probe_with_loopback_fallback(resolved, &host, port, log_prefix).await)
        }
        _ => Ok(resolved),
    }
}

/// Verify a reconstructed backend URL responds, retrying alternate loopback
/// spellings. Handles the backend binding `localhost` (which may resolve to
/// `::1`) while we defaulted to `127.0.0.1`, or vice versa.
async fn probe_with_loopback_fallback(
    primary_url: String,
    host: &str,
    port: u16,
    log_prefix: &str,
) -> String {
    if probe_backend(&primary_url, log_prefix).await {
        tracing::info!("[{}] Using backend URL: {}", log_prefix, primary_url);
        return primary_url;
    }

    let alternates: &[&str] = match host {
        "127.0.0.1" => &["localhost", "[::1]"],
        "localhost" => &["127.0.0.1", "[::1]"],
        _ => &["localhost", "127.0.0.1"],
    };

    for alt_host in alternates {
        let alt_url = format!("http://{}:{}", alt_host, port);
        if probe_backend(&alt_url, log_prefix).await {
            tracing::info!(
                "[{}] Primary host {} unreachable, using alternate: {}",
                log_prefix,
                host,
                alt_url
            );
            return alt_url;
        }
    }

    // Fall through to the original URL — downstream reports the real error.
    tracing::warn!(
        "[{}] No backend responded on port {}. Using {} (may fail).",
        log_prefix,
        port,
        primary_url
    );
    primary_url
}

fn resolve_base_url_from_sources(
    log_prefix: &str,
    backend_url: Option<String>,
    host: String,
    explicit_port: Option<u16>,
    port_info: Option<PortInfo>,
) -> anyhow::Result<String> {
    if let Some(url) = backend_url {
        tracing::info!(
            "[{}] Using backend URL from VIBE_BACKEND_URL: {}",
            log_prefix,
            url
        );
        return Ok(url);
    }

    if let Some(port) = explicit_port {
        tracing::info!("[{}] Using port from environment: {}", log_prefix, port);
        let url = format!("http://{}:{}", host, port);
        tracing::info!("[{}] Using backend URL: {}", log_prefix, url);
        return Ok(url);
    }

    let port_info = port_info.ok_or_else(|| anyhow::anyhow!("Missing port file information"))?;
    if let Some(url) = port_info.backend_url {
        tracing::info!(
            "[{}] Using canonical backend URL from port file: {}",
            log_prefix,
            url
        );
        return Ok(url);
    }

    tracing::info!(
        "[{}] Using port from port file: {}",
        log_prefix,
        port_info.main_port
    );
    let url = format!("http://{}:{}", host, port_info.main_port);
    tracing::info!("[{}] Using backend URL: {}", log_prefix, url);
    Ok(url)
}

fn init_process_logging(log_prefix: &str, version: &str) {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    sentry_utils::init_once(SentrySource::Mcp);

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(EnvFilter::new("debug")),
        )
        .with(sentry_layer())
        .init();

    tracing::debug!(
        "[{}] Starting Vibe Kanban MCP server version {}...",
        log_prefix,
        version
    );
}

#[cfg(test)]
mod tests {
    use utils::port_file::PortInfo;

    use super::{
        LaunchConfig, McpLaunchMode, resolve_base_url_from_sources, resolve_launch_config_from_iter,
    };

    #[test]
    fn orchestrator_mode_does_not_require_session_id() {
        let config = resolve_launch_config_from_iter(
            ["--mode".to_string(), "orchestrator".to_string()].into_iter(),
        )
        .expect("config should parse");

        assert_eq!(
            config,
            LaunchConfig {
                mode: McpLaunchMode::Orchestrator
            }
        );
    }

    #[test]
    fn session_id_flag_is_rejected() {
        let error = resolve_launch_config_from_iter(
            [
                "--mode".to_string(),
                "orchestrator".to_string(),
                "--session-id".to_string(),
                "x".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("session id flag should be rejected");

        assert!(
            error
                .to_string()
                .contains("Unknown argument '--session-id'")
        );
    }

    #[test]
    fn vibe_backend_url_has_highest_precedence() {
        let url = resolve_base_url_from_sources(
            "test",
            Some("http://override:9999".to_string()),
            "legacy-host".to_string(),
            Some(7777),
            None,
        )
        .expect("base url should resolve");

        assert_eq!(url, "http://override:9999");
    }

    #[test]
    fn explicit_env_host_and_port_beat_port_file_url() {
        let url =
            resolve_base_url_from_sources("test", None, "env-host".to_string(), Some(7777), None)
                .expect("base url should resolve");

        assert_eq!(url, "http://env-host:7777");
    }

    #[test]
    fn canonical_backend_url_from_port_file_beats_legacy_reconstruction() {
        let url = resolve_base_url_from_sources(
            "test",
            None,
            "legacy-host".to_string(),
            None,
            Some(PortInfo {
                main_port: 4567,
                preview_proxy_port: Some(8901),
                backend_url: Some("http://localhost:4567".to_string()),
            }),
        )
        .expect("base url should resolve");

        assert_eq!(url, "http://localhost:4567");
    }

    #[test]
    fn legacy_port_file_still_reconstructs_from_host_and_port() {
        let url = resolve_base_url_from_sources(
            "test",
            None,
            "legacy-host".to_string(),
            None,
            Some(PortInfo {
                main_port: 4567,
                preview_proxy_port: Some(8901),
                backend_url: None,
            }),
        )
        .expect("base url should resolve");

        assert_eq!(url, "http://legacy-host:4567");
    }
}

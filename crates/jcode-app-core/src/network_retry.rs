use std::time::Duration;
use tokio::process::Command;
use tokio::time::{sleep, timeout};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkWaitPlan {
    pub reason: String,
    pub listener_summary: String,
}

pub fn classify_network_interruption(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = Some(error);
    while let Some(err) = current {
        let text = err.to_string().to_ascii_lowercase();
        parts.push(text);
        current = err.source();
    }
    classify_text(&parts.join(" | "))
}

pub fn classify_message(message: &str) -> Option<String> {
    classify_text(&message.to_ascii_lowercase())
}

fn classify_text(text: &str) -> Option<String> {
    let network_markers = [
        "connection reset",
        "connection aborted",
        "connection refused",
        "broken pipe",
        "network is unreachable",
        "network unreachable",
        "host is down",
        "no route to host",
        "not connected",
        "dns error",
        "failed to lookup address",
        "temporary failure in name resolution",
        "name or service not known",
        "could not resolve host",
        "couldn't resolve host",
        "host is unreachable",
        "operation timed out",
        "timed out",
        "timeout",
        "error trying to connect",
        "connection closed before message completed",
        "unexpected eof",
        "end of file before message completed",
    ];
    if network_markers.iter().any(|marker| text.contains(marker)) {
        return Some("the network connection appears to have dropped".to_string());
    }
    None
}

/// Default Ollama API socket. A `connection refused` against this on loopback is
/// not a real network outage — the local model server simply isn't running — so
/// it can be revived in place (see [`try_revive_local_ollama`]) instead of
/// waiting on internet connectivity, which is already up and would just spin.
const OLLAMA_LOCAL_MARKERS: [&str; 2] = ["127.0.0.1:11434", "localhost:11434"];

/// True when an error chain points at a downed local Ollama server (the request
/// targeted the loopback Ollama port and was refused). Model-agnostic: it keys on
/// the port, not on any model name, so it covers whatever local model is loaded.
pub fn is_local_ollama_outage_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut parts = Vec::new();
    let mut current = Some(error);
    while let Some(err) = current {
        parts.push(err.to_string().to_ascii_lowercase());
        current = err.source();
    }
    is_local_ollama_outage_text(&parts.join(" | "))
}

/// Message-string variant of [`is_local_ollama_outage_error`] for stream-error
/// events that surface a formatted string rather than a typed error.
pub fn is_local_ollama_outage_message(message: &str) -> bool {
    is_local_ollama_outage_text(&message.to_ascii_lowercase())
}

fn is_local_ollama_outage_text(lower: &str) -> bool {
    lower.contains("connection refused")
        && OLLAMA_LOCAL_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Revive a downed local Ollama server in place. Returns immediately if the API
/// port is already accepting connections; otherwise spawns `ollama serve` and
/// polls the port for up to ~20s. The spawn inherits this process's environment,
/// so the launcher's `OLLAMA_*` tuning (context length, KV-cache type, keep-alive)
/// carries through without being hardcoded here. No-op-safe when `ollama` is not
/// installed (returns false → caller falls back to the normal network wait).
pub async fn try_revive_local_ollama() -> bool {
    if ollama_api_port_open().await {
        return true;
    }
    if !command_exists("ollama").await {
        return false;
    }
    let spawned = Command::new("ollama")
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false) // outlive this Child handle; session_end hook reaps it
        .spawn();
    if spawned.is_err() {
        return false;
    }
    for _ in 0..20 {
        sleep(Duration::from_secs(1)).await;
        if ollama_api_port_open().await {
            return true;
        }
    }
    false
}

async fn ollama_api_port_open() -> bool {
    matches!(
        timeout(
            Duration::from_secs(1),
            tokio::net::TcpStream::connect("127.0.0.1:11434"),
        )
        .await,
        Ok(Ok(_))
    )
}

pub fn wait_plan() -> NetworkWaitPlan {
    #[cfg(target_os = "linux")]
    {
        NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary:
                "listening for Linux netlink changes via `ip monitor`; also verifying with reconnect probes"
                    .to_string(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        return NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary:
                "listening for macOS route/interface changes via `route -n monitor`; also verifying with reconnect probes"
                    .to_string(),
        };
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary: "waiting with lightweight reconnect probes".to_string(),
        }
    }
}

pub async fn wait_until_probably_online() {
    let mut delay = Duration::from_secs(1);
    loop {
        if probe_connectivity().await {
            return;
        }
        wait_for_platform_change_or_delay(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

pub async fn is_probably_online() -> bool {
    probe_connectivity().await
}

async fn probe_connectivity() -> bool {
    let client = jcode_provider_core::shared_http_client();
    let request = client
        .head("https://www.gstatic.com/generate_204")
        .timeout(Duration::from_secs(5));
    matches!(request.send().await, Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 204)
}

async fn wait_for_platform_change_or_delay(delay: Duration) {
    #[cfg(target_os = "linux")]
    {
        if command_exists("ip").await {
            let fut = wait_for_command_output("ip", &["monitor", "link", "address", "route"]);
            let _ = timeout(delay, fut).await;
            return;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if command_exists("route").await {
            let fut = wait_for_command_output("route", &["-n", "monitor"]);
            let _ = timeout(delay, fut).await;
            return;
        }
    }
    sleep(delay).await;
}

async fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!(
            "command -v {} >/dev/null 2>&1",
            shell_escape(command)
        ))
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

fn shell_escape(value: &str) -> String {
    value.replace('\'', "'\\''")
}

async fn wait_for_command_output(command: &str, args: &[&str]) {
    let mut command_builder = Command::new(command);
    command_builder
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = match command_builder.spawn() {
        Ok(child) => child,
        Err(_) => return,
    };
    if let Some(mut stdout) = child.stdout.take() {
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 1];
        let _ = stdout.read(&mut buf).await;
    }
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_network_errors() {
        assert!(classify_message("connection reset by peer").is_some());
        assert!(classify_message("temporary failure in name resolution").is_some());
        assert!(classify_message("network is unreachable").is_some());
        assert!(classify_message("401 unauthorized").is_none());
    }

    #[test]
    fn detects_local_ollama_outage_from_message() {
        // The real reqwest/Ollama failure string the TUI surfaces.
        let msg = "error sending request for url (http://localhost:11434/v1/chat/completions): \
                   client error (Connect): tcp connect error: Connection refused (os error 61)";
        assert!(is_local_ollama_outage_message(msg));
        assert!(is_local_ollama_outage_message(
            "endpoint http://127.0.0.1:11434/v1/chat/completions Connection refused"
        ));
    }

    #[test]
    fn ignores_non_local_or_non_refused_failures() {
        // Remote endpoint refused → not a local-Ollama revive case.
        assert!(!is_local_ollama_outage_message(
            "https://openrouter.ai/api/v1/chat/completions: Connection refused"
        ));
        // Local endpoint but a different failure (timeout, not refused).
        assert!(!is_local_ollama_outage_message(
            "http://127.0.0.1:11434/v1/chat/completions: operation timed out"
        ));
    }
}

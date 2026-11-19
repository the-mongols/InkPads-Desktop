use crate::watcher::WatcherStatus;
use crate::wows::{save_config, AppConfig};
use keyring::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;

const KEYRING_SERVICE: &str = "InkPadsDesktop";
const KEYRING_USER: &str = "session_token";
const INKPADS_LOGIN_BASE: &str = "https://inkpads.pages.dev/api/auth/wg-login";

pub fn load_session_token() -> Option<String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()?;
    match entry.get_password() {
        Ok(tok) if !tok.trim().is_empty() => Some(tok),
        _ => None,
    }
}

pub fn save_session_token(token: &str) -> Result<(), String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("Keyring init error: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("Keyring save error: {e}"))
}

pub fn clear_session_token() {
    if let Ok(entry) = Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        let _ = entry.delete_credential();
    }
}

pub fn format_account_label(nickname: Option<&str>, clan: Option<&str>) -> Option<String> {
    let nick = nickname?.trim();
    if nick.is_empty() {
        return None;
    }
    match clan.map(str::trim).filter(|c| !c.is_empty()) {
        Some(tag) => Some(format!("[{tag}] {nick}")),
        None => Some(nick.to_string()),
    }
}

pub async fn start_loopback_login(
    app: AppHandle,
    region: &str,
    token_slot: Arc<RwLock<Option<String>>>,
    config: Arc<RwLock<AppConfig>>,
    status: Arc<RwLock<WatcherStatus>>,
) -> Result<(), String> {
    let safe_region = match region {
        "eu" | "asia" => region,
        _ => "com",
    };

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind loopback port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to read loopback port: {e}"))?
        .port();

    let nonce = Uuid::new_v4().simple().to_string();
    let login_url = format!(
        "{INKPADS_LOGIN_BASE}?region={safe_region}&desktop_port={port}&state={nonce}"
    );

    info!(port, region = safe_region, "Opening WG OpenID login in browser");
    app.opener()
        .open_url(&login_url, None::<&str>)
        .map_err(|e| format!("Failed to open browser: {e}"))?;

    let expected_nonce = nonce.clone();
    let accept_res = tokio::time::timeout(Duration::from_secs(180), listener.accept()).await;
    let Ok(Ok((mut socket, _))) = accept_res else {
        return Err("WG login timed out (3 minutes)".to_string());
    };

    let mut buf = [0u8; 4096];
    let n = socket
        .read(&mut buf)
        .await
        .map_err(|e| format!("Failed to read callback request: {e}"))?;
    let req_str = String::from_utf8_lossy(&buf[..n]);
    let request_line = req_str.lines().next().unwrap_or("");
    let path_and_query = request_line.split_whitespace().nth(1).unwrap_or("/");

    let parsed = Url::parse(&format!("http://127.0.0.1:{port}{path_and_query}"))
        .map_err(|e| format!("Invalid callback URL: {e}"))?;
    let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();

    let returned_state = params.get("state").map(String::as_str).unwrap_or("");
    let token = params.get("token").map(String::as_str).unwrap_or("");

    let (http_status, body_html, result) = if returned_state == expected_nonce && !token.is_empty()
    {
        let nickname = params.get("nickname").cloned().unwrap_or_default();
        let clan = params.get("clan").cloned().filter(|c| !c.is_empty());

        if let Err(e) = save_session_token(token) {
            warn!(error = %e, "Failed to store token in Windows Credential Manager");
        }
        *token_slot.write().await = Some(token.to_string());
        {
            let mut cfg = config.write().await;
            cfg.linked_nickname = if nickname.is_empty() {
                None
            } else {
                Some(nickname.clone())
            };
            cfg.linked_clan = clan.clone();
            let _ = save_config(&cfg);
        }
        let label = format_account_label(Some(&nickname), clan.as_deref());
        {
            let mut st = status.write().await;
            st.linked_account = label.clone();
            st.last_event = Some(match label {
                Some(ref l) => format!("Linked WG account: {l}"),
                None => "Linked WG account".to_string(),
            });
        }

        (
            "200 OK",
            "<html><body style='font-family:system-ui;background:#0f172a;color:#e2e8f0;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'><div style='text-align:center'><h2>InkPads Desktop Linked!</h2><p>You can close this browser tab and return to InkPads.</p></div></body></html>",
            Ok(()),
        )
    } else {
        (
            "400 Bad Request",
            "<html><body style='font-family:system-ui;background:#0f172a;color:#f87171;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'><div style='text-align:center'><h2>Link Failed</h2><p>Invalid or expired login state. Please try again from the InkPads tray.</p></div></body></html>",
            Err("Invalid state or missing token in callback".to_string()),
        )
    };

    let response = format!(
        "HTTP/1.1 {http_status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_html}",
        body_html.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    result
}

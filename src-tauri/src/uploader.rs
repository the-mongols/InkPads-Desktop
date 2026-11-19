use crate::replay::ReplayProbe;
use reqwest::multipart;
use serde::Deserialize;
use std::path::Path;
use tokio::fs;
use tracing::{info, warn};

pub const INKPADS_BASE_URL: &str = "https://inkpads.pages.dev";

#[derive(Debug, Deserialize)]
struct PreflightResponse {
    results: Vec<PreflightResult>,
}

#[derive(Debug, Deserialize)]
struct PreflightResult {
    client_id: Option<String>,
    action: String,
    reason: Option<String>,
    matched_arena_id: Option<String>,
    arena_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum UploadOutcome {
    SkippedNonCb(String),
    SkippedDuplicate(String),
    WinStatsRecorded,
    UploadedForRender { action: String },
}

#[derive(Debug)]
pub enum UploadError {
    Unauthorized,
    Message(String),
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadError::Unauthorized => write!(f, "Session expired (401 Unauthorized)"),
            UploadError::Message(m) => write!(f, "{m}"),
        }
    }
}

pub struct UploaderClient {
    http: reqwest::Client,
    base_url: String,
}

impl UploaderClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent("InkPads-Desktop/0.1.0")
            .build()
            .expect("failed to build reqwest client");
        Self {
            http,
            base_url: INKPADS_BASE_URL.to_string(),
        }
    }

    /// Executes the two-phase preflight handshake and only uploads `.wowsreplay` binaries
    /// when required (`NEW_LOSS_UPLOAD`, `DUAL_UPGRADE_READY`, `WIN_TEMP_BUFFER`).
    pub async fn process_replay(
        &self,
        path: &Path,
        probe: &ReplayProbe,
        bearer_token: Option<&str>,
    ) -> Result<UploadOutcome, UploadError> {
        // Strict local Clan Battle scope gate before any network traffic.
        if !probe.is_clan_battle {
            return Ok(UploadOutcome::SkippedNonCb(format!(
                "Ignored local {} match ({})",
                probe.match_type, probe.filename
            )));
        }

        let preflight_url = format!("{}/api/worker/batch-preflight", self.base_url);
        let payload = serde_json::json!({ "batch": [probe] });

        let mut req = self.http.post(&preflight_url).json(&payload);
        if let Some(tok) = bearer_token {
            req = req.bearer_auth(tok);
        }

        let res = req
            .send()
            .await
            .map_err(|e| UploadError::Message(format!("Preflight request failed: {e}")))?;

        if res.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UploadError::Unauthorized);
        }

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(UploadError::Message(format!("Preflight HTTP {status}: {text}")));
        }

        let parsed: PreflightResponse = res
            .json()
            .await
            .map_err(|e| UploadError::Message(format!("Invalid preflight JSON: {e}")))?;

        let Some(item) = parsed
            .results
            .into_iter()
            .find(|r| r.client_id.as_deref() == Some(probe.client_id.as_str()))
        else {
            return Err(UploadError::Message(
                "Preflight response did not include matching client_id".to_string(),
            ));
        };

        match item.action.as_str() {
            "NON_CB_SKIP" => Ok(UploadOutcome::SkippedNonCb(
                item.reason.unwrap_or_else(|| "Non-Clan Battle".to_string()),
            )),
            "DUPLICATE_SKIP" => Ok(UploadOutcome::SkippedDuplicate(
                item.reason.unwrap_or_else(|| "Duplicate perspective".to_string()),
            )),
            "WIN_STATS_ONLY" => {
                // Golden Invariant 1: 0 bytes uploaded for Clan Battle victories.
                info!(file = %probe.filename, "Clan Battle win stats recorded (0 bytes uploaded)");
                Ok(UploadOutcome::WinStatsRecorded)
            }
            "NEW_LOSS_UPLOAD" | "DUAL_UPGRADE_READY" | "WIN_TEMP_BUFFER" => {
                let arena_id = item.matched_arena_id.or(item.arena_id).or_else(|| probe.arena_id.clone());
                self.upload_binary(path, &probe.filename, arena_id.as_deref(), bearer_token)
                    .await?;
                Ok(UploadOutcome::UploadedForRender {
                    action: item.action,
                })
            }
            other => {
                warn!(action = %other, "Unknown preflight action; skipping upload");
                Ok(UploadOutcome::SkippedNonCb(format!("Unknown action: {other}")))
            }
        }
    }

    async fn upload_binary(
        &self,
        path: &Path,
        filename: &str,
        arena_id: Option<&str>,
        bearer_token: Option<&str>,
    ) -> Result<(), UploadError> {
        let bytes = fs::read(path)
            .await
            .map_err(|e| UploadError::Message(format!("Failed to read replay bytes: {e}")))?;

        let part = multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")
            .map_err(|e| UploadError::Message(format!("Invalid MIME type: {e}")))?;

        let mut form = multipart::Form::new().part("file", part);
        if let Some(id) = arena_id {
            form = form.text("arena_id", id.to_string());
        }

        let ingest_url = format!("{}/api/worker/batch-ingest", self.base_url);
        let mut req = self.http.post(&ingest_url).multipart(form);
        if let Some(tok) = bearer_token {
            req = req.bearer_auth(tok);
        }

        let res = req
            .send()
            .await
            .map_err(|e| UploadError::Message(format!("Binary upload failed: {e}")))?;

        if res.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UploadError::Unauthorized);
        }

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(UploadError::Message(format!("Ingest HTTP {status}: {text}")));
        }

        info!(file = %filename, "Uploaded replay binary to batch-ingest");
        Ok(())
    }
}

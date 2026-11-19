use crate::auth::clear_session_token;
use crate::replay::probe_replay_header;
use crate::uploader::{UploadError, UploadOutcome, UploaderClient};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize)]
pub struct WatcherStatus {
    pub replays_dir: Option<String>,
    pub paused: bool,
    pub state: String,
    pub linked_account: Option<String>,
    pub last_event: Option<String>,
}

impl Default for WatcherStatus {
    fn default() -> Self {
        Self {
            replays_dir: None,
            paused: false,
            state: "Idle".to_string(),
            linked_account: None,
            last_event: None,
        }
    }
}

fn is_eligible_replay(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".wowsreplay") && lower != "temp.wowsreplay"
}

/// Waits until file size and mtime stabilize (and the game releases its write lock).
async fn wait_for_stabilization(path: &Path) -> Option<u64> {
    let mut prev_len: Option<u64> = None;
    let mut prev_mtime: Option<SystemTime> = None;

    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let Ok(meta) = tokio::fs::metadata(path).await else {
            return None;
        };
        if !meta.is_file() {
            return None;
        }
        let len = meta.len();
        let mtime = meta.modified().ok();

        if len > 12 && prev_len == Some(len) && prev_mtime == mtime {
            // Confirm file is readable (not exclusively locked by WoWS).
            if std::fs::File::open(path).is_ok() {
                return Some(len);
            }
        }

        prev_len = Some(len);
        prev_mtime = mtime;
    }

    None
}

pub fn spawn_watcher(
    status: Arc<RwLock<WatcherStatus>>,
    token_slot: Arc<RwLock<Option<String>>>,
    mut dir_rx: mpsc::Receiver<Option<PathBuf>>,
    initial_dir: Option<PathBuf>,
) {
    tauri::async_runtime::spawn(async move {
        let uploader = Arc::new(UploaderClient::new());
        let processed = Arc::new(RwLock::new(HashSet::<(PathBuf, u64)>::new()));
        let in_flight = Arc::new(RwLock::new(HashSet::<PathBuf>::new()));
        let (fs_tx, mut fs_rx) = mpsc::channel::<PathBuf>(128);

        let mut watcher: Option<RecommendedWatcher> = None;
        let current_dir = initial_dir;

        let configure_watcher = |dir: Option<PathBuf>,
                                 watcher_slot: &mut Option<RecommendedWatcher>,
                                 fs_tx: &mpsc::Sender<PathBuf>| {
            *watcher_slot = None;
            let Some(ref target) = dir else {
                return;
            };
            if !target.exists() {
                return;
            }
            let tx = fs_tx.clone();
            match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_)
                    ) {
                        for p in event.paths {
                            if is_eligible_replay(&p) {
                                let _ = tx.try_send(p);
                            }
                        }
                    }
                }
            }) {
                Ok(mut w) => {
                    if let Err(e) = w.watch(target, RecursiveMode::Recursive) {
                        error!(error = %e, path = %target.display(), "Failed to watch replays directory");
                    } else {
                        info!(path = %target.display(), "Watching WoWS replays directory");
                        *watcher_slot = Some(w);
                    }
                }
                Err(e) => {
                    error!(error = %e, "Failed to initialize filesystem watcher");
                }
            }
        };

        configure_watcher(current_dir.clone(), &mut watcher, &fs_tx);
        {
            let mut st = status.write().await;
            st.replays_dir = current_dir.as_ref().map(|p| p.display().to_string());
            st.state = if watcher.is_some() {
                if st.paused {
                    "Paused".to_string()
                } else {
                    "Watching".to_string()
                }
            } else {
                "WoWS Folder Not Found".to_string()
            };
        }

        loop {
            tokio::select! {
                Some(new_dir) = dir_rx.recv() => {
                    configure_watcher(new_dir.clone(), &mut watcher, &fs_tx);
                    let mut st = status.write().await;
                    st.replays_dir = new_dir.as_ref().map(|p| p.display().to_string());
                    st.state = if watcher.is_some() {
                        if st.paused { "Paused".to_string() } else { "Watching".to_string() }
                    } else {
                        "WoWS Folder Not Found".to_string()
                    };
                }
                Some(path) = fs_rx.recv() => {
                    if status.read().await.paused {
                        continue;
                    }
                    {
                        let mut active = in_flight.write().await;
                        if !active.insert(path.clone()) {
                            continue;
                        }
                    }
                    let status_clone = Arc::clone(&status);
                    let uploader_clone = Arc::clone(&uploader);
                    let processed_clone = Arc::clone(&processed);
                    let in_flight_clone = Arc::clone(&in_flight);
                    let token_clone = Arc::clone(&token_slot);

                    tauri::async_runtime::spawn(async move {
                        let size = wait_for_stabilization(&path).await;
                        in_flight_clone.write().await.remove(&path);
                        let Some(size) = size else {
                            return;
                        };

                        {
                            let seen = processed_clone.read().await;
                            if seen.contains(&(path.clone(), size)) {
                                return;
                            }
                        }

                        let probe = match tokio::task::spawn_blocking({
                            let p = path.clone();
                            move || probe_replay_header(&p)
                        })
                        .await
                        {
                            Ok(Ok(probe)) => probe,
                            Ok(Err(e)) => {
                                warn!(file = %path.display(), error = %e, "Header probe failed");
                                return;
                            }
                            Err(e) => {
                                warn!(file = %path.display(), error = %e, "Probe task panicked");
                                return;
                            }
                        };

                        processed_clone.write().await.insert((path.clone(), size));

                        // Stagger Clan Battle preflight calls (1–4s) so caller & backup don't burst simultaneously.
                        if probe.is_clan_battle {
                            let jitter_ms = 1000 + (uuid::Uuid::new_v4().as_u128() % 3000) as u64;
                            tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
                        }

                        let token = token_clone.read().await.clone();
                        match uploader_clone
                            .process_replay(&path, &probe, token.as_deref())
                            .await
                        {
                            Ok(outcome) => {
                                let msg = match outcome {
                                    UploadOutcome::SkippedNonCb(r) => r,
                                    UploadOutcome::SkippedDuplicate(r) => format!("Duplicate skipped: {r}"),
                                    UploadOutcome::WinStatsRecorded => {
                                        format!("CB Win stats recorded: {}", probe.filename)
                                    }
                                    UploadOutcome::UploadedForRender { action } => {
                                        format!("Queued ({action}): {}", probe.filename)
                                    }
                                };
                                info!(event = %msg);
                                status_clone.write().await.last_event = Some(msg);
                            }
                            Err(UploadError::Unauthorized) => {
                                warn!("Session token expired (401); clearing stored credential");
                                clear_session_token();
                                *token_clone.write().await = None;
                                let mut st = status_clone.write().await;
                                st.linked_account = None;
                                st.last_event = Some("Session expired — please re-link WG account".to_string());
                            }
                            Err(e) => {
                                error!(file = %probe.filename, error = %e, "Replay upload failed");
                                status_clone.write().await.last_event = Some(format!("Upload error: {e}"));
                            }
                        }
                    });
                }
            }
        }
    });
}

mod auth;
mod replay;
mod uploader;
mod watcher;
mod wows;

use auth::{
    clear_session_token, format_account_label, load_session_token, start_loopback_login,
};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::{mpsc, RwLock};
use watcher::WatcherStatus;
use wows::{load_config, normalize_replays_dir, resolve_replays_dir, save_config, AppConfig};

struct TrayHandles {
    tray: TrayIcon,
    status_item: MenuItem<tauri::Wry>,
    account_item: MenuItem<tauri::Wry>,
    toggle_item: MenuItem<tauri::Wry>,
}

struct AppState {
    status: Arc<RwLock<WatcherStatus>>,
    config: Arc<RwLock<AppConfig>>,
    token: Arc<RwLock<Option<String>>>,
    dir_tx: mpsc::Sender<Option<PathBuf>>,
    tray_handles: RwLock<Option<TrayHandles>>,
}

pub async fn refresh_tray_ui(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let st = state.status.read().await.clone();
    let guard = state.tray_handles.read().await;
    if let Some(handles) = guard.as_ref() {
        let _ = handles.status_item.set_text(format!("Watcher: {}", st.state));
        let acct = st
            .linked_account
            .as_deref()
            .unwrap_or("Not Linked");
        let _ = handles.account_item.set_text(format!("Account: {acct}"));
        let _ = handles.toggle_item.set_text(if st.paused {
            "Resume Watcher"
        } else {
            "Pause Watcher"
        });
        let tooltip = format!("InkPads — {} ({acct})", st.state);
        let _ = handles.tray.set_tooltip(Some(tooltip));
    }
}

#[tauri::command]
async fn get_watcher_status(state: State<'_, AppState>) -> Result<WatcherStatus, String> {
    Ok(state.status.read().await.clone())
}

#[tauri::command]
async fn toggle_watcher(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<WatcherStatus, String> {
    let paused = {
        let mut cfg = state.config.write().await;
        cfg.paused = !cfg.paused;
        let _ = save_config(&cfg);
        cfg.paused
    };
    let snapshot = {
        let mut st = state.status.write().await;
        st.paused = paused;
        if st.replays_dir.is_some() {
            st.state = if paused {
                "Paused".to_string()
            } else {
                "Watching".to_string()
            };
        }
        st.clone()
    };
    refresh_tray_ui(&app).await;
    Ok(snapshot)
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn pick_wows_folder(app: tauri::AppHandle) {
    tracing::info!("Opening WoWS folder picker");
    app.dialog().file().pick_folder(move |folder| {
        let Some(folder_path) = folder else {
            tracing::info!("WoWS folder selection cancelled");
            return;
        };
        let Ok(raw_path) = folder_path.into_path() else {
            return;
        };
        let Some(replays_dir) = normalize_replays_dir(&raw_path) else {
            tracing::warn!(path = %raw_path.display(), "Selected folder is not a WoWS root or replays directory");
            return;
        };
        tracing::info!(path = %replays_dir.display(), "Updated WoWS replays folder");
        let app_clone = app.clone();
        let state = app.state::<AppState>();
        let dir_tx = state.dir_tx.clone();
        let config = Arc::clone(&state.config);
        tauri::async_runtime::spawn(async move {
            {
                let mut cfg = config.write().await;
                cfg.wows_replays_dir = Some(replays_dir.clone());
                let _ = save_config(&cfg);
            }
            let _ = dir_tx.send(Some(replays_dir)).await;
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            refresh_tray_ui(&app_clone).await;
        });
    });
}

fn trigger_link_account(app: tauri::AppHandle, region: &'static str) {
    tracing::info!(region, "Starting WG account link flow");
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let token_slot = Arc::clone(&state.token);
        let config = Arc::clone(&state.config);
        let status = Arc::clone(&state.status);
        if let Err(e) = start_loopback_login(app.clone(), region, token_slot, config, status.clone()).await {
            tracing::warn!(error = %e, "WG account link failed");
            status.write().await.last_event = Some(format!("Link failed: {e}"));
        }
    });
}

fn check_for_updates(app: tauri::AppHandle, status: Arc<RwLock<WatcherStatus>>, user_initiated: bool) {
    tauri::async_runtime::spawn(async move {
        if user_initiated {
            tracing::info!("Checking for InkPads Desktop updates...");
        }
        let Ok(updater) = app.updater() else {
            return;
        };
        match updater.check().await {
            Ok(Some(update)) => {
                let version = update.version.clone();
                tracing::info!(%version, "Desktop update available; downloading and installing");
                status.write().await.last_event = Some(format!("Downloading update v{version}..."));
                if let Err(e) = update.download_and_install(|_, _| {}, || {}).await {
                    tracing::warn!(error = %e, "Failed to install update");
                    if user_initiated {
                        status.write().await.last_event = Some(format!("Update failed: {e}"));
                    }
                } else {
                    tracing::info!(%version, "Update installed; restarting");
                    app.restart();
                }
            }
            Ok(None) => {
                if user_initiated {
                    tracing::info!("InkPads Desktop is up to date");
                    status.write().await.last_event = Some("InkPads Desktop is up to date".to_string());
                }
            }
            Err(e) => {
                if user_initiated {
                    tracing::info!(error = %e, "Update check unavailable (no published release manifest yet)");
                    status.write().await.last_event = Some(format!("Update check unavailable: {e}"));
                } else {
                    tracing::debug!(error = %e, "Background update check skipped");
                }
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "inkpads_desktop_lib=info".into()),
        )
        .init();

    let cfg = load_config();
    let initial_dir = resolve_replays_dir(&cfg);
    let initial_paused = cfg.paused;
    let initial_token = load_session_token();
    let initial_account = if initial_token.is_some() {
        format_account_label(cfg.linked_nickname.as_deref(), cfg.linked_clan.as_deref())
    } else {
        None
    };

    let initial_state_str = if initial_dir.is_some() {
        if initial_paused {
            "Paused".to_string()
        } else {
            "Watching".to_string()
        }
    } else {
        "WoWS Folder Not Found".to_string()
    };

    let status = Arc::new(RwLock::new(WatcherStatus {
        replays_dir: initial_dir.as_ref().map(|p| p.display().to_string()),
        paused: initial_paused,
        state: initial_state_str.clone(),
        linked_account: initial_account.clone(),
        last_event: None,
    }));
    let config = Arc::new(RwLock::new(cfg));
    let token = Arc::new(RwLock::new(initial_token));
    let (dir_tx, dir_rx) = mpsc::channel::<Option<PathBuf>>(8);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState {
            status: Arc::clone(&status),
            config: Arc::clone(&config),
            token: Arc::clone(&token),
            dir_tx: dir_tx.clone(),
            tray_handles: RwLock::new(None),
        })
        .invoke_handler(tauri::generate_handler![get_watcher_status, toggle_watcher])
        .setup(move |app| {
            watcher::spawn_watcher(
                Arc::clone(&status),
                Arc::clone(&token),
                dir_rx,
                initial_dir.clone(),
            );
            check_for_updates(app.handle().clone(), Arc::clone(&status), false);

            let status_item = MenuItem::with_id(
                app,
                "status_label",
                format!("Watcher: {initial_state_str}"),
                false,
                None::<&str>,
            )?;
            let acct_label = initial_account.as_deref().unwrap_or("Not Linked");
            let account_item = MenuItem::with_id(
                app,
                "account_label",
                format!("Account: {acct_label}"),
                false,
                None::<&str>,
            )?;

            let open_item = MenuItem::with_id(app, "open", "Open InkPads", true, None::<&str>)?;
            let toggle_text = if initial_paused {
                "Resume Watcher"
            } else {
                "Pause Watcher"
            };
            let toggle_item =
                MenuItem::with_id(app, "toggle", toggle_text, true, None::<&str>)?;
            let folder_item =
                MenuItem::with_id(app, "folder", "Select WoWS Folder...", true, None::<&str>)?;

            let link_na = MenuItem::with_id(app, "link_com", "North America (NA)", true, None::<&str>)?;
            let link_eu = MenuItem::with_id(app, "link_eu", "Europe (EU)", true, None::<&str>)?;
            let link_asia = MenuItem::with_id(app, "link_asia", "Asia (ASIA)", true, None::<&str>)?;
            let link_submenu =
                Submenu::with_items(app, "Link WG Account", true, &[&link_na, &link_eu, &link_asia])?;
            let unlink_item =
                MenuItem::with_id(app, "unlink", "Sign Out", true, None::<&str>)?;
            let update_item =
                MenuItem::with_id(app, "update", "Check for Updates", true, None::<&str>)?;

            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let sep3 = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit InkPads", true, None::<&str>)?;

            let menu = Menu::with_items(
                app,
                &[
                    &status_item,
                    &account_item,
                    &sep1,
                    &open_item,
                    &toggle_item,
                    &folder_item,
                    &sep2,
                    &link_submenu,
                    &unlink_item,
                    &update_item,
                    &sep3,
                    &quit_item,
                ],
            )?;

            let mut tray_builder = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip(format!("InkPads — {initial_state_str} ({acct_label})"))
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main_window(app),
                    "toggle" => {
                        let app_handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app_handle.state::<AppState>();
                            let paused = {
                                let mut cfg = state.config.write().await;
                                cfg.paused = !cfg.paused;
                                let _ = save_config(&cfg);
                                cfg.paused
                            };
                            {
                                let mut st = state.status.write().await;
                                st.paused = paused;
                                if st.replays_dir.is_some() {
                                    st.state = if paused {
                                        "Paused".to_string()
                                    } else {
                                        "Watching".to_string()
                                    };
                                }
                            }
                            tracing::info!(paused, "Toggled replay watcher");
                            refresh_tray_ui(&app_handle).await;
                        });
                    }
                    "folder" => pick_wows_folder(app.clone()),
                    "link_com" => trigger_link_account(app.clone(), "com"),
                    "link_eu" => trigger_link_account(app.clone(), "eu"),
                    "link_asia" => trigger_link_account(app.clone(), "asia"),
                    "unlink" => {
                        let app_handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            clear_session_token();
                            let state = app_handle.state::<AppState>();
                            *state.token.write().await = None;
                            {
                                let mut cfg = state.config.write().await;
                                cfg.linked_nickname = None;
                                cfg.linked_clan = None;
                                let _ = save_config(&cfg);
                            }
                            {
                                let mut st = state.status.write().await;
                                st.linked_account = None;
                                st.last_event = Some("Signed out".to_string());
                            }
                            // Also log out the WebView session (mirrors clicking LOGOUT in the web header).
                            if let Some(win) = app_handle.get_webview_window("main") {
                                let _ = win.eval(
                                    "fetch('/api/auth/logout', { method: 'POST', credentials: 'include' }).finally(() => { try { sessionStorage.clear(); } catch(_) {} window.location.href = '/'; });",
                                );
                            }
                            tracing::info!("Signed out desktop & web session");
                            refresh_tray_ui(&app_handle).await;
                        });
                    }
                    "update" => {
                        let state = app.state::<AppState>();
                        check_for_updates(app.clone(), Arc::clone(&state.status), true);
                    }
                    "quit" => {
                        tracing::info!("Quitting InkPads Desktop");
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });

            if let Some(icon) = app.default_window_icon().cloned() {
                tray_builder = tray_builder.icon(icon);
            }

            let tray = tray_builder.build(app)?;
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<AppState>();
                *state.tray_handles.write().await = Some(TrayHandles {
                    tray,
                    status_item,
                    account_item,
                    toggle_item,
                });
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Keep running in the system tray when the main window is closed.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running InkPads Desktop");
}

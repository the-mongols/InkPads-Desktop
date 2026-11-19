use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub wows_replays_dir: Option<PathBuf>,
    pub paused: bool,
    pub linked_nickname: Option<String>,
    pub linked_clan: Option<String>,
}

fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "pages.inkpads", "InkPadsDesktop")
        .map(|dirs| dirs.config_dir().join("config.json"))
}

pub fn load_config() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return AppConfig::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_config(cfg: &AppConfig) -> Result<(), String> {
    let path = config_path().ok_or_else(|| "Unable to resolve config directory".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write config: {e}"))?;
    Ok(())
}

/// Normalizes a user-selected WoWS root or replays folder into the `replays` directory.
pub fn normalize_replays_dir(selected: &Path) -> Option<PathBuf> {
    if !selected.exists() || !selected.is_dir() {
        return None;
    }
    let name = selected
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name == "replays" {
        return Some(selected.to_path_buf());
    }
    let nested = selected.join("replays");
    if nested.exists() && nested.is_dir() {
        return Some(nested);
    }
    None
}

/// Probes common World of Warships installation roots on Windows for a `replays` directory.
pub fn detect_replays_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from(r"C:\Games\World_of_Warships\replays"),
        PathBuf::from(r"C:\Games\World_of_Warships_NA\replays"),
        PathBuf::from(r"C:\Games\World_of_Warships_EU\replays"),
        PathBuf::from(r"C:\Games\World_of_Warships_ASIA\replays"),
        PathBuf::from(r"D:\Games\World_of_Warships\replays"),
        PathBuf::from(r"E:\Games\World_of_Warships\replays"),
        PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\World of Warships\replays"),
        PathBuf::from(r"D:\SteamLibrary\steamapps\common\World of Warships\replays"),
        PathBuf::from(r"E:\SteamLibrary\steamapps\common\World of Warships\replays"),
        PathBuf::from(r"C:\Program Files\Epic Games\WorldOfWarships\replays"),
    ];

    // Also scan C:\Games, D:\Games, E:\Games for any World_of_Warships* directory.
    for root in [r"C:\Games", r"D:\Games", r"E:\Games"] {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let fname = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if fname.contains("world_of_warships") || fname.contains("world of warships") {
                        candidates.push(path.join("replays"));
                    }
                }
            }
        }
    }

    candidates.into_iter().find(|p| p.exists() && p.is_dir())
}

pub fn resolve_replays_dir(cfg: &AppConfig) -> Option<PathBuf> {
    if let Some(saved) = &cfg.wows_replays_dir {
        if let Some(norm) = normalize_replays_dir(saved) {
            return Some(norm);
        }
    }
    detect_replays_dir()
}

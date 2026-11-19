use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const REPLAY_MAGIC: [u8; 4] = [0x12, 0x32, 0x34, 0x11];
const MAX_HEADER_JSON_BYTES: usize = 4 * 1024 * 1024; // 4 MiB safety cap

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayProbe {
    pub client_id: String,
    pub filename: String,
    pub file_size: u64,
    pub date_time: String,
    pub map_name: String,
    pub match_group: String,
    pub game_type: String,
    pub scenario: String,
    pub scenario_config: String,
    pub player_name: String,
    pub player_vehicle: String,
    pub vehicles: Vec<Value>,
    pub arena_id: Option<String>,
    pub friendly_clan: String,
    pub enemy_clan: String,
    pub player_team: i64,
    pub result: String,
    pub fingerprint: String,
    pub match_type: String,
    pub is_clan_battle: bool,
    pub uploader_name: String,
    pub clan_id: u64,
}

/// Strictly mirrors `ReplayParser.isSpectator` in `apps/web/src/core/ReplayParser.js`
/// and `apps/worker/src/replay_parser.js`.
pub fn is_spectator(v: &Value) -> bool {
    let relation = v.get("relation").and_then(Value::as_i64);
    let is_spec_camel = v.get("isSpectator").and_then(Value::as_bool).unwrap_or(false);
    let is_spec_snake = v.get("is_spectator").and_then(Value::as_bool).unwrap_or(false);
    let ship_id = v.get("shipId").and_then(Value::as_i64);
    let team_id = v.get("teamId").and_then(Value::as_i64);

    relation == Some(3)
        || is_spec_camel
        || is_spec_snake
        || ship_id == Some(0)
        || team_id == Some(-1)
        || !matches!(relation, Some(0) | Some(1) | Some(2))
}

fn value_as_str<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str) {
            return s;
        }
    }
    ""
}

fn value_as_id_string(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(val) = v.get(*k) {
            if let Some(s) = val.as_str() {
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            } else if let Some(n) = val.as_u64() {
                if n > 0 {
                    return Some(n.to_string());
                }
            } else if let Some(n) = val.as_i64() {
                if n > 0 {
                    return Some(n.to_string());
                }
            }
        }
    }
    None
}

fn most_common_clan(vehicles: &[&Value]) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for v in vehicles {
        if let Some(clan) = v.get("clan").and_then(Value::as_str) {
            let trimmed = clan.trim();
            if !trimmed.is_empty() {
                *counts.entry(trimmed.to_string()).or_insert(0) += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(clan, _)| clan)
        .unwrap_or_default()
}

/// Reads only the 12-byte header + JSON metadata block from a `.wowsreplay` file.
pub fn probe_replay_header(path: &Path) -> Result<ReplayProbe, String> {
    let mut file = File::open(path).map_err(|e| format!("Failed to open replay: {e}"))?;
    let file_size = file
        .metadata()
        .map_err(|e| format!("Failed to stat replay: {e}"))?
        .len();

    if file_size < 12 {
        return Err("Replay file too small (< 12 bytes)".to_string());
    }

    let mut header = [0u8; 12];
    file.read_exact(&mut header)
        .map_err(|e| format!("Failed to read 12-byte header: {e}"))?;

    if header[0..4] != REPLAY_MAGIC {
        return Err("Invalid replay file: Magic number mismatch".to_string());
    }

    let meta_len = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
    if meta_len == 0 || meta_len > MAX_HEADER_JSON_BYTES || (12 + meta_len as u64) > file_size {
        return Err(format!("Invalid replay JSON header length: {meta_len}"));
    }

    file.seek(SeekFrom::Start(12))
        .map_err(|e| format!("Failed to seek to JSON header: {e}"))?;
    let mut json_buf = vec![0u8; meta_len];
    file.read_exact(&mut json_buf)
        .map_err(|e| format!("Failed to read JSON header: {e}"))?;

    let meta: Value =
        serde_json::from_slice(&json_buf).map_err(|e| format!("Invalid replay JSON: {e}"))?;

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("replay.wowsreplay")
        .to_string();

    let vehicles = meta
        .get("vehicles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let combatants: Vec<&Value> = vehicles.iter().filter(|v| !is_spectator(v)).collect();

    let mut player_ids: Vec<i64> = combatants
        .iter()
        .filter_map(|v| v.get("id").and_then(Value::as_i64))
        .collect();
    player_ids.sort_unstable();
    let ids_joined = player_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let date_time = value_as_str(&meta, &["dateTime", "date_time"]).to_string();
    let fingerprint = format!("{date_time}_{ids_joined}");

    let player_name = value_as_str(&meta, &["playerName", "player_name"]).to_string();
    let mut friendly_clan = value_as_str(&meta, &["playerClan", "player_clan"]).to_string();

    if friendly_clan.is_empty() && !player_name.is_empty() {
        for v in &combatants {
            if v.get("name").and_then(Value::as_str) == Some(player_name.as_str()) {
                if let Some(c) = v.get("clan").and_then(Value::as_str) {
                    if !c.is_empty() {
                        friendly_clan = c.to_string();
                        break;
                    }
                }
            }
        }
    }

    let friendly_vehicles: Vec<&Value> = combatants
        .iter()
        .copied()
        .filter(|v| matches!(v.get("relation").and_then(Value::as_i64), Some(0) | Some(1)))
        .collect();
    let enemy_vehicles: Vec<&Value> = combatants
        .iter()
        .copied()
        .filter(|v| v.get("relation").and_then(Value::as_i64) == Some(2))
        .collect();

    if friendly_clan.is_empty() && !friendly_vehicles.is_empty() {
        friendly_clan = most_common_clan(&friendly_vehicles);
    }
    let enemy_clan = most_common_clan(&enemy_vehicles);

    let match_group = value_as_str(&meta, &["matchGroup", "match_group"]).to_string();
    let game_type = value_as_str(&meta, &["gameType", "game_type"]).to_string();
    let scenario = value_as_str(&meta, &["scenarioConfig", "scenario", "gameLogic"]).to_string();

    let match_group_lower = match_group.to_ascii_lowercase();
    let game_type_upper = game_type.to_ascii_uppercase();
    let scenario_upper = scenario.to_ascii_uppercase();

    let is_tournament = scenario_upper.contains("TOURNAMENT")
        || scenario_upper.contains("FOUR AREAS")
        || scenario_upper.contains("FOUR_AREAS")
        || match_group_lower.contains("tournament")
        || match_group_lower.contains("kots");

    let is_training = match_group_lower.contains("training")
        || match_group_lower.contains("custom")
        || game_type_upper.contains("TRAINING");

    let is_intra_clan = !friendly_clan.is_empty()
        && !enemy_clan.is_empty()
        && friendly_clan.eq_ignore_ascii_case(&enemy_clan);

    let is_clan_battle = !is_tournament
        && !is_training
        && !is_intra_clan
        && (match_group_lower.contains("clan")
            || match_group_lower.contains("cvc")
            || game_type_upper == "CLANBATTLE"
            || game_type_upper == "CW"
            || scenario_upper.contains("CVC"));

    let match_type = if is_tournament {
        "kots"
    } else if is_training || is_intra_clan {
        if is_intra_clan {
            "scrim"
        } else {
            "training"
        }
    } else if is_clan_battle {
        "clan_battle"
    } else if match_group_lower == "pvp" || game_type_upper.contains("RANDOM") {
        "random"
    } else if match_group_lower == "ranked" || game_type_upper.contains("RANKED") {
        "ranked"
    } else if match_group_lower == "cooperative"
        || match_group_lower == "coop"
        || game_type_upper.contains("COOP")
    {
        "coop"
    } else if match_group_lower == "brawl" {
        "brawl"
    } else if match_group_lower == "pve" {
        "operations"
    } else {
        "unknown"
    }
    .to_string();

    let map_name = value_as_str(&meta, &["mapDisplayName", "mapName"]).to_string();
    let player_vehicle = value_as_str(&meta, &["playerVehicle", "player_vehicle"]).to_string();
    let arena_id = value_as_id_string(&meta, &["arenaUniqueID", "arenaId", "arena_id"]);
    let player_team = meta
        .get("playerTeam")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    let result = meta
        .get("matchResult")
        .or_else(|| meta.get("result"))
        .and_then(Value::as_str)
        .or_else(|| {
            meta.get("gameos_results")
                .and_then(|g| g.get("result"))
                .and_then(Value::as_str)
        })
        .unwrap_or("unknown")
        .to_ascii_lowercase();

    let uploader_name = if player_name.is_empty() {
        "Teammate".to_string()
    } else {
        player_name.clone()
    };

    Ok(ReplayProbe {
        client_id: format!("desktop_{fingerprint}_{filename}"),
        filename,
        file_size,
        date_time,
        map_name,
        match_group,
        game_type,
        scenario: scenario.clone(),
        scenario_config: scenario,
        player_name,
        player_vehicle,
        vehicles,
        arena_id,
        friendly_clan,
        enemy_clan,
        player_team,
        result,
        fingerprint,
        match_type,
        is_clan_battle,
        uploader_name,
        clan_id: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use uuid::Uuid;

    fn write_temp_replay(json_str: &str) -> std::path::PathBuf {
        let id = Uuid::new_v4().simple();
        let path = std::env::temp_dir().join(format!("inkpads_test_{id}.wowsreplay"));
        let mut f = File::create(&path).unwrap();
        f.write_all(&REPLAY_MAGIC).unwrap();
        f.write_all(&1u32.to_le_bytes()).unwrap();
        f.write_all(&(json_str.len() as u32).to_le_bytes()).unwrap();
        f.write_all(json_str.as_bytes()).unwrap();
        path
    }

    #[test]
    fn probes_clan_battle_and_quarantines_spectators() {
        let json = r#"{
            "dateTime": "18.11.2026 20:15:00",
            "matchGroup": "clan",
            "gameType": "ClanBattle",
            "mapDisplayName": "Hotspot",
            "playerName": "CaptA",
            "playerClan": "KITED",
            "vehicles": [
                {"id": 100, "name": "CaptA", "clan": "KITED", "relation": 0, "shipId": 4180, "teamId": 0},
                {"id": 200, "name": "EnemyB", "clan": "ENEMY", "relation": 2, "shipId": 4181, "teamId": 1},
                {"id": 999, "name": "SpecZ", "clan": "SPEC", "relation": 3, "shipId": 0, "teamId": -1}
            ]
        }"#;
        let path = write_temp_replay(json);
        let probe = probe_replay_header(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(probe.is_clan_battle);
        assert_eq!(probe.match_type, "clan_battle");
        assert_eq!(probe.friendly_clan, "KITED");
        assert_eq!(probe.enemy_clan, "ENEMY");
        assert_eq!(probe.fingerprint, "18.11.2026 20:15:00_100,200");
    }

    #[test]
    fn rejects_intra_clan_scrim() {
        let json = r#"{
            "dateTime": "18.11.2026 20:15:00",
            "matchGroup": "clan",
            "playerName": "CaptA",
            "vehicles": [
                {"id": 100, "name": "CaptA", "clan": "KITED", "relation": 0, "shipId": 4180, "teamId": 0},
                {"id": 200, "name": "MateB", "clan": "KITED", "relation": 2, "shipId": 4181, "teamId": 1}
            ]
        }"#;
        let path = write_temp_replay(json);
        let probe = probe_replay_header(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(!probe.is_clan_battle);
        assert_eq!(probe.match_type, "scrim");
    }
}

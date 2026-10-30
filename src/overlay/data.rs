use crate::debug_log;
use serde::Deserialize;
use std::collections::HashMap;
use std::{fs, path::Path};

use std::sync::{Arc, RwLock};

#[derive(Default)]
pub struct AppState {
    pub key_item_quantity: u32,
    pub event_flags: HashMap<i32, bool>,
    pub goal_complete: bool,
    pub initialized: bool,
    pub death_count: u32,
    pub great_runes: i32,
    pub player_check_count: u32,
    pub team_unique_check_count: u32,
    pub player_shard_count: u32,
    pub team_unique_shard_count: u32,
    pub player_great_rune_count: u32,
    pub team_unique_great_rune_count: u32,
    pub required_unique_shards: Option<u32>,
    pub shard_requirement_met: bool,
    pub required_unique_great_runes: Option<u32>,
    pub great_rune_requirement_met: bool,
    pub victory_flag_reached: bool,
    pub finish_requirements_met: bool,
    pub player_finish_igt_ms: Option<u64>,
    pub teammate_current_igt_ms: Option<u64>,
    pub teammate_finish_igt_ms: Option<u64>,
    pub team_finished: bool,
    pub combined_finish_igt_ms: Option<u64>,
    pub combined_current_igt_ms: Option<u64>,
    pub combined_check_count: Option<u32>,
    pub combined_death_count: Option<u32>,
    pub total_check_count: u32,
    pub pending_event_flags: Vec<PendingEventFlag>,
    pub applied_event_delivery_ids: Vec<String>,
    pub pending_death_links: Vec<PendingDeathLink>,
    pub applied_death_link_delivery_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingEventFlag {
    pub delivery_id: String,
    pub event_flag: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDeathLink {
    pub delivery_id: String,
}

pub type SharedState = Arc<RwLock<AppState>>;

pub fn create_state() -> SharedState {
    Arc::new(RwLock::new(AppState::default()))
}

#[derive(Debug, Deserialize, Clone)]
pub struct BossEntry {
    pub boss: String,
    pub place: String,
    pub flag_id: i32,
    #[serde(default)]
    pub remembrance: Option<i32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RegionData {
    pub region_name: String,
    #[serde(default)]
    pub regions: Vec<i32>,
    #[serde(default)]
    pub bosses: Vec<BossEntry>,
}

pub type BossRegions = Vec<RegionData>;

/// Loads the localized boss data JSON file according to `language` in config.
///
/// Search order:
/// 1️⃣ `data/<language>/bosses.json`
/// 2️⃣ `data/engus/bosses.json`
/// 3️⃣ `data/bosses.json`
pub fn load_localized_boss_data(
    config_dir: &Path,
    language: &str,
    data_file: &str,
) -> Option<BossRegions> {
    // normalize language (trim + lowercase for directory consistency)
    let lang_norm = language.trim();
    let lang_norm = if lang_norm.is_empty() {
        "engus"
    } else {
        lang_norm
    };

    // construct candidate search order
    let candidates = [
        config_dir.join(format!("data/{}/{}", lang_norm, data_file)),
        config_dir.join(format!("data/engus/{}", data_file)),
        config_dir.join(format!("data/{}", data_file)),
    ];

    for path in candidates {
        if !path.exists() {
            debug_log!(
                "[absoloot_overlay] ⚠ Boss data not found at '{}'",
                path.display()
            );
            continue;
        }

        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<BossRegions>(&contents) {
                Ok(data) => {
                    debug_log!(
                        "[absoloot_overlay] ✅ Loaded boss data from '{}'",
                        path.display()
                    );
                    return Some(data);
                }
                Err(e) => {
                    debug_log!(
                        "[absoloot_overlay] ❌ Failed to parse JSON '{}': {:?}",
                        path.display(),
                        e
                    );
                }
            },
            Err(e) => {
                debug_log!(
                    "[absoloot_overlay] ❌ Could not read '{}': {:?}",
                    path.display(),
                    e
                );
            }
        }
    }

    debug_log!("[absoloot_overlay] ❌ No valid boss data file found");
    None
}

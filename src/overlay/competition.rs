use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam::channel::{Receiver, Sender, bounded};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::{
    debug_log,
    overlay::{data::SharedState, game_monitor::MonitorObservation},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const ERROR_LOG_INTERVAL: Duration = Duration::from_secs(30);
const TELEMETRY_QUEUE_CAPACITY: usize = 8;
const DEFAULT_DEATH_LINK_REBOUND_SECONDS: u64 = 30;

fn default_seed_name() -> String {
    "Absoloot Chaos Seed".to_string()
}

fn default_death_link_rebound_seconds() -> u64 {
    DEFAULT_DEATH_LINK_REBOUND_SECONDS
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollectibleDisplayMode {
    #[default]
    Shards,
    GreatRunes,
    Both,
}

impl CollectibleDisplayMode {
    pub fn shows_shards(self) -> bool {
        matches!(self, Self::Shards | Self::Both)
    }

    pub fn shows_great_runes(self) -> bool {
        matches!(self, Self::GreatRunes | Self::Both)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EventFlagMirror {
    pub source_event_flag: i32,
    pub target_event_flag: i32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompetitionSettings {
    #[serde(default = "default_seed_name")]
    pub seed_name: String,
    pub api_url: String,
    pub assignment_id: String,
    pub assignment_token: String,
    pub seed_id: String,
    #[serde(default)]
    pub win_condition_flag: i32,
    #[serde(default)]
    pub shard_event_flags: Vec<i32>,
    #[serde(default)]
    pub great_rune_event_flags: Vec<i32>,
    #[serde(default)]
    pub event_flag_mirrors: Vec<EventFlagMirror>,
    #[serde(default)]
    pub required_unique_shards: Option<u32>,
    #[serde(default)]
    pub required_unique_great_runes: Option<u32>,
    #[serde(default)]
    pub collectible_display_mode: CollectibleDisplayMode,
    #[serde(default)]
    pub death_link_enabled: bool,
    #[serde(default = "default_death_link_rebound_seconds")]
    pub death_link_rebound_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompetitionFile {
    competition: CompetitionSettings,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestRequest<'a> {
    version: u8,
    action: &'static str,
    assignment_id: &'a str,
    seed_id: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestResponse {
    version: u8,
    seed_id: String,
    win_condition_flag: i32,
    required_unique_shards: Option<u32>,
    #[serde(default)]
    required_unique_great_runes: Option<u32>,
    #[serde(default)]
    collectible_display_mode: CollectibleDisplayMode,
    shard_flags: Vec<ManifestShard>,
    #[serde(default)]
    great_rune_flags: Vec<ManifestShard>,
    #[serde(default)]
    event_flag_mirrors: Vec<ManifestEventFlagMirror>,
    #[serde(default)]
    death_link_enabled: bool,
    #[serde(default = "default_death_link_rebound_seconds")]
    death_link_rebound_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestEventFlagMirror {
    source_event_flag: i32,
    target_event_flag: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestShard {
    event_flag: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotRequest<'a> {
    version: u8,
    action: &'static str,
    assignment_id: &'a str,
    seed_id: &'a str,
    active_event_flags: &'a [i32],
    active_check_flags: &'a [i32],
    active_mirror_flags: &'a [i32],
    current_igt_ms: u32,
    death_count: u32,
    check_count: u32,
    shard_count: u32,
    applied_event_delivery_ids: &'a [String],
    applied_death_link_delivery_ids: &'a [String],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotResponse {
    player_check_count: u32,
    team_unique_check_count: u32,
    player_shard_count: u32,
    team_unique_shard_count: u32,
    #[serde(default)]
    player_great_rune_count: u32,
    #[serde(default)]
    team_unique_great_rune_count: u32,
    required_unique_shards: Option<u32>,
    shard_requirement_met: bool,
    #[serde(default)]
    required_unique_great_runes: Option<u32>,
    #[serde(default = "default_true")]
    great_rune_requirement_met: bool,
    victory_flag_reached: bool,
    finish_requirements_met: bool,
    player_finish_igt_ms: Option<u64>,
    teammate_current_igt_ms: Option<u64>,
    teammate_finish_igt_ms: Option<u64>,
    team_finished: bool,
    combined_finish_igt_ms: Option<u64>,
    combined_current_igt_ms: Option<u64>,
    combined_check_count: Option<u32>,
    combined_death_count: Option<u32>,
    #[serde(default)]
    event_flag_deliveries: Vec<EventFlagDelivery>,
    #[serde(default)]
    death_link_deliveries: Vec<DeathLinkDelivery>,
}

#[derive(Debug, Deserialize)]
struct EventFlagDelivery {
    #[serde(alias = "deliveryId")]
    delivery_id: String,
    #[serde(alias = "eventFlag")]
    event_flag: i32,
}

#[derive(Debug, Deserialize)]
struct DeathLinkDelivery {
    #[serde(alias = "deliveryId")]
    delivery_id: String,
}

pub struct CompetitionRuntime {
    settings: Option<CompetitionSettings>,
    sender: Option<Sender<MonitorObservation>>,
    receiver: Option<Receiver<MonitorObservation>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl CompetitionRuntime {
    pub fn load(directory: &Path) -> (Self, Option<String>) {
        let path = directory.join("competition_config.toml");
        if !path.exists() {
            return (Self::disabled(), None);
        }

        let settings = fs::read_to_string(&path)
            .map_err(|error| format!("Failed to read {}: {error}", path.display()))
            .and_then(|source| {
                toml::from_str::<CompetitionFile>(&source)
                    .map_err(|error| format!("Failed to parse {}: {error}", path.display()))
            })
            .and_then(|file| validate_settings(file.competition))
            .and_then(fetch_manifest);

        match settings {
            Ok(settings) => {
                let (sender, receiver) = bounded(TELEMETRY_QUEUE_CAPACITY);
                (
                    Self {
                        settings: Some(settings),
                        sender: Some(sender),
                        receiver: Some(receiver),
                        stop: Arc::new(AtomicBool::new(false)),
                        worker: None,
                    },
                    None,
                )
            }
            Err(error) => (Self::disabled(), Some(error)),
        }
    }

    fn disabled() -> Self {
        Self {
            settings: None,
            sender: None,
            receiver: None,
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    pub fn settings(&self) -> Option<&CompetitionSettings> {
        self.settings.as_ref()
    }

    pub fn sender(&self) -> Option<Sender<MonitorObservation>> {
        self.sender.clone()
    }

    pub fn start(&mut self, state: SharedState) {
        self.stop();
        let (Some(settings), Some(receiver)) = (self.settings.clone(), self.receiver.take()) else {
            return;
        };
        self.stop.store(false, Ordering::Release);
        let stop = self.stop.clone();
        self.worker = thread::Builder::new()
            .name("absoloot-telemetry".to_string())
            .spawn(move || upload_loop(settings, receiver, state, stop))
            .ok();
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for CompetitionRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn validate_settings(mut settings: CompetitionSettings) -> Result<CompetitionSettings, String> {
    if settings.seed_name.trim().is_empty() {
        return Err("Competition seed_name is required.".to_string());
    }
    if !(settings.api_url.starts_with("https://")
        || settings.api_url.starts_with("http://localhost"))
    {
        return Err("Competition api_url must use HTTPS (or localhost HTTP).".to_string());
    }
    if settings.assignment_id.trim().is_empty() || settings.seed_id.trim().is_empty() {
        return Err("Competition assignment_id and seed_id are required.".to_string());
    }
    if settings.assignment_token.len() != 64
        || !settings
            .assignment_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Competition assignment_token must be a 64-character token.".to_string());
    }
    if settings.win_condition_flag < 0
        || settings.shard_event_flags.iter().any(|flag| *flag < 0)
        || settings.great_rune_event_flags.iter().any(|flag| *flag < 0)
        || settings
            .event_flag_mirrors
            .iter()
            .any(|mapping| mapping.source_event_flag < 0 || mapping.target_event_flag < 0)
    {
        return Err("Competition event flags must be non-negative.".to_string());
    }
    if settings.required_unique_shards == Some(0) {
        return Err("Competition required_unique_shards must be greater than zero.".to_string());
    }
    if settings.required_unique_great_runes == Some(0) {
        return Err(
            "Competition required_unique_great_runes must be greater than zero.".to_string(),
        );
    }
    if settings.death_link_rebound_seconds == 0 {
        return Err("Competition death-link settings are invalid.".to_string());
    }
    settings.shard_event_flags.sort_unstable();
    settings.shard_event_flags.dedup();
    settings.great_rune_event_flags.sort_unstable();
    settings.great_rune_event_flags.dedup();
    settings
        .event_flag_mirrors
        .sort_unstable_by_key(|mapping| mapping.source_event_flag);
    settings
        .event_flag_mirrors
        .dedup_by_key(|mapping| mapping.source_event_flag);
    if settings
        .shard_event_flags
        .contains(&settings.win_condition_flag)
        || settings
            .great_rune_event_flags
            .contains(&settings.win_condition_flag)
    {
        return Err("The win condition flag cannot also be a collectible flag.".to_string());
    }
    if settings
        .shard_event_flags
        .iter()
        .any(|flag| settings.great_rune_event_flags.contains(flag))
    {
        return Err("An event flag cannot be both a shard and a great rune.".to_string());
    }
    Ok(settings)
}

fn fetch_manifest(settings: CompetitionSettings) -> Result<CompetitionSettings, String> {
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Could not create the competition HTTP client: {error}"))?;
    let request = ManifestRequest {
        version: 1,
        action: "manifest",
        assignment_id: &settings.assignment_id,
        seed_id: &settings.seed_id,
    };
    let manifest = client
        .post(&settings.api_url)
        .bearer_auth(&settings.assignment_token)
        .json(&request)
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.json::<ManifestResponse>())
        .map_err(|error| format!("Could not load the private seed manifest: {error}"))?;

    apply_manifest(settings, manifest)
}

fn apply_manifest(
    mut settings: CompetitionSettings,
    manifest: ManifestResponse,
) -> Result<CompetitionSettings, String> {
    if manifest.version != 1 || manifest.seed_id != settings.seed_id {
        return Err(
            "The telemetry service returned a manifest for the wrong seed or version.".to_string(),
        );
    }

    settings.win_condition_flag = manifest.win_condition_flag;
    settings.required_unique_shards = manifest.required_unique_shards;
    settings.required_unique_great_runes = manifest.required_unique_great_runes;
    settings.collectible_display_mode = manifest.collectible_display_mode;
    settings.shard_event_flags = manifest
        .shard_flags
        .into_iter()
        .map(|shard| shard.event_flag)
        .collect();
    settings.great_rune_event_flags = manifest
        .great_rune_flags
        .into_iter()
        .map(|great_rune| great_rune.event_flag)
        .collect();
    settings.event_flag_mirrors = manifest
        .event_flag_mirrors
        .into_iter()
        .map(|mapping| EventFlagMirror {
            source_event_flag: mapping.source_event_flag,
            target_event_flag: mapping.target_event_flag,
        })
        .collect();
    settings.death_link_enabled = manifest.death_link_enabled;
    settings.death_link_rebound_seconds = manifest.death_link_rebound_seconds;
    validate_settings(settings)
}

fn upload_loop(
    settings: CompetitionSettings,
    receiver: Receiver<MonitorObservation>,
    state: SharedState,
    stop: Arc<AtomicBool>,
) {
    let client = match Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            debug_log!("[absoloot_telemetry] Could not create HTTP client: {error}");
            return;
        }
    };
    let mut last_error_log: Option<Instant> = None;

    while !stop.load(Ordering::Acquire) {
        let observation = match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(observation) => observation,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => return,
        };

        let applied_event_delivery_ids = state
            .read()
            .map(|state| state.applied_event_delivery_ids.clone())
            .unwrap_or_default();
        let applied_death_link_delivery_ids = state
            .read()
            .map(|state| state.applied_death_link_delivery_ids.clone())
            .unwrap_or_default();
        let request = SnapshotRequest {
            version: 1,
            action: "snapshot",
            assignment_id: &settings.assignment_id,
            seed_id: &settings.seed_id,
            active_event_flags: &observation.active_event_flags,
            active_check_flags: &observation.active_boss_flags,
            active_mirror_flags: &observation.active_mirror_flags,
            current_igt_ms: observation.current_igt_ms,
            death_count: observation.death_count,
            check_count: observation.check_count,
            shard_count: observation.shard_count,
            applied_event_delivery_ids: &applied_event_delivery_ids,
            applied_death_link_delivery_ids: &applied_death_link_delivery_ids,
        };
        let response = client
            .post(&settings.api_url)
            .bearer_auth(&settings.assignment_token)
            .json(&request)
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.json::<SnapshotResponse>());

        match response {
            Ok(response) => {
                last_error_log = None;
                if let Ok(mut state) = state.write() {
                    state
                        .applied_event_delivery_ids
                        .retain(|delivery_id| !applied_event_delivery_ids.contains(delivery_id));
                    state.applied_death_link_delivery_ids.retain(|delivery_id| {
                        !applied_death_link_delivery_ids.contains(delivery_id)
                    });
                    state.player_check_count = response.player_check_count;
                    state.team_unique_check_count = response.team_unique_check_count;
                    state.player_shard_count = response.player_shard_count;
                    state.team_unique_shard_count = response.team_unique_shard_count;
                    state.player_great_rune_count = response.player_great_rune_count;
                    state.team_unique_great_rune_count = response.team_unique_great_rune_count;
                    state.required_unique_shards = response.required_unique_shards;
                    state.shard_requirement_met = response.shard_requirement_met;
                    state.required_unique_great_runes = response.required_unique_great_runes;
                    state.great_rune_requirement_met = response.great_rune_requirement_met;
                    state.victory_flag_reached = response.victory_flag_reached;
                    state.finish_requirements_met = response.finish_requirements_met;
                    state.player_finish_igt_ms = response.player_finish_igt_ms;
                    state.teammate_current_igt_ms = response.teammate_current_igt_ms;
                    state.teammate_finish_igt_ms = response.teammate_finish_igt_ms;
                    state.team_finished = response.team_finished;
                    state.combined_finish_igt_ms = response.combined_finish_igt_ms;
                    state.combined_current_igt_ms = response.combined_current_igt_ms;
                    state.combined_check_count = response.combined_check_count;
                    state.combined_death_count = response.combined_death_count;
                    for delivery in response.event_flag_deliveries {
                        if delivery.event_flag >= 0
                            && !state
                                .pending_event_flags
                                .iter()
                                .any(|pending| pending.delivery_id == delivery.delivery_id)
                        {
                            state.pending_event_flags.push(
                                crate::overlay::data::PendingEventFlag {
                                    delivery_id: delivery.delivery_id,
                                    event_flag: delivery.event_flag,
                                },
                            );
                        }
                    }
                    state.pending_death_links =
                        if settings.death_link_enabled && state.player_finish_igt_ms.is_none() {
                            response
                                .death_link_deliveries
                                .into_iter()
                                .map(|delivery| crate::overlay::data::PendingDeathLink {
                                    delivery_id: delivery.delivery_id,
                                })
                                .collect()
                        } else {
                            Vec::new()
                        };
                }
            }
            Err(error) => {
                let should_log = last_error_log
                    .is_none_or(|logged_at| logged_at.elapsed() >= ERROR_LOG_INTERVAL);
                if should_log {
                    debug_log!("[absoloot_telemetry] Snapshot upload failed: {error}");
                    last_error_log = Some(Instant::now());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CollectibleDisplayMode, CompetitionFile, DEFAULT_DEATH_LINK_REBOUND_SECONDS,
        ManifestEventFlagMirror, ManifestResponse, ManifestShard, SnapshotResponse, apply_manifest,
        validate_settings,
    };

    #[test]
    fn accepts_database_event_delivery_field_names() {
        let response: SnapshotResponse = serde_json::from_str(
            r#"{
                "playerCheckCount": 0,
                "teamUniqueCheckCount": 0,
                "playerShardCount": 0,
                "teamUniqueShardCount": 0,
                "playerGreatRuneCount": 1,
                "teamUniqueGreatRuneCount": 2,
                "requiredUniqueShards": null,
                "shardRequirementMet": true,
                "requiredUniqueGreatRunes": 2,
                "greatRuneRequirementMet": true,
                "victoryFlagReached": false,
                "finishRequirementsMet": false,
                "playerFinishIgtMs": null,
                "teammateCurrentIgtMs": 495667,
                "teammateFinishIgtMs": null,
                "teamFinished": false,
                "combinedFinishIgtMs": null,
                "combinedCurrentIgtMs": 934331,
                "combinedCheckCount": 0,
                "combinedDeathCount": 5,
                "eventFlagDeliveries": [{
                    "delivery_id": "81218a38-7e69-459a-84fe-9526936b0da1",
                    "event_flag": 1033640001
                }],
                "deathLinkDeliveries": [{
                    "deliveryId": "b01773fd-590f-4a99-8137-3df494aab540",
                    "speffectId": 102350
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(response.combined_death_count, Some(5));
        assert_eq!(response.player_great_rune_count, 1);
        assert_eq!(response.team_unique_great_rune_count, 2);
        assert_eq!(response.required_unique_great_runes, Some(2));
        assert!(response.great_rune_requirement_met);
        assert_eq!(response.event_flag_deliveries.len(), 1);
        assert_eq!(response.event_flag_deliveries[0].event_flag, 1_033_640_001);
        assert_eq!(response.death_link_deliveries.len(), 1);
        assert_eq!(
            response.death_link_deliveries[0].delivery_id,
            "b01773fd-590f-4a99-8137-3df494aab540"
        );
    }

    #[test]
    fn parses_flag_free_competition_config() {
        let source = r#"
[competition]
api_url = "https://example.supabase.co/functions/v1/absoloot-telemetry"
assignment_id = "assignment"
assignment_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
seed_id = "seed"
"#;
        let file: CompetitionFile = toml::from_str(source).unwrap();
        let settings = validate_settings(file.competition).unwrap();
        assert!(settings.shard_event_flags.is_empty());
        assert!(settings.great_rune_event_flags.is_empty());
    }

    #[test]
    fn applies_and_normalizes_private_manifest() {
        let source = r#"
[competition]
api_url = "https://example.supabase.co/functions/v1/absoloot-telemetry"
assignment_id = "assignment"
assignment_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
seed_id = "seed"
"#;
        let file: CompetitionFile = toml::from_str(source).unwrap();
        let settings = validate_settings(file.competition).unwrap();
        let resolved = apply_manifest(
            settings,
            ManifestResponse {
                version: 1,
                seed_id: "seed".to_string(),
                win_condition_flag: 20_010_800,
                required_unique_shards: Some(17),
                required_unique_great_runes: Some(2),
                collectible_display_mode: CollectibleDisplayMode::Both,
                death_link_enabled: true,
                death_link_rebound_seconds: DEFAULT_DEATH_LINK_REBOUND_SECONDS,
                event_flag_mirrors: vec![ManifestEventFlagMirror {
                    source_event_flag: 10_043_289,
                    target_event_flag: 342_432,
                }],
                shard_flags: vec![
                    ManifestShard { event_flag: 3 },
                    ManifestShard { event_flag: 1 },
                    ManifestShard { event_flag: 3 },
                ],
                great_rune_flags: vec![
                    ManifestShard { event_flag: 8 },
                    ManifestShard { event_flag: 7 },
                    ManifestShard { event_flag: 8 },
                ],
            },
        )
        .unwrap();

        assert_eq!(resolved.win_condition_flag, 20_010_800);
        assert_eq!(resolved.required_unique_shards, Some(17));
        assert_eq!(resolved.required_unique_great_runes, Some(2));
        assert_eq!(
            resolved.collectible_display_mode,
            CollectibleDisplayMode::Both
        );
        assert_eq!(resolved.shard_event_flags, [1, 3]);
        assert_eq!(resolved.great_rune_event_flags, [7, 8]);
        assert_eq!(resolved.event_flag_mirrors.len(), 1);
        assert_eq!(resolved.event_flag_mirrors[0].source_event_flag, 10_043_289);
        assert_eq!(resolved.event_flag_mirrors[0].target_event_flag, 342_432);
        assert!(resolved.death_link_enabled);
        assert_eq!(resolved.death_link_rebound_seconds, 30);
    }

    #[test]
    fn rejects_insecure_remote_urls_and_overlapping_flags() {
        let source = r#"
[competition]
api_url = "http://example.com/telemetry"
assignment_id = "assignment"
assignment_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
seed_id = "seed"
win_condition_flag = 10
shard_event_flags = [10]
great_rune_event_flags = [11]
"#;
        let file: CompetitionFile = toml::from_str(source).unwrap();
        assert!(validate_settings(file.competition).is_err());
    }

    #[test]
    fn rejects_flags_shared_by_shards_and_great_runes() {
        let source = r#"
[competition]
api_url = "https://example.supabase.co/functions/v1/absoloot-telemetry"
assignment_id = "assignment"
assignment_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
seed_id = "seed"
win_condition_flag = 10
shard_event_flags = [20]
great_rune_event_flags = [20]
"#;
        let file: CompetitionFile = toml::from_str(source).unwrap();
        assert!(validate_settings(file.competition).is_err());
    }
}

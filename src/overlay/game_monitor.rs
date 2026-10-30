use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use crossbeam::channel::Sender;

use crate::{
    debug_log,
    er::{events::EventFlagCache, gamedata::read_game_data, inventory::try_get_key_item_quantity},
    overlay::{
        competition::CompetitionSettings,
        config::{ConfigSnapshot, SharedConfig, VictoryCondition},
        data::SharedState,
        victory::VictoryTracker,
    },
};

const GREAT_RUNE_FLAGS: [i32; 7] = [181, 182, 183, 184, 185, 186, 187];
const ERROR_LOG_INTERVAL: Duration = Duration::from_secs(5);
const TELEMETRY_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn is_great_rune_flag(flag_id: i32) -> bool {
    GREAT_RUNE_FLAGS.contains(&flag_id)
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(error) => {
            lock.clear_poison();
            error.into_inner()
        }
    }
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(error) => {
            lock.clear_poison();
            error.into_inner()
        }
    }
}

fn initial_state_snapshot(state: &SharedState) -> (HashMap<i32, bool>, u32, u32, i32, bool) {
    let state = read_unpoisoned(state);
    (
        state.event_flags.clone(),
        state.key_item_quantity,
        state.death_count,
        state.great_runes,
        state.goal_complete,
    )
}

fn initial_published_igt(igt: &RwLock<u32>) -> u32 {
    *read_unpoisoned(igt)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ConfiguredVictory {
    revision: u64,
    condition: VictoryCondition,
}

impl From<&ConfigSnapshot> for ConfiguredVictory {
    fn from(snapshot: &ConfigSnapshot) -> Self {
        Self {
            revision: snapshot.revision,
            condition: snapshot
                .config
                .as_ref()
                .map(|config| config.victory.clone())
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SampleObservation {
    Current(bool),
    Stale(ConfiguredVictory),
}

fn configured_victory(config: &SharedConfig) -> ConfiguredVictory {
    let snapshot = read_unpoisoned(config);
    ConfiguredVictory::from(&*snapshot)
}

fn configured_victory_for_competition(
    config: &SharedConfig,
    competition: Option<&CompetitionSettings>,
) -> ConfiguredVictory {
    let mut configured = configured_victory(config);
    if competition.is_some() {
        // Competition completion is authoritative on the server because it may
        // require both the victory flag and a team-wide shard threshold.
        configured.condition = VictoryCondition::None;
    }
    configured
}

fn reconfigure_victory(
    configured: &mut ConfiguredVictory,
    latest: ConfiguredVictory,
    victory: &mut VictoryTracker,
) -> bool {
    if *configured == latest {
        return false;
    }
    let reconfigured = victory.reconfigure(latest.condition.clone());
    *configured = latest;
    reconfigured
}

fn observe_if_current(
    config: &SharedConfig,
    sampled_revision: u64,
    game_time: Option<u32>,
    victory: &mut VictoryTracker,
    values: &HashMap<i32, bool>,
    unresolved: &[i32],
) -> SampleObservation {
    let snapshot = read_unpoisoned(config);
    if snapshot.revision != sampled_revision {
        return SampleObservation::Stale(ConfiguredVictory::from(&*snapshot));
    }
    if game_time.is_none() {
        return SampleObservation::Current(false);
    }
    SampleObservation::Current(victory.observe(values, unresolved))
}

fn monitored_flag_ids(boss_flag_ids: &HashSet<i32>, victory: &VictoryTracker) -> HashSet<i32> {
    boss_flag_ids
        .iter()
        .copied()
        .chain(GREAT_RUNE_FLAGS)
        .chain(victory.requested_flag_ids())
        .collect()
}

fn create_competition_flag_cache(
    boss_flag_ids: &HashSet<i32>,
    victory: &VictoryTracker,
    competition: Option<&CompetitionSettings>,
) -> EventFlagCache {
    let competition_flags = competition.into_iter().flat_map(|settings| {
        settings
            .shard_event_flags
            .iter()
            .copied()
            .chain(settings.great_rune_event_flags.iter().copied())
            .chain(std::iter::once(settings.win_condition_flag))
            .chain(
                settings
                    .event_flag_mirrors
                    .iter()
                    .map(|mapping| mapping.source_event_flag),
            )
    });
    EventFlagCache::new(
        monitored_flag_ids(boss_flag_ids, victory)
            .into_iter()
            .chain(competition_flags),
    )
}

fn merge_boss_flags(
    published: &mut HashMap<i32, bool>,
    sampled: &HashMap<i32, bool>,
    boss_flag_ids: &HashSet<i32>,
) -> bool {
    let mut changed = false;
    for flag_id in boss_flag_ids {
        if let Some(value) = sampled.get(flag_id)
            && published.insert(*flag_id, *value) != Some(*value)
        {
            changed = true;
        }
    }
    changed
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorObservation {
    pub active_boss_flags: Vec<i32>,
    pub active_event_flags: Vec<i32>,
    pub active_mirror_flags: Vec<i32>,
    pub current_igt_ms: u32,
    pub death_count: u32,
    pub check_count: u32,
    pub shard_count: u32,
    pub observed_at: SystemTime,
}

#[cfg(test)]
fn build_monitor_observation(
    sampled: &HashMap<i32, bool>,
    boss_flag_ids: &HashSet<i32>,
    observed_at: SystemTime,
) -> MonitorObservation {
    let mut active_boss_flags = boss_flag_ids
        .iter()
        .filter(|flag_id| sampled.get(flag_id).copied().unwrap_or(false))
        .copied()
        .collect::<Vec<_>>();
    active_boss_flags.sort_unstable();
    let check_count = active_boss_flags.len() as u32;
    MonitorObservation {
        active_boss_flags,
        active_event_flags: Vec::new(),
        active_mirror_flags: Vec::new(),
        current_igt_ms: 0,
        death_count: 0,
        check_count,
        shard_count: 0,
        observed_at,
    }
}

fn build_competition_observation(
    sampled: &HashMap<i32, bool>,
    boss_flag_ids: &HashSet<i32>,
    settings: &CompetitionSettings,
    current_igt_ms: u32,
    death_count: u32,
    shard_count: u32,
    observed_at: SystemTime,
) -> MonitorObservation {
    let mut active_event_flags = settings
        .shard_event_flags
        .iter()
        .copied()
        .chain(settings.great_rune_event_flags.iter().copied())
        .chain(std::iter::once(settings.win_condition_flag))
        .filter(|flag_id| sampled.get(flag_id).copied().unwrap_or(false))
        .collect::<Vec<_>>();
    active_event_flags.sort_unstable();
    active_event_flags.dedup();
    let mut active_mirror_flags = settings
        .event_flag_mirrors
        .iter()
        .map(|mapping| mapping.source_event_flag)
        .filter(|flag_id| sampled.get(flag_id).copied().unwrap_or(false))
        .collect::<Vec<_>>();
    active_mirror_flags.sort_unstable();
    active_mirror_flags.dedup();
    let mut active_check_flags = boss_flag_ids
        .iter()
        .copied()
        .filter(|flag_id| sampled.get(flag_id).copied().unwrap_or(false))
        .collect::<Vec<_>>();
    active_check_flags.sort_unstable();
    active_check_flags.dedup();
    MonitorObservation {
        check_count: active_check_flags.len() as u32,
        active_boss_flags: active_check_flags,
        active_event_flags,
        active_mirror_flags,
        current_igt_ms,
        death_count,
        shard_count,
        observed_at,
    }
}

struct MonitorStateUpdate {
    event_flags: Option<HashMap<i32, bool>>,
    key_item_quantity: u32,
    death_count: u32,
    great_runes: i32,
    goal_complete: bool,
}

fn publish_state_after_igt(
    state: &SharedState,
    igt: &RwLock<u32>,
    game_time: Option<u32>,
    update: MonitorStateUpdate,
) {
    if let Some(game_time) = game_time {
        *write_unpoisoned(igt) = game_time;
    }
    let mut state = write_unpoisoned(state);
    if let Some(event_flags) = update.event_flags {
        state.event_flags = event_flags;
    }
    state.key_item_quantity = update.key_item_quantity;
    state.death_count = update.death_count;
    state.great_runes = update.great_runes;
    state.goal_complete = update.goal_complete;
    state.initialized = true;
}

pub fn start_game_monitor(
    state: SharedState,
    igt: Arc<RwLock<u32>>,
    flag_ids: Vec<i32>,
    config: SharedConfig,
    key_item_id: i32,
    poll_ms: u64,
    stop: Arc<AtomicBool>,
    observation_tx: Option<Sender<MonitorObservation>>,
    competition: Option<CompetitionSettings>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let update_interval = Duration::from_millis(poll_ms.max(10));
        let boss_flag_ids: HashSet<_> = flag_ids
            .into_iter()
            .filter(|flag_id| !is_great_rune_flag(*flag_id))
            .collect();
        let mut configured = configured_victory_for_competition(&config, competition.as_ref());
        let mut victory =
            VictoryTracker::new(configured.condition.clone(), boss_flag_ids.iter().copied());
        let mut flag_cache =
            create_competition_flag_cache(&boss_flag_ids, &victory, competition.as_ref());
        let (
            mut published_flags,
            mut published_quantity,
            mut published_deaths,
            mut published_runes,
            mut published_goal_complete,
        ) = initial_state_snapshot(&state);
        let previous_flag_count = published_flags.len();
        published_flags.retain(|flag_id, _| boss_flag_ids.contains(flag_id));
        let mut boss_flags_need_publish = published_flags.len() != previous_flag_count;
        let mut sampled_rune_flags = HashMap::with_capacity(GREAT_RUNE_FLAGS.len());
        let mut published_igt = initial_published_igt(&igt);
        let mut next_flag_error_log = Instant::now();
        let mut last_unresolved_count = None;
        let mut next_telemetry = Instant::now();
        if let Ok(mut current_state) = state.write() {
            current_state.total_check_count = boss_flag_ids.len() as u32;
        }

        while !stop.load(Ordering::Acquire) {
            let cycle_started = Instant::now();
            let latest = configured_victory_for_competition(&config, competition.as_ref());
            if reconfigure_victory(&mut configured, latest, &mut victory) {
                flag_cache =
                    create_competition_flag_cache(&boss_flag_ids, &victory, competition.as_ref());
                last_unresolved_count = None;
                debug_log!(
                    "[absoloot_overlay] Victory condition changed to {:?}",
                    victory.condition()
                );
            }

            let mut flags_changed = boss_flags_need_publish;
            let mut completed_this_cycle = false;
            let game_data = read_game_data();
            let key_item_quantity = try_get_key_item_quantity(key_item_id);
            let player_is_loaded = game_data.is_some() && key_item_quantity.is_some();
            let player_game_time = if player_is_loaded {
                game_data.map(|snapshot| snapshot.play_time_ms)
            } else {
                None
            };
            match flag_cache.sample(cycle_started) {
                Ok(sample) => {
                    match observe_if_current(
                        &config,
                        configured.revision,
                        player_game_time,
                        &mut victory,
                        &sample.values,
                        &sample.unresolved,
                    ) {
                        SampleObservation::Current(completed) => {
                            completed_this_cycle = completed;
                            flags_changed |= merge_boss_flags(
                                &mut published_flags,
                                &sample.values,
                                &boss_flag_ids,
                            );
                            if let (Some(settings), Some(sender), Some(game_data), Some(quantity)) = (
                                competition.as_ref(),
                                observation_tx.as_ref(),
                                game_data,
                                key_item_quantity,
                            ) && (cycle_started >= next_telemetry || completed_this_cycle)
                            {
                                let telemetry_igt =
                                    if victory.is_complete() && !completed_this_cycle {
                                        published_igt
                                    } else {
                                        game_data.play_time_ms
                                    };
                                let observation = build_competition_observation(
                                    &sample.values,
                                    &boss_flag_ids,
                                    settings,
                                    telemetry_igt,
                                    game_data.death_count,
                                    quantity,
                                    SystemTime::now(),
                                );
                                let _ = sender.try_send(observation);
                                next_telemetry = cycle_started + TELEMETRY_INTERVAL;
                            }
                            for (flag_id, value) in sample.values {
                                if is_great_rune_flag(flag_id) {
                                    sampled_rune_flags.insert(flag_id, value);
                                }
                            }
                            let unresolved_count = sample.unresolved.len();
                            if last_unresolved_count != Some(unresolved_count) {
                                if unresolved_count != 0 {
                                    debug_log!(
                                        "[event_flags] {unresolved_count} requested flags are not currently resolvable; retrying"
                                    );
                                } else if last_unresolved_count.is_some_and(|count| count != 0) {
                                    debug_log!("[event_flags] All requested flags resolved");
                                }
                                last_unresolved_count = Some(unresolved_count);
                            }
                        }
                        SampleObservation::Stale(latest) => {
                            if reconfigure_victory(&mut configured, latest, &mut victory) {
                                flag_cache = create_competition_flag_cache(
                                    &boss_flag_ids,
                                    &victory,
                                    competition.as_ref(),
                                );
                                last_unresolved_count = None;
                                debug_log!(
                                    "[absoloot_overlay] Victory condition changed to {:?}",
                                    victory.condition()
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    if cycle_started >= next_flag_error_log {
                        debug_log!("[event_flags] {error}; retrying");
                        next_flag_error_log = cycle_started + ERROR_LOG_INTERVAL;
                    }
                }
            }

            let runes = GREAT_RUNE_FLAGS
                .iter()
                .filter(|flag| sampled_rune_flags.get(flag).copied().unwrap_or(false))
                .count() as i32;
            let quantity = key_item_quantity.unwrap_or(published_quantity);
            let deaths = if player_is_loaded {
                game_data
                    .map(|snapshot| snapshot.death_count)
                    .unwrap_or(published_deaths)
            } else {
                published_deaths
            };
            let state_changed = flags_changed
                || quantity != published_quantity
                || deaths != published_deaths
                || runes != published_runes
                || victory.is_complete() != published_goal_complete;
            let game_time_to_publish = if !victory.is_complete() || completed_this_cycle {
                game_data
                    .filter(|_| player_is_loaded)
                    .map(|snapshot| snapshot.play_time_ms)
                    .filter(|game_time| *game_time != published_igt)
            } else {
                None
            };
            if state_changed {
                publish_state_after_igt(
                    &state,
                    &igt,
                    game_time_to_publish,
                    MonitorStateUpdate {
                        event_flags: flags_changed.then(|| published_flags.clone()),
                        key_item_quantity: quantity,
                        death_count: deaths,
                        great_runes: runes,
                        goal_complete: victory.is_complete(),
                    },
                );
                if flags_changed {
                    boss_flags_need_publish = false;
                }
                published_quantity = quantity;
                published_deaths = deaths;
                published_runes = runes;
                published_goal_complete = victory.is_complete();
            } else if let Some(game_time) = game_time_to_publish {
                let mut in_game_time = write_unpoisoned(&igt);
                *in_game_time = game_time;
            }
            if let Some(game_time) = game_time_to_publish {
                published_igt = game_time;
            }
            if completed_this_cycle {
                debug_log!(
                    "[absoloot_overlay] Victory condition {:?} completed at {published_igt} ms",
                    victory.condition()
                );
            }

            if let Some(remaining) = update_interval.checked_sub(cycle_started.elapsed()) {
                thread::sleep(remaining);
            }
        }
        debug_log!("[absoloot_overlay] Monitor thread exiting gracefully");
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        sync::{Arc, RwLock},
        thread,
        time::{Duration, Instant, SystemTime},
    };

    use crate::overlay::{
        config::{ConfigSnapshot, RuntimeConfig, SharedConfig, VictoryCondition},
        data::create_state,
        style::IgniteConfig,
        victory::VictoryTracker,
    };

    use super::{
        ConfiguredVictory, GREAT_RUNE_FLAGS, MonitorStateUpdate, SampleObservation,
        build_monitor_observation, configured_victory, initial_published_igt,
        initial_state_snapshot, merge_boss_flags, monitored_flag_ids, observe_if_current,
        publish_state_after_igt, reconfigure_victory, write_unpoisoned,
    };

    fn runtime_with_victory(victory: VictoryCondition) -> Arc<RuntimeConfig> {
        let mut runtime = RuntimeConfig::try_from(IgniteConfig {
            common: None,
            input: None,
            style: None,
            boss: None,
            overlay: None,
            timer: None,
            victory: None,
        })
        .unwrap();
        runtime.victory = victory;
        Arc::new(runtime)
    }

    #[test]
    fn monitor_observation_contains_only_sorted_active_boss_flags() {
        let sampled = HashMap::from([(30, true), (10, false), (20, true), (181, true)]);
        let boss_ids = HashSet::from([10, 20, 30]);
        let observed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(7);

        let observation = build_monitor_observation(&sampled, &boss_ids, observed_at);

        assert_eq!(observation.active_boss_flags, [20, 30]);
        assert_eq!(observation.observed_at, observed_at);
    }

    fn shared_config(revision: u64, victory: VictoryCondition) -> SharedConfig {
        Arc::new(RwLock::new(ConfigSnapshot {
            revision,
            config: Some(runtime_with_victory(victory)),
        }))
    }

    #[test]
    fn monitor_targets_include_victory_ids_without_making_them_bosses() {
        let boss_ids = HashSet::from([1, 2]);
        let tracker = VictoryTracker::new(VictoryCondition::BossIds(vec![20, 30]), [1, 2]);
        let monitored = monitored_flag_ids(&boss_ids, &tracker);

        assert!(monitored.is_superset(&HashSet::from([1, 2, 20, 30])));
        assert!(GREAT_RUNE_FLAGS.iter().all(|id| monitored.contains(id)));

        let mut published = HashMap::new();
        assert!(merge_boss_flags(
            &mut published,
            &HashMap::from([(1, true), (20, true)]),
            &boss_ids,
        ));
        assert_eq!(published, HashMap::from([(1, true)]));
    }

    #[test]
    fn poisoned_shared_writes_recover_the_inner_state() {
        let shared = Arc::new(RwLock::new(0));
        let poisoned = Arc::clone(&shared);
        assert!(
            thread::spawn(move || {
                let _guard = poisoned.write().unwrap();
                panic!("poison test lock");
            })
            .join()
            .is_err()
        );

        *write_unpoisoned(&shared) = 1;

        assert_eq!(*shared.read().unwrap(), 1);
    }

    #[test]
    fn runtime_config_poison_recovers_the_valid_snapshot_and_allows_reload() {
        let config = shared_config(7, VictoryCondition::OneBoss(10));
        let mut configured = configured_victory(&config);
        let mut victory = VictoryTracker::new(configured.condition.clone(), []);
        let poisoned = Arc::clone(&config);
        assert!(
            thread::spawn(move || {
                let _guard = poisoned.write().unwrap();
                panic!("poison test config");
            })
            .join()
            .is_err()
        );

        let recovered = configured_victory(&config);
        assert_eq!(recovered.revision, 7);
        assert_eq!(recovered.condition, VictoryCondition::OneBoss(10));
        assert!(config.write().is_ok());

        *config.write().unwrap() = ConfigSnapshot {
            revision: 8,
            config: Some(runtime_with_victory(VictoryCondition::OneBoss(20))),
        };
        let latest = configured_victory(&config);
        assert!(reconfigure_victory(&mut configured, latest, &mut victory,));
        assert_eq!(victory.condition(), &VictoryCondition::OneBoss(20));
    }

    #[test]
    fn monitor_initialization_recovers_a_poisoned_valid_victory_snapshot() {
        let config = shared_config(7, VictoryCondition::OneBoss(10));
        let poisoned = Arc::clone(&config);
        assert!(
            thread::spawn(move || {
                let _guard = poisoned.write().unwrap();
                panic!("poison test initial config");
            })
            .join()
            .is_err()
        );

        let configured = configured_victory(&config);

        assert_eq!(configured.revision, 7);
        assert_eq!(configured.condition, VictoryCondition::OneBoss(10));
        assert!(config.read().is_ok());
    }

    #[test]
    fn hot_reload_between_sampling_and_observation_discards_the_stale_sample() {
        let config = shared_config(0, VictoryCondition::OneBoss(10));
        let mut configured = configured_victory(&config);
        let mut victory = VictoryTracker::new(configured.condition.clone(), []);

        *config.write().unwrap() = ConfigSnapshot {
            revision: 1,
            config: Some(runtime_with_victory(VictoryCondition::OneBoss(20))),
        };

        let latest = match observe_if_current(
            &config,
            configured.revision,
            None,
            &mut victory,
            &HashMap::from([(10, true)]),
            &[],
        ) {
            SampleObservation::Stale(latest) => latest,
            other => panic!("expected a stale sample, got {other:?}"),
        };
        assert!(!victory.is_complete());
        assert!(reconfigure_victory(&mut configured, latest, &mut victory,));
        assert_eq!(victory.requested_flag_ids().collect::<Vec<_>>(), [20]);
    }

    #[test]
    fn same_condition_revision_updates_without_rebuilding_victory() {
        let mut configured = ConfiguredVictory {
            revision: 1,
            condition: VictoryCondition::OneBoss(10),
        };
        let mut victory = VictoryTracker::new(configured.condition.clone(), []);

        assert!(!reconfigure_victory(
            &mut configured,
            ConfiguredVictory {
                revision: 2,
                condition: VictoryCondition::OneBoss(10),
            },
            &mut victory,
        ));
        assert_eq!(configured.revision, 2);
        assert_eq!(victory.condition(), &VictoryCondition::OneBoss(10));
    }

    #[test]
    fn post_completion_revision_updates_without_rebuilding_victory() {
        let mut configured = ConfiguredVictory {
            revision: 1,
            condition: VictoryCondition::OneBoss(10),
        };
        let mut victory = VictoryTracker::new(configured.condition.clone(), []);
        assert!(victory.observe(&HashMap::from([(10, true)]), &[]));

        assert!(!reconfigure_victory(
            &mut configured,
            ConfiguredVictory {
                revision: 2,
                condition: VictoryCondition::OneBoss(20),
            },
            &mut victory,
        ));
        assert_eq!(configured.revision, 2);
        assert_eq!(configured.condition, VictoryCondition::OneBoss(20));
        assert_eq!(victory.condition(), &VictoryCondition::OneBoss(10));
        assert!(victory.is_complete());
    }

    #[test]
    fn victory_waits_for_current_game_time_before_completing() {
        let config = shared_config(0, VictoryCondition::OneBoss(10));
        let configured = configured_victory(&config);
        let mut victory = VictoryTracker::new(configured.condition, []);
        let active_target = HashMap::from([(10, true)]);

        assert_eq!(
            observe_if_current(
                &config,
                configured.revision,
                None,
                &mut victory,
                &active_target,
                &[],
            ),
            SampleObservation::Current(false)
        );
        assert!(!victory.is_complete());

        assert_eq!(
            observe_if_current(
                &config,
                configured.revision,
                Some(456),
                &mut victory,
                &active_target,
                &[],
            ),
            SampleObservation::Current(true)
        );
        assert!(victory.is_complete());
    }

    #[test]
    fn completion_cycle_publishes_current_igt_before_goal_state() {
        let state = create_state();
        let igt = Arc::new(RwLock::new(100));
        let state_guard = state.write().unwrap();
        let worker_state = Arc::clone(&state);
        let worker_igt = Arc::clone(&igt);
        let worker = thread::spawn(move || {
            publish_state_after_igt(
                &worker_state,
                &worker_igt,
                Some(456),
                MonitorStateUpdate {
                    event_flags: None,
                    key_item_quantity: 0,
                    death_count: 0,
                    great_runes: 0,
                    goal_complete: true,
                },
            );
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while *igt.read().unwrap() != 456 {
            assert!(
                Instant::now() < deadline,
                "IGT was not published while goal-state publication was blocked"
            );
            thread::yield_now();
        }
        assert!(!state_guard.goal_complete);
        drop(state_guard);

        worker.join().unwrap();
        assert!(state.read().unwrap().goal_complete);
        assert_eq!(*igt.read().unwrap(), 456);
    }

    #[test]
    fn monitor_initialization_recovers_default_valued_poisoned_snapshots() {
        let state = create_state();
        let poisoned_state = Arc::clone(&state);
        assert!(
            thread::spawn(move || {
                let _guard = poisoned_state.write().unwrap();
                panic!("poison test state");
            })
            .join()
            .is_err()
        );
        let igt = Arc::new(RwLock::new(0));
        let poisoned_igt = Arc::clone(&igt);
        assert!(
            thread::spawn(move || {
                let _guard = poisoned_igt.write().unwrap();
                panic!("poison test IGT");
            })
            .join()
            .is_err()
        );

        let (flags, quantity, deaths, runes, goal_complete) = initial_state_snapshot(&state);
        let published_igt = initial_published_igt(&igt);

        assert!(flags.is_empty());
        assert_eq!((quantity, deaths, runes, goal_complete), (0, 0, 0, false));
        assert_eq!(published_igt, 0);
        assert!(state.read().is_ok());
        assert!(igt.read().is_ok());
    }
}

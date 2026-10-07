use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, WorldChrMan},
    fd4::FD4TaskData,
};
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};

use crate::{debug_log, overlay::data::SharedState};

const WRITER_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(5);

pub fn start_death_link_writer(
    state: SharedState,
    stop: Arc<AtomicBool>,
    poll_ms: u64,
    rebound_seconds: u64,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("absoloot-death-link-writer".to_string())
        .spawn(move || {
            let task_manager = match CSTaskImp::wait_for_instance(Duration::MAX) {
                Ok(task_manager) => task_manager,
                Err(error) => {
                    debug_log!("[death_link] Could not attach to Elden Ring task manager: {error}");
                    return;
                }
            };
            let poll_interval = Duration::from_millis(poll_ms.max(10));
            let rebound_buffer = Duration::from_secs(rebound_seconds.max(1));
            let mut last_poll = Instant::now()
                .checked_sub(poll_interval)
                .unwrap_or_else(Instant::now);
            let mut next_application = Instant::now();
            let mut next_error_log = Instant::now();
            let mut disabled = false;

            task_manager.run_recurring(
                move |_: &FD4TaskData| {
                    if disabled
                        || stop.load(Ordering::Acquire)
                        || last_poll.elapsed() < poll_interval
                        || Instant::now() < next_application
                    {
                        return;
                    }
                    last_poll = Instant::now();

                    let delivery = state.read().ok().and_then(|current_state| {
                        if current_state.player_finish_igt_ms.is_some() {
                            None
                        } else {
                            current_state.pending_death_links.first().cloned()
                        }
                    });
                    let Some(delivery) = delivery else {
                        return;
                    };

                    let result = catch_unwind(AssertUnwindSafe(|| {
                        // SAFETY: Elden Ring invokes this callback through its task system,
                        // providing the main-thread exclusivity required by instance_mut.
                        let Ok(world_chr_man) = (unsafe { WorldChrMan::instance_mut() }) else {
                            return false;
                        };
                        let Some(ref mut main_player) = world_chr_man.main_player else {
                            return false;
                        };
                        let character = &main_player.chr_ins;
                        let is_ready = character.chr_flags1c8.is_active()
                            && !character.chr_flags1c5.death_flag()
                            && !character.chr_flags1c5.is_invincible()
                            && character.modules.data.hp > 0;
                        if !is_ready {
                            return false;
                        }

                        // Update the live character data on Elden Ring's task thread so
                        // the game can run its normal death processing without requiring
                        // a seed-specific SpEffect.
                        main_player.chr_ins.modules.data.hp = 0;
                        true
                    }));

                    match result {
                        Ok(true) => {
                            next_application = Instant::now() + rebound_buffer;
                            if let Ok(mut current_state) = state.write()
                                && current_state
                                    .pending_death_links
                                    .iter()
                                    .any(|pending| pending.delivery_id == delivery.delivery_id)
                            {
                                current_state.pending_death_links.retain(|pending| {
                                    pending.delivery_id != delivery.delivery_id
                                });
                                if !current_state
                                    .applied_death_link_delivery_ids
                                    .contains(&delivery.delivery_id)
                                {
                                    current_state
                                        .applied_death_link_delivery_ids
                                        .push(delivery.delivery_id.clone());
                                }
                            }
                            debug_log!(
                                "[death_link] Set player HP to zero for delivery {}",
                                delivery.delivery_id
                            );
                        }
                        Ok(false) => {
                            // Keep the delivery queued while the player is absent, loading,
                            // warping, dead, or otherwise not ready for the linked death.
                            if Instant::now() >= next_error_log {
                                debug_log!(
                                    "[death_link] Player is not ready; keeping the teammate death queued"
                                );
                                next_error_log =
                                    Instant::now() + WRITER_ERROR_LOG_INTERVAL;
                            }
                        }
                        Err(_) => {
                            disabled = true;
                            debug_log!(
                                "[death_link] Death-link writer panicked and has been disabled for this process"
                            );
                        }
                    }

                },
                CSTaskGroupIndex::FrameBegin,
            );
        })
        .expect("death-link writer thread could not be started")
}

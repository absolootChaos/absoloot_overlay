use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crossbeam::channel::Sender;

use crate::{
    er::{death_link::start_death_link_writer, events::start_event_flag_writer},
    overlay::{
        competition::CompetitionSettings,
        config::SharedConfig,
        data::SharedState,
        game_monitor::{MonitorObservation, start_game_monitor},
    },
};

pub struct OverlayRuntime {
    monitor_stop: Arc<AtomicBool>,
    monitor_thread: Option<thread::JoinHandle<()>>,
    event_writer_thread: Option<thread::JoinHandle<()>>,
    death_link_writer_thread: Option<thread::JoinHandle<()>>,
}

impl OverlayRuntime {
    pub fn new() -> Self {
        Self {
            monitor_stop: Arc::new(AtomicBool::new(false)),
            monitor_thread: None,
            event_writer_thread: None,
            death_link_writer_thread: None,
        }
    }

    pub fn start(
        &mut self,
        state: SharedState,
        in_game_time: Arc<RwLock<u32>>,
        boss_flags: Vec<i32>,
        config: SharedConfig,
        key_item_id: i32,
        poll_ms: u64,
        observation_tx: Option<Sender<MonitorObservation>>,
        competition: Option<CompetitionSettings>,
    ) {
        self.stop();
        self.monitor_stop.store(false, Ordering::Release);
        let death_link_rebound_seconds = competition
            .as_ref()
            .map(|settings| settings.death_link_rebound_seconds)
            .unwrap_or(30);
        self.monitor_thread = Some(start_game_monitor(
            state.clone(),
            in_game_time,
            boss_flags,
            config,
            key_item_id,
            poll_ms,
            self.monitor_stop.clone(),
            observation_tx,
            competition,
        ));
        self.event_writer_thread = Some(start_event_flag_writer(
            state.clone(),
            self.monitor_stop.clone(),
            poll_ms,
        ));
        self.death_link_writer_thread = Some(start_death_link_writer(
            state,
            self.monitor_stop.clone(),
            poll_ms,
            death_link_rebound_seconds,
        ));
    }

    pub fn stop(&mut self) {
        self.monitor_stop.store(true, Ordering::Release);
        if let Some(thread) = self.monitor_thread.take() {
            let _ = thread.join();
        }
        // The writer registers work with Elden Ring's task system. Signal the
        // callback to stop, then detach the registration thread so shutdown
        // cannot block on game-owned task lifetime.
        self.event_writer_thread.take();
        self.death_link_writer_thread.take();
    }
}

impl Default for OverlayRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for OverlayRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

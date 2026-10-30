use std::collections::HashMap;

use crate::{
    overlay::{
        competition::CompetitionSettings,
        config::TimerSettings,
        data::AppState,
        game_monitor::is_great_rune_flag,
        style::{DEFAULT_DISPLAY_TEXT, TimerMode},
    },
    util::text_formatter::format_display_text,
};

const GOAL_COMPLETE_TITLE: &str = "GOAL COMPLETE";

pub struct OverlayViewModel {
    pub title: Option<&'static str>,
    pub lines: Vec<String>,
    pub competition: Option<CompetitionPanel>,
}

pub struct CompetitionPanel {
    pub seed_name: String,
    pub current_igt: String,
    pub combined_igt: String,
    pub checks: String,
    pub combined_checks: String,
    pub shards: String,
    pub combined_unique_shards: String,
    pub shows_shards: bool,
    pub great_runes: String,
    pub combined_unique_great_runes: String,
    pub shows_great_runes: bool,
    pub deaths: String,
    pub combined_deaths: String,
    pub shard_goal_notice: Option<String>,
    pub great_rune_goal_notice: Option<String>,
    pub victory_flag_reached: bool,
}

impl CompetitionPanel {
    pub fn line_count(&self) -> usize {
        6 + usize::from(self.shows_shards)
            + usize::from(self.shows_great_runes)
            + usize::from(self.shard_goal_notice.is_some())
            + usize::from(self.great_rune_goal_notice.is_some())
    }
}

impl OverlayViewModel {
    pub fn build(
        state: Option<&AppState>,
        in_game_time_ms: u32,
        timer: TimerSettings,
        template: Option<&str>,
    ) -> Self {
        let (kills, total, deaths, shards, runes) = state
            .map(|state| {
                (
                    state
                        .event_flags
                        .iter()
                        .filter(|(flag_id, defeated)| !is_great_rune_flag(**flag_id) && **defeated)
                        .count(),
                    state
                        .event_flags
                        .keys()
                        .filter(|flag_id| !is_great_rune_flag(**flag_id))
                        .count(),
                    state.death_count,
                    state.key_item_quantity,
                    state.great_runes,
                )
            })
            .unwrap_or((0, 0, 0, 0, 0));
        let variables = HashMap::from([
            ("kills", kills.to_string()),
            ("total", total.to_string()),
            ("deaths", deaths.to_string()),
            ("igt", format_timer(in_game_time_ms, timer)),
            ("shards", shards.to_string()),
            ("runes", runes.to_string()),
            (
                "player_shards",
                state
                    .map(|state| state.player_shard_count)
                    .unwrap_or(0)
                    .to_string(),
            ),
            (
                "team_shards",
                state
                    .map(|state| state.team_unique_shard_count)
                    .unwrap_or(0)
                    .to_string(),
            ),
            (
                "player_great_runes",
                state
                    .map(|state| state.player_great_rune_count)
                    .unwrap_or(0)
                    .to_string(),
            ),
            (
                "team_great_runes",
                state
                    .map(|state| state.team_unique_great_rune_count)
                    .unwrap_or(0)
                    .to_string(),
            ),
            (
                "teammate_igt",
                optional_duration(state.and_then(|state| {
                    state
                        .teammate_finish_igt_ms
                        .or(state.teammate_current_igt_ms)
                })),
            ),
            (
                "combined_igt",
                optional_duration(state.and_then(|state| state.combined_finish_igt_ms)),
            ),
        ]);
        let title = state
            .is_some_and(|state| state.goal_complete)
            .then_some(GOAL_COMPLETE_TITLE);
        Self {
            title,
            lines: format_display_text(template.unwrap_or(DEFAULT_DISPLAY_TEXT), &variables),
            competition: None,
        }
    }

    pub fn build_with_competition(
        state: Option<&AppState>,
        in_game_time_ms: u32,
        timer: TimerSettings,
        template: Option<&str>,
        settings: &CompetitionSettings,
    ) -> Self {
        let mut model = Self::build(state, in_game_time_ms, timer, template);
        let current_igt = state
            .and_then(|state| state.player_finish_igt_ms)
            .and_then(|value| i64::try_from(value).ok())
            .map(format_duration)
            .unwrap_or_else(|| format_timer(in_game_time_ms, timer));
        let shard_goal_notice = state.and_then(|state| {
            state
                .required_unique_shards
                .filter(|_| state.shard_requirement_met)
                .map(|required| {
                    format!(
                        "TEAM SHARD GOAL REACHED  {}/{}",
                        state.team_unique_shard_count, required
                    )
                })
        });
        let great_rune_goal_notice = state.and_then(|state| {
            state
                .required_unique_great_runes
                .filter(|_| state.great_rune_requirement_met)
                .map(|required| {
                    format!(
                        "TEAM GREAT RUNE GOAL REACHED  {}/{}",
                        state.team_unique_great_rune_count, required
                    )
                })
        });
        model.competition = Some(CompetitionPanel {
            seed_name: settings.seed_name.clone(),
            current_igt,
            combined_igt: optional_duration(state.and_then(|state| state.combined_current_igt_ms)),
            checks: state
                .map(|state| state.player_check_count)
                .unwrap_or(0)
                .to_string(),
            combined_checks: state
                .map(|state| {
                    format!(
                        "{}/{}",
                        state.team_unique_check_count, state.total_check_count
                    )
                })
                .unwrap_or_else(|| "--".to_string()),
            shards: state
                .map(|state| state.player_shard_count)
                .unwrap_or(0)
                .to_string(),
            combined_unique_shards: state
                .map(|state| state.team_unique_shard_count)
                .unwrap_or(0)
                .to_string(),
            shows_shards: settings.collectible_display_mode.shows_shards(),
            great_runes: state
                .map(|state| state.player_great_rune_count)
                .unwrap_or(0)
                .to_string(),
            combined_unique_great_runes: state
                .map(|state| state.team_unique_great_rune_count)
                .unwrap_or(0)
                .to_string(),
            shows_great_runes: settings.collectible_display_mode.shows_great_runes(),
            deaths: state
                .map(|state| state.death_count)
                .unwrap_or(0)
                .to_string(),
            combined_deaths: optional_count(state.and_then(|state| state.combined_death_count)),
            shard_goal_notice,
            great_rune_goal_notice,
            victory_flag_reached: state.is_some_and(|state| state.victory_flag_reached),
        });
        model
    }
}

fn optional_count(value: Option<u32>) -> String {
    value.map_or_else(|| "--".to_string(), |value| value.to_string())
}

fn optional_duration(milliseconds: Option<u64>) -> String {
    milliseconds
        .and_then(|value| i64::try_from(value).ok())
        .map(format_duration)
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn format_timer(in_game_time_ms: u32, timer: TimerSettings) -> String {
    let raw_ms = i64::from(in_game_time_ms);
    let prep_ms = i64::from(timer.prep_minutes) * 60_000;
    let timer_target_ms = i64::from(timer.timer_minutes) * 60_000;
    let display_ms = match timer.mode {
        TimerMode::Regular => raw_ms,
        TimerMode::Timer => timer_target_ms - raw_ms,
        TimerMode::Prep => raw_ms - prep_ms,
        TimerMode::PrepTimer if raw_ms < prep_ms => raw_ms - prep_ms,
        TimerMode::PrepTimer => timer_target_ms - (raw_ms - prep_ms),
    };
    format_duration(display_ms)
}

fn format_duration(milliseconds: i64) -> String {
    let total_seconds = milliseconds / 1_000;
    let sign = if total_seconds < 0 { "-" } else { "" };
    let total_seconds = total_seconds.unsigned_abs();
    if total_seconds > 86_400 {
        let (days, remainder) = (total_seconds / 86_400, total_seconds % 86_400);
        let (hours, remainder) = (remainder / 3_600, remainder % 3_600);
        format!(
            "{sign}{days:02}:{hours:02}:{:02}:{:02}",
            remainder / 60,
            remainder % 60
        )
    } else {
        let (hours, remainder) = (total_seconds / 3_600, total_seconds % 3_600);
        format!(
            "{sign}{hours:02}:{:02}:{:02}",
            remainder / 60,
            remainder % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::overlay::{
        competition::{CollectibleDisplayMode, CompetitionSettings},
        config::TimerSettings,
        data::AppState,
        style::TimerMode,
    };

    use super::{OverlayViewModel, format_duration, format_timer};

    #[test]
    fn formats_short_multi_day_and_negative_times() {
        assert_eq!(format_duration(3_661_000), "01:01:01");
        assert_eq!(format_duration(90_061_000), "01:01:01:01");
        assert_eq!(format_duration(-61_000), "-00:01:01");
    }

    #[test]
    fn preserves_all_legacy_timer_modes() {
        let timer = |mode, prep_minutes, timer_minutes| TimerSettings {
            mode,
            prep_minutes,
            timer_minutes,
        };
        assert_eq!(
            format_timer(60_000, timer(TimerMode::Regular, 0, 0)),
            "00:01:00"
        );
        assert_eq!(
            format_timer(60_000, timer(TimerMode::Timer, 0, 10)),
            "00:09:00"
        );
        assert_eq!(
            format_timer(60_000, timer(TimerMode::Prep, 2, 0)),
            "-00:01:00"
        );
        assert_eq!(
            format_timer(60_000, timer(TimerMode::PrepTimer, 2, 10)),
            "-00:01:00"
        );
        assert_eq!(
            format_timer(180_000, timer(TimerMode::PrepTimer, 2, 10)),
            "00:09:00"
        );
    }

    #[test]
    fn builds_template_once_from_snapshot() {
        let model = OverlayViewModel::build(
            None,
            0,
            TimerSettings::default(),
            Some("Bosses: {kills}/{total}$n{igt}"),
        );
        assert_eq!(model.lines, [" Bosses: 0/0 ", " 00:00:00 "]);
    }

    #[test]
    fn excludes_great_rune_flags_from_boss_totals() {
        let mut state = AppState::default();
        state.event_flags.extend([(1000, true), (1001, false)]);
        state
            .event_flags
            .extend((181..=187).map(|flag_id| (flag_id, true)));
        state.great_runes = 7;

        let model = OverlayViewModel::build(
            Some(&state),
            0,
            TimerSettings::default(),
            Some("{kills}/{total} runes={runes}"),
        );

        assert_eq!(model.lines, [" 1/2 runes=7 "]);
    }

    #[test]
    fn exposes_goal_title_only_after_completion() {
        let incomplete = OverlayViewModel::build(
            Some(&AppState::default()),
            0,
            TimerSettings::default(),
            Some("IGT: {igt}"),
        );
        assert_eq!(incomplete.title, None);

        let complete = AppState {
            goal_complete: true,
            ..Default::default()
        };
        let complete = OverlayViewModel::build(
            Some(&complete),
            0,
            TimerSettings::default(),
            Some("IGT: {igt}"),
        );
        assert_eq!(complete.title, Some("GOAL COMPLETE"));
        assert_eq!(complete.lines, [" IGT: 00:00:00 "]);
    }

    #[test]
    fn exposes_server_confirmed_team_values() {
        let state = AppState {
            player_shard_count: 2,
            team_unique_shard_count: 3,
            teammate_current_igt_ms: Some(3_661_000),
            combined_finish_igt_ms: Some(7_322_000),
            ..Default::default()
        };
        let model = OverlayViewModel::build(
            Some(&state),
            0,
            TimerSettings::default(),
            Some(
                "Player {player_shards} Team {team_shards}$nMate {teammate_igt} Total {combined_igt}",
            ),
        );

        assert_eq!(
            model.lines,
            [" Player 2 Team 3 ", " Mate 01:01:01 Total 02:02:02 ",]
        );
    }

    #[test]
    fn builds_competition_scorecard_with_player_and_team_metrics() {
        let mut state = AppState {
            key_item_quantity: 2,
            death_count: 3,
            player_check_count: 2,
            team_unique_check_count: 5,
            total_check_count: 34,
            team_unique_shard_count: 4,
            player_shard_count: 2,
            player_great_rune_count: 1,
            team_unique_great_rune_count: 2,
            required_unique_shards: Some(4),
            shard_requirement_met: true,
            required_unique_great_runes: Some(2),
            great_rune_requirement_met: true,
            combined_current_igt_ms: Some(7_322_000),
            combined_check_count: Some(5),
            combined_death_count: Some(6),
            ..Default::default()
        };
        state.event_flags.extend([(1000, true), (1001, true)]);
        let settings = CompetitionSettings {
            seed_name: "Testing Seed".to_string(),
            api_url: "https://example.supabase.co/functions/v1/telemetry".to_string(),
            assignment_id: "assignment".to_string(),
            assignment_token: "a".repeat(64),
            seed_id: "seed".to_string(),
            win_condition_flag: 20_010_800,
            shard_event_flags: vec![2_046_410_800],
            great_rune_event_flags: vec![2_051_440_800],
            event_flag_mirrors: vec![],
            required_unique_shards: Some(4),
            required_unique_great_runes: Some(2),
            collectible_display_mode: CollectibleDisplayMode::Both,
            death_link_enabled: false,
            death_link_rebound_seconds: 30,
        };

        let model = OverlayViewModel::build_with_competition(
            Some(&state),
            3_661_000,
            TimerSettings::default(),
            None,
            &settings,
        );
        let panel = model.competition.expect("competition scorecard");
        assert_eq!(panel.seed_name, "Testing Seed");
        assert_eq!(panel.current_igt, "01:01:01");
        assert_eq!(panel.combined_igt, "02:02:02");
        assert_eq!(panel.checks, "2");
        assert_eq!(panel.combined_checks, "5/34");
        assert_eq!(panel.shards, "2");
        assert_eq!(panel.combined_unique_shards, "4");
        assert!(panel.shows_shards);
        assert_eq!(panel.great_runes, "1");
        assert_eq!(panel.combined_unique_great_runes, "2");
        assert!(panel.shows_great_runes);
        assert_eq!(panel.line_count(), 10);
        assert_eq!(panel.deaths, "3");
        assert_eq!(panel.combined_deaths, "6");
        assert_eq!(
            panel.shard_goal_notice.as_deref(),
            Some("TEAM SHARD GOAL REACHED  4/4")
        );
        assert_eq!(
            panel.great_rune_goal_notice.as_deref(),
            Some("TEAM GREAT RUNE GOAL REACHED  2/2")
        );
    }
}

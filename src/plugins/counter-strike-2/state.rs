use cs2_gsi::model::{GameState, MapPhase, RoundPhase};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MatchState {
    Inactive,
    ActiveRound,
    PlayerDead,
    BetweenRounds,
}

pub(super) fn resolve(snapshot: &GameState) -> MatchState {
    let Some(map) = snapshot.map.as_ref() else {
        return MatchState::Inactive;
    };
    match map.phase {
        MapPhase::Intermission => return MatchState::BetweenRounds,
        MapPhase::Live => {}
        MapPhase::Warmup | MapPhase::Gameover | MapPhase::Unknown => {
            return MatchState::Inactive;
        }
    }
    let Some(round) = snapshot.round.as_ref() else {
        return MatchState::Inactive;
    };
    match round.phase {
        RoundPhase::Freezetime | RoundPhase::Over => MatchState::BetweenRounds,
        RoundPhase::Live if local_player_is_dead(snapshot) => MatchState::PlayerDead,
        RoundPhase::Live if snapshot.player.is_some() => MatchState::ActiveRound,
        RoundPhase::Live | RoundPhase::Unknown => MatchState::Inactive,
    }
}

fn local_player_is_dead(snapshot: &GameState) -> bool {
    let Some(player) = snapshot.player.as_ref() else {
        return false;
    };
    let local_steam_id = snapshot.provider.steamid.trim();
    let observed_steam_id = player.steamid.trim();
    let spectating_another_player = !local_steam_id.is_empty()
        && !observed_steam_id.is_empty()
        && local_steam_id != observed_steam_id;
    spectating_another_player || snapshot.local_is_dead()
}

#[cfg(test)]
mod tests {
    use cs2_gsi::model::{Map, Player, PlayerActivity, PlayerState, Provider, Round};

    use super::*;

    fn snapshot(round: RoundPhase, health: i32) -> GameState {
        GameState {
            provider: Provider {
                steamid: "local-player".into(),
                ..Provider::default()
            },
            map: Some(Map {
                phase: MapPhase::Live,
                ..Map::default()
            }),
            round: Some(Round {
                phase: round,
                ..Round::default()
            }),
            player: Some(Player {
                steamid: "local-player".into(),
                activity: PlayerActivity::Playing,
                state: PlayerState {
                    health,
                    ..PlayerState::default()
                },
                ..Player::default()
            }),
            ..GameState::default()
        }
    }

    #[test]
    fn round_end_beats_death_in_the_same_snapshot() {
        assert_eq!(
            resolve(&snapshot(RoundPhase::Over, 0)),
            MatchState::BetweenRounds
        );
    }

    #[test]
    fn live_health_selects_alive_and_dead_states() {
        assert_eq!(
            resolve(&snapshot(RoundPhase::Live, 100)),
            MatchState::ActiveRound
        );
        assert_eq!(
            resolve(&snapshot(RoundPhase::Live, 0)),
            MatchState::PlayerDead
        );
    }

    #[test]
    fn spectating_a_living_teammate_keeps_the_local_player_dead() {
        let mut state = snapshot(RoundPhase::Live, 100);
        state.player.as_mut().unwrap().steamid = "living-teammate".into();

        assert_eq!(resolve(&state), MatchState::PlayerDead);
    }

    #[test]
    fn warmup_releases_playback_control() {
        let mut state = snapshot(RoundPhase::Freezetime, 100);
        state.map.as_mut().unwrap().phase = MapPhase::Warmup;
        assert_eq!(resolve(&state), MatchState::Inactive);
    }

    #[test]
    fn halftime_uses_the_between_rounds_rule() {
        let mut state = snapshot(RoundPhase::Over, 100);
        state.map.as_mut().unwrap().phase = MapPhase::Intermission;
        state.round = None;
        assert_eq!(resolve(&state), MatchState::BetweenRounds);
    }
}

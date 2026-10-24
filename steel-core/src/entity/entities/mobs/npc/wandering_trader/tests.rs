//! Wandering trader spawner rule tests (vanilla `WanderingTraderSpawner`).

use super::spawner::{
    DEFAULT_SPAWN_DELAY, DEFAULT_TICK_DELAY, MIN_SPAWN_CHANCE, WANDERING_TRADER_DESPAWN_DELAY,
    WanderingTraderData, WanderingTraderSpawner,
};

#[test]
fn trader_data_defaults_match_vanilla() {
    let data = WanderingTraderData::default();

    assert_eq!(data.spawn_delay, 24000);
    assert_eq!(data.spawn_chance, MIN_SPAWN_CHANCE);
}

#[test]
fn spawner_starts_with_vanilla_timers() {
    let spawner = WanderingTraderSpawner::new();

    assert_eq!(spawner.data().spawn_delay, DEFAULT_SPAWN_DELAY);
    assert_eq!(spawner.data().spawn_chance, MIN_SPAWN_CHANCE);
    assert_eq!(DEFAULT_TICK_DELAY, 1200);
}

#[test]
fn spawner_ticks_down_by_one_evaluation_per_minute() {
    let spawner = WanderingTraderSpawner::new();
    let mut data = spawner.data();
    // Vanilla decrements `spawnDelay` by `DEFAULT_TICK_DELAY` per evaluation.
    data.set_spawn_delay(data.spawn_delay - DEFAULT_TICK_DELAY);

    assert_eq!(data.spawn_delay, 24_000 - 1_200);
}

#[test]
fn spawn_chance_grows_by_25_and_clamps_at_75() {
    let mut chance = MIN_SPAWN_CHANCE;
    let mut observed = Vec::new();
    for _ in 0..4 {
        chance = (chance + 25).clamp(25, 75);
        observed.push(chance);
    }

    assert_eq!(observed, vec![50, 75, 75, 75]);
}

#[test]
fn trader_despawn_delay_is_two_in_game_days() {
    assert_eq!(WANDERING_TRADER_DESPAWN_DELAY, 48_000);
}

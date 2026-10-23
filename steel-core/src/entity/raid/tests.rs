//! Raid wave composition and status transition tests.

use std::collections::BTreeSet;

use steel_utils::BlockPos;
use steel_utils::types::Difficulty;

use super::raid::{Raid, RaidStatus, RaiderType};

fn raid(difficulty: Difficulty) -> Raid {
    Raid::new(BlockPos::new(0, 64, 0), difficulty)
}

#[test]
fn group_counts_match_vanilla_difficulty_table() {
    assert_eq!(Raid::num_groups(Difficulty::Peaceful), 0);
    assert_eq!(Raid::num_groups(Difficulty::Easy), 3);
    assert_eq!(Raid::num_groups(Difficulty::Normal), 5);
    assert_eq!(Raid::num_groups(Difficulty::Hard), 7);
}

#[test]
fn fresh_raid_starts_in_the_pre_raid_window() {
    let raid = raid(Difficulty::Normal);
    assert_eq!(raid.status(), RaidStatus::Ongoing);
    assert!(raid.is_active());
    assert!(!raid.is_started());
    assert!(!raid.has_first_wave_spawned());
    assert_eq!(raid.groups_spawned(), 0);
    assert_eq!(raid.raid_omen_level(), 0);
    // The first wave is gated behind the 300-tick pre-raid cooldown.
    assert_eq!(
        raid.raid_cooldown_ticks(),
        super::raid::DEFAULT_PRE_RAID_TICKS
    );
    assert!(!raid.should_spawn_group());
    assert!(!raid.has_bonus_wave());
}

#[test]
fn default_wave_spawn_tables_match_vanilla() {
    let raid = raid(Difficulty::Normal);

    // Pillager `{0, 4, 3, 3, 4, 4, 4, 2}` and vindicator `{0, 0, 2, 0, 1, 4, 2, 5}`.
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Pillager, 1, false),
        4
    );
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Pillager, 2, false),
        3
    );
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Pillager, 7, false),
        2
    );
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Vindicator, 2, false),
        2
    );
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Vindicator, 5, false),
        4
    );
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Vindicator, 7, false),
        5
    );

    // Witch `{0, 0, 0, 0, 3, 0, 0, 1}` and ravager `{0, 0, 0, 1, 0, 1, 0, 2}`.
    assert_eq!(raid.get_default_num_spawns(RaiderType::Witch, 4, false), 3);
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Ravager, 3, false),
        1
    );
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Ravager, 7, false),
        2
    );

    // Evoker `{0, 0, 0, 0, 0, 1, 1, 2}`.
    assert_eq!(raid.get_default_num_spawns(RaiderType::Evoker, 4, false), 0);
    assert_eq!(raid.get_default_num_spawns(RaiderType::Evoker, 6, false), 1);

    // A bonus wave reads the difficulty group count instead of the wave index.
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Pillager, 1, true),
        4
    );
    assert_eq!(
        raid.get_default_num_spawns(RaiderType::Vindicator, 1, true),
        4
    );
    assert_eq!(raid.get_default_num_spawns(RaiderType::Ravager, 1, true), 1);
}

#[test]
fn bonus_spawns_are_zero_without_bonus_wave_and_bounded_with_one() {
    let mut raid = raid(Difficulty::Normal);

    // Evokers never get bonus spawns, on any difficulty or wave.
    assert_eq!(
        raid.get_potential_bonus_spawns(RaiderType::Evoker, 6, Difficulty::Hard, true),
        0
    );
    // Witches are skipped on easy difficulty, early waves and wave 4.
    assert_eq!(
        raid.get_potential_bonus_spawns(RaiderType::Witch, 5, Difficulty::Easy, false),
        0
    );
    assert_eq!(
        raid.get_potential_bonus_spawns(RaiderType::Witch, 2, Difficulty::Hard, false),
        0
    );
    assert_eq!(
        raid.get_potential_bonus_spawns(RaiderType::Witch, 4, Difficulty::Hard, false),
        0
    );
    // Ravagers only roll on a bonus wave, and never on easy.
    assert_eq!(
        raid.get_potential_bonus_spawns(RaiderType::Ravager, 4, Difficulty::Hard, false),
        0
    );
    assert_eq!(
        raid.get_potential_bonus_spawns(RaiderType::Ravager, 4, Difficulty::Easy, true),
        0
    );

    // Pillagers/vindicators roll `0..=cap`, so the total stays within the vanilla bound.
    let base = raid.get_default_num_spawns(RaiderType::Pillager, 1, false);
    for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
        for _ in 0..32 {
            let count = raid.wave_raider_count(RaiderType::Pillager, 1, difficulty, false);
            assert!(count >= base, "raider count never drops below the table");
            assert!(
                count <= base + 2,
                "bonus spawns stay within the vanilla cap"
            );
        }
    }
}

#[test]
fn cooldown_and_status_transitions_follow_vanilla_rules() {
    let mut raid = raid(Difficulty::Easy);

    // A stopped raid is not "over": vanilla keeps it around until the manager drops it.
    raid.stop();
    assert!(raid.is_stopped());
    assert!(!raid.is_over());
    assert!(!raid.is_active());

    // Victory/loss are the only "over" states, and both allow a bonus wave check.
    let victory = Raid::from_saved(
        true,
        true,
        100,
        2,
        3,
        0,
        0,
        0.0,
        3,
        RaidStatus::Victory,
        BlockPos::new(0, 64, 0),
        BTreeSet::new(),
    );
    assert!(victory.is_victory());
    assert!(victory.is_over());
    assert!(
        !victory.has_more_waves(),
        "final wave with no bonus wave left"
    );
    assert!(
        victory.has_bonus_wave(),
        "omen level 2 unlocks the bonus wave"
    );

    let loss = Raid::from_saved(
        true,
        true,
        100,
        1,
        2,
        0,
        0,
        0.0,
        5,
        RaidStatus::Loss,
        BlockPos::new(0, 64, 0),
        BTreeSet::new(),
    );
    assert!(loss.is_loss());
    assert!(loss.is_over());
    assert!(
        loss.has_more_waves(),
        "waves beyond the spawned count remain"
    );
}

#[test]
fn between_waves_and_bonus_wave_gating_match_vanilla() {
    let mut raid = raid(Difficulty::Normal);
    assert!(!raid.is_between_waves(), "no wave spawned yet");

    // Simulate the end of wave 1: one spawned wave, no raiders alive, cooldown running.
    let mut spawned = Raid::from_saved(
        true,
        true,
        200,
        1,
        1,
        super::raid::DEFAULT_PRE_RAID_TICKS,
        0,
        10.0,
        5,
        RaidStatus::Ongoing,
        BlockPos::new(0, 64, 0),
        BTreeSet::new(),
    );
    assert!(spawned.has_first_wave_spawned());
    assert!(spawned.is_between_waves());
    assert!(!spawned.should_spawn_group(), "cooldown still running");
    assert!(!spawned.should_spawn_bonus_group());

    // Omen level 1 keeps the bonus wave locked; level 2 unlocks it after the final wave.
    assert!(!spawned.has_bonus_wave());
    spawned.set_raid_omen_level(2);
    assert!(spawned.has_bonus_wave());
    assert!(spawned.has_more_waves());
    assert_eq!(spawned.enchant_odds(), 0.1);
}

#[test]
fn raid_status_names_round_trip() {
    for status in RaidStatus::VALUES {
        assert_eq!(
            RaidStatus::from_serialized_name(status.serialized_name()),
            Some(status)
        );
    }
    assert_eq!(RaidStatus::from_serialized_name("unknown"), None);
}

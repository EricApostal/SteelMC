//! Raid wave composition, status transition and boss-bar tests.

use std::collections::BTreeSet;
use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::{BossBarColor, BossBarOverlay, BossEventOperation};
use steel_registry::{
    REGISTRY, RegistryExt as _, init_vanilla_registry, vanilla_entities, vanilla_poi_types,
};
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, ChunkPos, translations};
use text_components::TextComponent;

use crate::entity::spawn::create_entity_instance;
use crate::entity::{Player, SharedEntity};
use crate::test_support::{
    TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
};

use super::raid::{DEFAULT_PRE_RAID_TICKS, MAX_CELEBRATION_TICKS, Raid, RaidStatus, RaiderType};

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
        victory.has_bonus_wave(),
        "omen level 2 unlocks the bonus wave"
    );
    assert!(
        victory.has_more_waves(),
        "vanilla `hasMoreWaves` keeps the raid open while the bonus wave is unspawned"
    );

    let bonus_spawned = Raid::from_saved(
        true,
        true,
        100,
        2,
        4,
        0,
        0,
        0.0,
        3,
        RaidStatus::Victory,
        BlockPos::new(0, 64, 0),
        BTreeSet::new(),
    );
    assert!(
        bonus_spawned.has_spawned_bonus_wave(),
        "groups spawned beyond the normal waves are the bonus wave"
    );
    assert!(
        !bonus_spawned.has_more_waves(),
        "a spawned bonus wave is the last wave"
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
    let raid = raid(Difficulty::Normal);
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

// ---------------------------------------------------------------------------
// Boss bar (`Raid.raidEvent`) tests.
// ---------------------------------------------------------------------------

const fn raid_center() -> BlockPos {
    BlockPos::new(8, 64, 8)
}

fn raid_name() -> TextComponent {
    TextComponent::translated(translations::EVENT_MINECRAFT_RAID.msg())
}

fn victory_name() -> TextComponent {
    TextComponent::translated(translations::EVENT_MINECRAFT_RAID_VICTORY_FULL.msg())
}

fn defeat_name() -> TextComponent {
    TextComponent::translated(translations::EVENT_MINECRAFT_RAID_DEFEAT_FULL.msg())
}

/// A test world holding an occupied village POI and a loaded chunk at the raid center,
/// plus one player registered in the world.
fn boss_bar_fixture(key: &'static str) -> (TestWorld, Arc<Player>) {
    boss_bar_fixture_at(key, raid_center())
}

/// [`boss_bar_fixture`] with the village and its loaded chunk placed at `center`.
///
/// The player stays at the world origin, so `center` fixes the player's distance from
/// the raid.
fn boss_bar_fixture_at(key: &'static str, center: BlockPos) -> (TestWorld, Arc<Player>) {
    init_vanilla_registry();
    let fixture = fresh_test_world(key);
    let world = Arc::clone(&fixture.world);
    let home = REGISTRY
        .poi_types
        .id_from_key(&vanilla_poi_types::HOME.key)
        .expect("home POI type is registered");
    world.poi_storage.lock().add(center, home, 1);
    world.poi_storage.lock().reserve_ticket(center);
    insert_ready_full_chunk(&world, ChunkPos::from_block_pos(center));

    let player = TestPlayerBuilder::new(Arc::clone(&world), "RaidBarViewer", 1).build();
    assert!(world.players.insert(Arc::clone(&player)));
    (fixture, player)
}

/// A raid in the between-waves window whose `updatePlayers` gate fires on the next tick.
fn between_waves_raid(status: RaidStatus) -> Raid {
    Raid::from_saved(
        true,
        true,
        19,
        1,
        1,
        DEFAULT_PRE_RAID_TICKS - 1,
        0,
        0.0,
        5,
        status,
        raid_center(),
        BTreeSet::new(),
    )
}

#[test]
fn starting_a_raid_raises_a_visible_boss_event_with_wave_progress() {
    let (fixture, _player) = boss_bar_fixture("raid_boss_bar_start");
    let world = Arc::clone(&fixture.world);

    let mut raid = Raid::new(raid_center(), Difficulty::Normal);
    // Vanilla `/raid` and `Raids.createOrExtendRaid` set the omen level on the new raid.
    raid.set_raid_omen_level(1);
    let bar_id = raid.raid_event().id();
    assert!(
        raid.raid_event().is_visible(),
        "vanilla's `ServerBossEvent` starts visible"
    );
    assert_eq!(
        raid.raid_event().player_count(),
        0,
        "nobody tracks the bar before the first tick"
    );

    raid.tick(&world, 1);

    {
        let event = raid.raid_event();
        assert!(event.is_visible());
        assert_eq!(
            event.player_count(),
            1,
            "the player standing in the village tracks the raid bar"
        );
        assert_eq!(event.name(), &raid_name());
        assert_eq!(event.color(), BossBarColor::Red);
        assert_eq!(event.overlay(), BossBarOverlay::Notched10);
        // Vanilla `Raid.tick` sets `(300 - raidCooldownTicks) / 300.0F` after decrementing
        // the cooldown, so the first tick of a fresh raid reports 1/300.
        assert!(
            (event.progress() - 1.0 / 300.0).abs() < 1e-6,
            "unexpected progress {}",
            event.progress()
        );
    }

    let packets = raid.raid_event_mut().drain_packets();
    assert!(
        packets.iter().all(|packet| packet.id == bar_id),
        "every operation targets the raid's own bar id"
    );
    assert_eq!(packets.len(), 2, "expected add then progress update");
    match &packets[0].operation {
        BossEventOperation::Add {
            name,
            progress,
            color,
            overlay,
            darken_screen,
            play_music,
            create_world_fog,
        } => {
            assert_eq!(name, &raid_name());
            // Vanilla's `ServerBossEvent` constructor leaves progress at 1.0, and
            // `Raid.tick` adds the player before lowering it, so the add carries 1.0.
            assert!((progress - 1.0).abs() < 1e-6);
            assert_eq!(*color, BossBarColor::Red);
            assert_eq!(*overlay, BossBarOverlay::Notched10);
            assert!(!darken_screen && !play_music && !create_world_fog);
        }
        other => panic!("expected the add operation, got {other:?}"),
    }
    match &packets[1].operation {
        BossEventOperation::UpdateProgress { progress } => {
            assert!((progress - 1.0 / 300.0).abs() < 1e-6);
        }
        other => panic!("expected the progress operation, got {other:?}"),
    }

    // The pre-raid window fills the bar up to 1.0 over its 300 ticks.
    for _ in 0..29 {
        raid.tick(&world, 1);
    }
    let progress = raid.raid_event().progress();
    assert!(
        (progress - 30.0 / 300.0).abs() < 1e-6,
        "progress climbs one tick per wave cooldown tick, got {progress}"
    );
    assert!((0.0..=1.0).contains(&progress));
}

#[test]
fn the_boss_event_tracks_players_strictly_inside_the_raid_radius() {
    // The fixture's player sits at the world origin, so the raid center fixes its
    // distance. Vanilla `Raids.getNearbyRaid` keeps positions *strictly* closer than
    // `Raid.VALID_RAID_RADIUS` (96 blocks): 95 blocks is inside ...
    let inside = BlockPos::new(95, 0, 0);
    let (fixture, _player) = boss_bar_fixture_at("raid_boss_bar_radius_inside", inside);
    let world = Arc::clone(&fixture.world);
    let mut raid = between_waves_raid(RaidStatus::Ongoing);
    raid.set_center(inside);
    raid.tick(&world, 1);
    assert_eq!(
        raid.raid_event().player_count(),
        1,
        "a player 95 blocks from the center tracks the bar"
    );

    // ... and exactly 96 blocks away is outside.
    let edge = BlockPos::new(96, 0, 0);
    let (fixture, _player) = boss_bar_fixture_at("raid_boss_bar_radius_edge", edge);
    let world = Arc::clone(&fixture.world);
    let mut raid = between_waves_raid(RaidStatus::Ongoing);
    raid.set_center(edge);
    raid.tick(&world, 1);
    assert_eq!(
        raid.raid_event().player_count(),
        0,
        "a player exactly 96 blocks from the center is not tracked"
    );
}

#[test]
fn stopping_a_raid_removes_its_boss_bar_from_every_tracker() {
    let (fixture, _player) = boss_bar_fixture("raid_boss_bar_stop");
    let world = Arc::clone(&fixture.world);
    let mut raid = between_waves_raid(RaidStatus::Ongoing);

    raid.tick(&world, 1);
    assert_eq!(raid.raid_event().player_count(), 1);
    let bar_id = raid.raid_event().id();
    assert!(
        raid.raid_event_mut()
            .drain_packets()
            .iter()
            .any(|packet| matches!(packet.operation, BossEventOperation::Add { .. })),
        "the first tick raises the bar"
    );

    raid.stop();

    assert!(raid.is_stopped());
    assert_eq!(
        raid.raid_event().player_count(),
        0,
        "vanilla `removeAllPlayers` forgets every tracker"
    );
    assert!(
        raid.raid_event().is_visible(),
        "vanilla `Raid.stop` leaves the visible flag alone"
    );
    let packets = raid.raid_event_mut().drain_packets();
    assert_eq!(
        packets.len(),
        1,
        "stopping queues exactly one operation per tracker"
    );
    assert_eq!(packets[0].id, bar_id);
    assert!(matches!(packets[0].operation, BossEventOperation::Remove));
}

#[test]
fn winning_a_raid_shows_the_victory_bar_and_then_removes_it() {
    let (fixture, _player) = boss_bar_fixture("raid_boss_bar_victory");
    let world = Arc::clone(&fixture.world);
    let mut raid = between_waves_raid(RaidStatus::Victory);
    let bar_id = raid.raid_event().id();

    for _ in 0..20 {
        raid.tick(&world, 1);
    }

    assert_eq!(raid.raid_event().player_count(), 1);
    assert!(raid.raid_event().is_visible());
    assert_eq!(raid.raid_event().name(), &victory_name());
    assert!(
        (raid.raid_event().progress() - 0.0).abs() < 1e-6,
        "vanilla clears the progress for the victory bar"
    );
    assert!(
        raid
            .raid_event_mut()
            .drain_packets()
            .iter()
            .any(|packet| matches!(&packet.operation, BossEventOperation::UpdateName { name } if name == &victory_name())),
        "the victory title reaches the tracker"
    );

    for _ in 20..=MAX_CELEBRATION_TICKS {
        raid.tick(&world, 1);
    }
    assert!(raid.is_stopped(), "the celebration ends the raid");
    assert_eq!(raid.raid_event().player_count(), 0);
    let packets = raid.raid_event_mut().drain_packets();
    assert!(
        packets
            .iter()
            .any(|packet| packet.id == bar_id
                && matches!(packet.operation, BossEventOperation::Remove)),
        "the victory bar is taken down once the celebration ends"
    );
}

#[test]
fn losing_a_raid_shows_the_defeat_bar_and_then_removes_it() {
    let (fixture, _player) = boss_bar_fixture("raid_boss_bar_defeat");
    let world = Arc::clone(&fixture.world);
    let mut raid = between_waves_raid(RaidStatus::Loss);
    let bar_id = raid.raid_event().id();

    for _ in 0..20 {
        raid.tick(&world, 1);
    }

    assert_eq!(raid.raid_event().player_count(), 1);
    assert!(raid.raid_event().is_visible());
    assert_eq!(raid.raid_event().name(), &defeat_name());

    for _ in 20..=MAX_CELEBRATION_TICKS {
        raid.tick(&world, 1);
    }
    assert!(raid.is_stopped());
    assert_eq!(raid.raid_event().player_count(), 0);
    let packets = raid.raid_event_mut().drain_packets();
    assert!(
        packets
            .iter()
            .any(|packet| packet.id == bar_id
                && matches!(packet.operation, BossEventOperation::Remove)),
        "the defeat bar is taken down once the celebration ends"
    );
}

#[test]
fn adding_a_damaged_raider_refreshes_the_boss_bar_progress() {
    let (fixture, _player) = boss_bar_fixture("raid_boss_bar_join");
    let world = Arc::clone(&fixture.world);

    let raider: SharedEntity =
        create_entity_instance(&world, &vanilla_entities::PILLAGER, DVec3::ZERO)
            .expect("pillager can be created in the test world");
    world
        .try_add_entity(SharedEntity::clone(&raider))
        .expect("register the pillager");
    raider
        .as_living_entity()
        .expect("pillagers are living entities")
        .set_health(10.0);

    // The raid already carries 20.0 total health from two raiders at full health.
    let mut raid = Raid::from_saved(
        true,
        true,
        100,
        1,
        1,
        0,
        0,
        20.0,
        5,
        RaidStatus::Ongoing,
        raid_center(),
        BTreeSet::new(),
    );

    raid.add_wave_mob(&world, 1, &raider, true);

    // Vanilla `Raid.addWaveMob` adds the raider's health and then calls `updateBossbar`,
    // which reports `healthOfLivingRaiders / totalHealth`.
    assert!(
        (raid.raid_event().progress() - 10.0 / 30.0).abs() < 1e-6,
        "expected 10/30 after the join, got {}",
        raid.raid_event().progress()
    );
}

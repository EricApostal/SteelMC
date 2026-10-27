//! Driver tests: every auto criterion moves a sidebar score from the vanilla
//! entry point that feeds it, and `dummy` criteria stay inert.

use std::slice;
use std::sync::Arc;

use text_components::TextComponent;
use tokio::runtime::Builder;

use crate::config::ResolvedDomainConfig;
use crate::entity::{Entity, LivingEntity, damage::DamageSource, init_entities};
use crate::permission::PermissionSubjectIndex;
use crate::player::Player;
use crate::scoreboard::{DisplaySlot, ObjectiveCriteria, RenderType, ScoreHolder, Scoreboard};
use crate::server::Server;
use crate::server::test_server_with_worlds_and_config;
use crate::test_support::{
    TestEntity, TestPlayerBuilder, TestWorld, fresh_test_world_in_domain, test_runtime_config,
};
use crate::world::World;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{
    init_vanilla_registry, vanilla_blocks, vanilla_damage_types, vanilla_entities,
};

/// A live server for one fresh domain so the driver has a domain scoreboard.
///
/// The `TestWorld` handle is held (not just its world) because it owns the
/// world's thread-local damage history.
struct ScoreboardTestSetup {
    server: Arc<Server>,
    world: Arc<World>,
    #[expect(
        dead_code,
        reason = "holding the fixture keeps its damage history alive"
    )]
    fixture: TestWorld,
}

fn set_up(domain_key: &'static str) -> ScoreboardTestSetup {
    let fixture = fresh_test_world_in_domain("scoreboard", domain_key);
    let world = Arc::clone(&fixture.world);
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    let server = runtime.block_on(async {
        let domain = ResolvedDomainConfig {
            name: world.domain().to_owned(),
            default_world: world.key.clone(),
            worlds: vec![world.key.clone()],
        };
        test_server_with_worlds_and_config(
            domain.name.clone(),
            slice::from_ref(&domain),
            slice::from_ref(&world),
            PermissionSubjectIndex::new(),
            test_runtime_config(1),
        )
        .await
        .expect("test server should initialize")
    });
    ScoreboardTestSetup {
        server,
        world,
        fixture,
    }
}

fn test_player(setup: &ScoreboardTestSetup, name: &str, entity_id: i32) -> Arc<Player> {
    TestPlayerBuilder::new(Arc::clone(&setup.world), name, entity_id)
        .server(&setup.server)
        .build()
}

fn domain_scoreboard(setup: &ScoreboardTestSetup) -> &Scoreboard {
    setup
        .server
        .scoreboards
        .get(setup.world.domain())
        .expect("domain scoreboard should exist")
}

fn score_of(scoreboard: &Scoreboard, holder: &str, objective: &str) -> Option<i32> {
    let objective = scoreboard.objective(objective)?;
    scoreboard.score(&ScoreHolder::new(holder), &objective)
}

fn track_on_sidebar(scoreboard: &Scoreboard, objective_name: &str) {
    let objective = scoreboard
        .objective(objective_name)
        .expect("objective should exist");
    scoreboard
        .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
        .expect("display slot should accept the objective");
}

#[test]
fn scoreboard_mined_stat_moves_from_block_mine() {
    init_vanilla_registry();
    let setup = set_up("mined_stat");
    let player = test_player(&setup, "Miner", 1);
    let scoreboard = domain_scoreboard(&setup);

    let objective = scoreboard
        .add_objective_full(
            "mined",
            ObjectiveCriteria::parse("minecraft.mined:minecraft.stone")
                .expect("mined criteria should parse"),
            TextComponent::plain("Mined"),
            RenderType::Integer,
            false,
            None,
        )
        .expect("objective should be added");
    track_on_sidebar(scoreboard, "mined");
    // Only the objective-adding packets exist so far.
    let _ = scoreboard.pending_packets();

    // Vanilla `ServerPlayer.awardStat`: awarding the stat pushes it into the
    // matching objective.
    player.award_stat(&vanilla_stat_types::BLOCK_MINED, &vanilla_blocks::STONE);
    assert_eq!(score_of(scoreboard, "Miner", "mined"), Some(1));

    // The sidebar score reaches the client in the same flush as the packet.
    let packets = scoreboard.pending_packets();
    assert!(
        !packets.is_empty(),
        "writing a displayed objective should queue a packet"
    );
    let _ = objective;
}

#[test]
fn scoreboard_equivalent_stat_spellings_move_together() {
    init_vanilla_registry();
    let setup = set_up("mined_spelling");
    let player = test_player(&setup, "Miner", 1);
    let scoreboard = domain_scoreboard(&setup);

    // The dotted and the colon spelling name the same vanilla criterion.
    let _dotted = scoreboard
        .add_objective_full(
            "dotted",
            ObjectiveCriteria::parse("minecraft.mined:minecraft.stone")
                .expect("stat criteria should parse"),
            TextComponent::plain("Mined"),
            RenderType::Integer,
            false,
            None,
        )
        .expect("objective should be added");
    let _short = scoreboard
        .add_objective_full(
            "short",
            ObjectiveCriteria::parse("mined:stone").expect("short spelling should parse"),
            TextComponent::plain("Mined"),
            RenderType::Integer,
            false,
            None,
        )
        .expect("objective should be added");

    player.award_stat(&vanilla_stat_types::BLOCK_MINED, &vanilla_blocks::STONE);
    assert_eq!(score_of(scoreboard, "Miner", "dotted"), Some(1));
    assert_eq!(score_of(scoreboard, "Miner", "short"), Some(1));

    // Resetting the stat clears every objective tracking it, like vanilla
    // `ServerPlayer.resetStat`.
    player.reset_stat(vanilla_stat_types::BLOCK_MINED.get(&vanilla_blocks::STONE));
    assert_eq!(score_of(scoreboard, "Miner", "dotted"), Some(0));
    assert_eq!(score_of(scoreboard, "Miner", "short"), Some(0));
}

#[test]
fn scoreboard_death_count_moves_on_death() {
    init_vanilla_registry();
    let setup = set_up("death_count");
    let player = test_player(&setup, "Doomed", 1);
    player.set_health(0.0);
    let scoreboard = domain_scoreboard(&setup);

    let _objective = scoreboard
        .add_objective_full(
            "deaths",
            ObjectiveCriteria::death_count(),
            TextComponent::plain("Deaths"),
            RenderType::Integer,
            false,
            None,
        )
        .expect("objective should be added");
    track_on_sidebar(scoreboard, "deaths");

    // Vanilla `ServerPlayer.die` pushes `DEATH_COUNT` before the kill credit.
    player.die(&DamageSource::environment(&vanilla_damage_types::GENERIC));
    assert_eq!(score_of(scoreboard, "Doomed", "deaths"), Some(1));
}

#[test]
fn scoreboard_kill_counts_move_on_kill() {
    init_vanilla_registry();
    init_entities();
    let setup = set_up("kill_counts");
    let killer = test_player(&setup, "Killer", 1);
    let player_victim = test_player(&setup, "Victim", 2);
    let scoreboard = domain_scoreboard(&setup);

    for (name, criteria) in [
        ("kills", ObjectiveCriteria::player_kill_count()),
        ("total", ObjectiveCriteria::total_kill_count()),
        (
            "killed_players",
            ObjectiveCriteria::parse("minecraft.killed:minecraft.player")
                .expect("killed criteria should parse"),
        ),
    ] {
        scoreboard
            .add_objective_full(
                name,
                criteria,
                TextComponent::plain(name),
                RenderType::Integer,
                false,
                None,
            )
            .expect("objective should be added");
    }

    // Vanilla `ServerPlayer.awardKillScore`: a player kill raises the kill
    // criteria (`minecraft.killed:minecraft.player` moves through its own
    // criteria driver, covered by the stat tests above).
    killer.award_kill_score(
        &*player_victim as &dyn Entity,
        &DamageSource::environment(&vanilla_damage_types::GENERIC),
    );
    assert_eq!(score_of(scoreboard, "Killer", "kills"), Some(1));
    assert_eq!(score_of(scoreboard, "Killer", "total"), Some(1));

    // The stat-driven half: the killer's own kill stat pushes every objective
    // tracking it, which is how `minecraft.killed:minecraft.player` sidebars move.
    killer.award_stat(
        &vanilla_stat_types::ENTITY_KILLED,
        &vanilla_entities::PLAYER,
    );
    assert_eq!(score_of(scoreboard, "Killer", "killed_players"), Some(1));

    // A mob kill only advances `totalKillCount` (and the stat-driven criteria).
    // The mob id is fixed so it can never collide with the killer's id, which
    // `awardKillScore` uses to ignore a self-kill.
    let mob = TestEntity::shared(
        i32::MAX,
        glam::DVec3::new(0.0, 64.0, 0.0),
        Arc::downgrade(&setup.world),
        &vanilla_entities::PIG,
    );
    killer.award_kill_score(
        &*mob,
        &DamageSource::environment(&vanilla_damage_types::GENERIC),
    );
    assert_eq!(score_of(scoreboard, "Killer", "kills"), Some(1));
    assert_eq!(score_of(scoreboard, "Killer", "total"), Some(2));
}

#[test]
fn scoreboard_derived_criteria_follow_player_state() {
    init_vanilla_registry();
    let setup = set_up("derived");
    let player = test_player(&setup, "Tracked", 1);
    let scoreboard = domain_scoreboard(&setup);

    for (name, criteria) in [
        ("health", ObjectiveCriteria::health()),
        ("food", ObjectiveCriteria::food()),
        ("air", ObjectiveCriteria::air()),
        ("armor", ObjectiveCriteria::armor()),
        ("xp", ObjectiveCriteria::experience()),
        ("level", ObjectiveCriteria::level()),
    ] {
        scoreboard
            .add_objective_full(
                name,
                criteria,
                TextComponent::plain(name),
                RenderType::Integer,
                false,
                None,
            )
            .expect("objective should be added");
    }

    // Vanilla's first tick writes every derived criterion once.
    player.tick_objective_criteria();
    assert_eq!(score_of(scoreboard, "Tracked", "health"), Some(20));
    assert_eq!(score_of(scoreboard, "Tracked", "food"), Some(20));
    assert_eq!(score_of(scoreboard, "Tracked", "air"), Some(300));
    assert_eq!(score_of(scoreboard, "Tracked", "armor"), Some(0));
    assert_eq!(score_of(scoreboard, "Tracked", "xp"), Some(0));
    assert_eq!(score_of(scoreboard, "Tracked", "level"), Some(0));

    // An unchanged tick broadcasts nothing.
    let _ = scoreboard.pending_packets();
    player.tick_objective_criteria();
    assert!(
        scoreboard.pending_packets().is_empty(),
        "unchanged criteria should not queue packets"
    );

    // Changed values update through the same per-tick path.
    player.set_health(7.5);
    player.food_data.lock().food_level = 14;
    player.set_air_supply(120);
    player.tick_objective_criteria();
    assert_eq!(score_of(scoreboard, "Tracked", "health"), Some(8));
    assert_eq!(score_of(scoreboard, "Tracked", "food"), Some(14));
    assert_eq!(score_of(scoreboard, "Tracked", "air"), Some(120));
    assert_eq!(score_of(scoreboard, "Tracked", "armor"), Some(0));

    // Experience points and levels update independently: points first (fewer
    // than the 7 needed to roll a level), then the level on its own tick.
    player.experience.lock().add_points(5);
    player.tick_objective_criteria();
    assert_eq!(score_of(scoreboard, "Tracked", "xp"), Some(5));
    assert_eq!(score_of(scoreboard, "Tracked", "level"), Some(0));
    player.experience.lock().add_levels(2);
    player.tick_objective_criteria();
    assert_eq!(score_of(scoreboard, "Tracked", "level"), Some(2));
}

#[test]
fn scoreboard_dummy_objective_stays_inert() {
    init_vanilla_registry();
    init_entities();
    let setup = set_up("dummy_inert");
    let player = test_player(&setup, "Steady", 1);
    let scoreboard = domain_scoreboard(&setup);

    let _dummy = scoreboard
        .add_objective("untouched")
        .expect("objective should be added");

    // Stat awards, kill awards and the derived-criteria tick never touch it.
    player.award_stat(&vanilla_stat_types::BLOCK_MINED, &vanilla_blocks::STONE);
    player.tick_objective_criteria();
    assert_eq!(score_of(scoreboard, "Steady", "untouched"), None);

    player.set_health(0.0);
    player.die(&DamageSource::environment(&vanilla_damage_types::GENERIC));
    let victim = test_player(&setup, "Other", 2);
    player.award_kill_score(
        &*victim as &dyn Entity,
        &DamageSource::environment(&vanilla_damage_types::GENERIC),
    );
    assert_eq!(score_of(scoreboard, "Steady", "untouched"), None);
}

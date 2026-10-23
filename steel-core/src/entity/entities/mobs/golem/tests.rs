use std::io::Cursor;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::read_compound as read_borrowed_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::{init_vanilla_registry, vanilla_entities};
use steel_utils::UuidExt as _;

use super::*;
use crate::entity::entities::PigEntity;
use crate::entity::neutral_mob::NO_ANGER_END_TIME;
use crate::entity::{Entity, EntityReference, EntitySpawnReason, Mob};

fn golem() -> IronGolemEntity {
    init_vanilla_registry();
    IronGolemEntity::new(&vanilla_entities::IRON_GOLEM, 1, DVec3::ZERO, Weak::new())
}

#[test]
fn player_created_flag_round_trips_through_nbt() {
    let golem = golem();
    assert!(!golem.is_player_created());

    golem.set_player_created(true);
    let mut nbt = NbtCompound::new();
    golem.save_additional(&mut nbt);
    assert_eq!(nbt.byte("PlayerCreated"), Some(1));

    golem.set_player_created(false);
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_borrowed_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
    golem.load_additional((&borrowed).into());

    assert!(golem.is_player_created());
}

#[test]
fn anger_timer_and_target_use_vanilla_save_keys() {
    let angry_golem = golem();
    let target_uuid = uuid::Uuid::from_int_array(&[1, 2, 3, 4]);
    assert!(target_uuid.is_some());

    angry_golem.set_persistent_anger_end_time(12_345);
    angry_golem.set_persistent_anger_target(Some(EntityReference::from_uuid(
        target_uuid.expect("uuid parses"),
    )));

    let mut nbt = NbtCompound::new();
    angry_golem.add_persistent_anger_save_data(&mut nbt);
    assert_eq!(nbt.long("anger_end_time"), Some(12_345));
    assert!(nbt.int_array("angry_at").is_some());

    let reloaded = golem();
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_borrowed_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
    reloaded.read_persistent_anger_save_data(&(&borrowed).into());

    assert_eq!(reloaded.get_persistent_anger_end_time(), 12_345);
    assert_eq!(
        reloaded
            .get_persistent_anger_target()
            .map(|reference| reference.uuid()),
        target_uuid
    );
}

#[test]
fn legacy_anger_time_key_is_still_read() {
    let golem = golem();
    let mut nbt = NbtCompound::new();
    nbt.insert("AngerTime", 40_i32);

    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_borrowed_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
    golem.read_persistent_anger_save_data(&(&borrowed).into());

    // No level is attached, so `setTimeToRemainAngry` cannot resolve a game time and
    // the mob keeps vanilla's `NO_ANGER_END_TIME` sentinel.
    assert_eq!(golem.get_persistent_anger_end_time(), NO_ANGER_END_TIME);
}

#[test]
fn can_attack_rules_match_vanilla() {
    let golem = golem();
    golem.set_player_created(true);

    // Player-created golems never attack players; nobody attacks creepers.
    assert!(!golem.allows_attacking(true, false));
    assert!(!golem.allows_attacking(false, true));
    assert!(golem.allows_attacking(false, false));

    let target = Arc::new(PigEntity::new(
        &vanilla_entities::PIG,
        2,
        DVec3::new(1.0, 0.0, 0.0),
        Weak::new(),
    ));
    assert!(Mob::can_attack(&golem, target.as_ref()));
}

#[test]
fn crackiness_thresholds_follow_crackiness_golem() {
    let golem = golem();
    let max_health = golem.get_max_health();

    golem.set_health(max_health);
    assert_eq!(golem.crackiness(), CrackinessLevel::None);

    golem.set_health(max_health * 0.6);
    assert_eq!(golem.crackiness(), CrackinessLevel::Low);

    golem.set_health(max_health * 0.4);
    assert_eq!(golem.crackiness(), CrackinessLevel::Medium);

    golem.set_health(max_health * 0.2);
    assert_eq!(golem.crackiness(), CrackinessLevel::High);
}

#[test]
fn offer_flower_sets_and_clears_the_vanilla_counter() {
    let golem = golem();
    golem.offer_flower(true);
    assert_eq!(golem.offer_flower_tick(), 400);

    golem.offer_flower(false);
    assert_eq!(golem.offer_flower_tick(), 0);
}

#[test]
fn triggered_spawn_marks_the_golem_player_created() {
    use crate::test_support::fresh_test_world;

    let world_fixture = fresh_test_world("iron_golem_finalize_spawn");
    let world = &world_fixture.world;
    let golem: SharedEntity = Arc::new(IronGolemEntity::new(
        &vanilla_entities::IRON_GOLEM,
        1,
        DVec3::ZERO,
        Arc::downgrade(world),
    ));
    let golem_entity = golem
        .downcast_ref::<IronGolemEntity>()
        .expect("golem downcast");
    assert!(!golem_entity.is_player_created());

    golem_entity.finalize_spawn(world, EntitySpawnReason::Triggered, None);
    assert!(golem_entity.is_player_created());

    let other: SharedEntity = Arc::new(IronGolemEntity::new(
        &vanilla_entities::IRON_GOLEM,
        2,
        DVec3::ZERO,
        Arc::downgrade(world),
    ));
    let other_entity = other
        .downcast_ref::<IronGolemEntity>()
        .expect("golem downcast");
    other_entity.finalize_spawn(world, EntitySpawnReason::MobSummoned, None);
    assert!(!other_entity.is_player_created());
}

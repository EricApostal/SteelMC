//! Zombie family persistence and conversion-state unit tests.

use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use std::io::Cursor;
use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::entity_data::VillagerData;
use steel_registry::{REGISTRY, RegistryExt as _, init_vanilla_registry, vanilla_entities};
use steel_utils::Identifier;

use crate::behavior::init_behaviors;
use crate::entity::entities::mobs::zombie::{ZombieEntity, ZombieVillagerEntity};
use crate::entity::{Entity, EntityFluidContact};

/// Re-parses saved NBT and hands the borrowed view to `load_additional`,
/// mirroring the entity reload path.
fn reload(nbt: &NbtCompound, load: impl FnOnce(BorrowedNbtCompoundView<'_, '_>)) {
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("reborrow failed: {error}"));
    load((&borrowed).into());
}

/// Marks the zombie's eyes as being in water, the state vanilla gates
/// `InWaterTime` on. A unit test cannot reach the fluid scan without a level.
fn submerge(zombie: &ZombieEntity) {
    zombie
        .base()
        .set_fluid_contact(EntityFluidContact::from_parts(0.5, 0.0, true, false));
}

#[test]
fn zombie_nbt_round_trip_keeps_baby_doors_and_drowning_state() {
    init_vanilla_registry();

    let zombie = ZombieEntity::new(&vanilla_entities::ZOMBIE, 1, DVec3::ZERO, Weak::new());
    zombie.set_baby(true);
    zombie.set_can_break_doors(true);
    zombie.set_conversion_time(123);
    zombie.set_in_water_time(400);
    submerge(&zombie);

    let mut nbt = NbtCompound::new();
    zombie.save_additional(&mut nbt);
    assert_eq!(nbt.byte("IsBaby"), Some(1));
    assert_eq!(nbt.byte("CanBreakDoors"), Some(1));
    assert_eq!(nbt.int("DrownedConversionTime"), Some(123));
    assert_eq!(nbt.int("InWaterTime"), Some(400));

    let loaded = ZombieEntity::new(&vanilla_entities::ZOMBIE, 2, DVec3::ZERO, Weak::new());
    reload(&nbt, |borrowed| loaded.load_additional(borrowed));

    assert!(loaded.is_baby());
    assert!(loaded.can_break_doors());
    assert!(loaded.is_under_water_converting());
    assert_eq!(loaded.conversion_time(), 123);

    // `InWaterTime` is only written while the eyes are in water, so the reloaded
    // timer is observable by saving the submerged zombie again.
    submerge(&loaded);
    let mut resaved = NbtCompound::new();
    loaded.save_additional(&mut resaved);
    assert_eq!(resaved.int("InWaterTime"), Some(400));
}

#[test]
fn zombie_villager_nbt_round_trip_keeps_villager_data_and_conversion() {
    init_vanilla_registry();
    // `start_converting` applies the strength effect, which needs the effect registry.
    init_behaviors();

    let zombie_villager = ZombieVillagerEntity::new(
        &vanilla_entities::ZOMBIE_VILLAGER,
        1,
        DVec3::ZERO,
        Weak::new(),
    );
    let profession = REGISTRY
        .villager_professions
        .id_from_key(&Identifier::vanilla_static("farmer"))
        .expect("farmer profession is registered") as i32;
    let villager_type = REGISTRY
        .villager_types
        .id_from_key(&Identifier::vanilla_static("desert"))
        .expect("desert villager type is registered") as i32;
    zombie_villager.set_villager_data(VillagerData::new(villager_type, profession, 3));
    zombie_villager.set_villager_data_finalized(true);
    zombie_villager.set_villager_xp(42);
    let starter = uuid::Uuid::from_u128(0x1234_5678);
    zombie_villager.start_converting(Some(starter), 4321);

    let mut nbt = NbtCompound::new();
    zombie_villager.save_additional(&mut nbt);
    assert_eq!(nbt.int("ConversionTime"), Some(4321));
    assert_eq!(nbt.int("Xp"), Some(42));
    assert_eq!(nbt.byte("VillagerDataFinalized"), Some(1));
    assert_eq!(
        nbt.compound("VillagerData")
            .and_then(|data| data.int("level")),
        Some(3)
    );

    let loaded = ZombieVillagerEntity::new(
        &vanilla_entities::ZOMBIE_VILLAGER,
        2,
        DVec3::ZERO,
        Weak::new(),
    );
    assert!(!loaded.is_converting());
    reload(&nbt, |borrowed| loaded.load_additional(borrowed));

    assert!(loaded.is_converting());
    assert_eq!(loaded.villager_xp(), 42);
    assert!(loaded.villager_data_finalized());
    assert_eq!(
        loaded.villager_data(),
        VillagerData::new(villager_type, profession, 3)
    );
    assert_eq!(
        loaded.conversion_starter(),
        Some(starter),
        "the curing player must survive a save/load round trip"
    );
}

#[test]
fn zombie_reinforcement_target_falls_back_to_the_attacker() {
    use std::sync::Arc;

    use steel_registry::vanilla_damage_types;

    use crate::entity::damage::DamageSource;
    use crate::entity::{Mob, SharedEntity};
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};

    init_vanilla_registry();

    let fixture = fresh_test_world("zombie_reinforcement_target");
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "ZombieAttacker", 1).build();
    let attacker: SharedEntity = player;
    let zombie = ZombieEntity::new(
        &vanilla_entities::ZOMBIE,
        2,
        DVec3::ZERO,
        Arc::downgrade(&fixture.world),
    );

    assert!(zombie.target().is_none());
    let source = DamageSource::direct(&vanilla_damage_types::PLAYER_ATTACK, Arc::clone(&attacker));

    // Vanilla `Zombie.hurtServer` uses `getTarget()`, else the source entity, so
    // reinforcements spawned from an untargeted zombie hunt the attacker.
    let selected = zombie
        .reinforcement_target(&source)
        .expect("the attacker must become the reinforcement target");
    assert!(Arc::ptr_eq(&selected, &attacker));
}

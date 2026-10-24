use crate::entity::ageable::AgeableMob as _;
use crate::entity::entities::{ExperienceOrbEntity, ItemEntity, ZombieEntity};
use crate::entity::next_entity_id;
use crate::player::{Player, ResetReason};
use crate::test_support::{
    TestEntity, TestPlayerBuilder, TestWorld, dropped_items, enchanted_item, with_enchantment,
};

use super::*;

const PIG_POSITION: DVec3 = DVec3::new(8.5, 64.0, 8.5);

fn death_loot_world(key: &'static str) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    let world_fixture = fresh_test_world(key);
    insert_ready_full_chunk(
        &world_fixture.world,
        ChunkPos::from_entity_pos(PIG_POSITION),
    );
    world_fixture
}

fn player_holding(world: &Arc<World>, weapon: ItemStack) -> Arc<Player> {
    let player =
        TestPlayerBuilder::new(Arc::clone(world), "PigKiller".to_owned(), next_entity_id()).build();
    player
        .inventory
        .lock()
        .set_item_in_hand(InteractionHand::MainHand, weapon);
    assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
    player
}

fn kill_pig(world: &Arc<World>, loot_seed: i64, source: &DamageSource) {
    let pig = Arc::new(PigEntity::new(
        &vanilla_entities::PIG,
        next_entity_id(),
        PIG_POSITION,
        Arc::downgrade(world),
    ));
    pig.set_death_loot_table_seed(loot_seed);
    world
        .try_add_entity(Arc::clone(&pig) as SharedEntity)
        .expect("pig should attach to the loaded chunk");
    pig.die(source);
}

fn dropped_item_stacks(world: &World) -> Vec<ItemStack> {
    dropped_items(world, PIG_POSITION)
        .iter()
        .filter_map(|entity| {
            entity
                .downcast_ref::<ItemEntity>()
                .map(ItemEntity::get_item)
        })
        .collect()
}

#[test]
fn melee_kill_with_fire_aspect_drops_cooked_porkchop() {
    let world_fixture = death_loot_world("death_loot_melee_fire_aspect");
    let world = &world_fixture.world;
    let player = player_holding(
        world,
        enchanted_item(
            &vanilla_items::DIAMOND_SWORD,
            Identifier::vanilla_static("fire_aspect"),
            1,
        ),
    );
    let source = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK)
        .with_causing_entity(Arc::clone(&player) as SharedEntity)
        .with_direct_entity(player as SharedEntity);

    kill_pig(world, 1, &source);

    let drops = dropped_item_stacks(world);
    assert_eq!(drops.len(), 1);
    assert!(drops[0].is(&vanilla_items::COOKED_PORKCHOP));
}

#[test]
fn arrow_kill_smelts_by_direct_attacker_and_loots_by_attacker() {
    const KILLS: i64 = 20;

    let world_fixture = death_loot_world("death_loot_arrow_attackers");
    let world = &world_fixture.world;
    let weapon = with_enchantment(
        enchanted_item(
            &vanilla_items::DIAMOND_SWORD,
            Identifier::vanilla_static("fire_aspect"),
            1,
        ),
        Identifier::vanilla_static("looting"),
        3,
    );
    let player = player_holding(world, weapon);
    let arrow = TestEntity::shared(
        next_entity_id(),
        PIG_POSITION,
        Arc::downgrade(world),
        &vanilla_entities::ARROW,
    );
    world
        .try_add_entity(Arc::clone(&arrow))
        .expect("arrow should attach to the loaded chunk");
    let source = DamageSource::environment(&vanilla_damage_types::ARROW)
        .with_causing_entity(player as SharedEntity)
        .with_direct_entity(arrow);

    for loot_seed in 1..=KILLS {
        kill_pig(world, loot_seed, &source);
    }

    let drops = dropped_item_stacks(world);
    assert_eq!(drops.len(), KILLS as usize);
    assert!(drops.iter().all(|drop| drop.is(&vanilla_items::PORKCHOP)));
    assert!(drops.iter().any(|drop| drop.count > 3));
}

fn experience_in(world: &World) -> i32 {
    world
        .get_entities_in_aabb(&WorldAabb::of_size(PIG_POSITION, 16.0, 16.0, 16.0))
        .iter()
        .filter(|entity| entity.entity_type() == &vanilla_entities::EXPERIENCE_ORB)
        .filter_map(|entity| entity.as_ref().downcast_ref::<ExperienceOrbEntity>())
        .map(|orb| orb.value() * orb.count())
        .sum()
}

fn player_kill_source(player: &Arc<Player>) -> DamageSource {
    DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK)
        .with_causing_entity(Arc::clone(player) as SharedEntity)
        .with_direct_entity(Arc::clone(player) as SharedEntity)
}

fn spawn_zombie(world: &Arc<World>, baby: bool) -> Arc<ZombieEntity> {
    let zombie = Arc::new(ZombieEntity::new(
        &vanilla_entities::ZOMBIE,
        next_entity_id(),
        PIG_POSITION,
        Arc::downgrade(world),
    ));
    zombie.set_baby(baby);
    world
        .try_add_entity(Arc::clone(&zombie) as SharedEntity)
        .expect("zombie should attach to the loaded chunk");
    zombie
}

#[test]
fn baby_animals_drop_neither_loot_nor_experience() {
    let world_fixture = death_loot_world("death_loot_baby_pig");
    let world = &world_fixture.world;
    let player = player_holding(world, ItemStack::empty());

    let pig = Arc::new(PigEntity::new(
        &vanilla_entities::PIG,
        next_entity_id(),
        PIG_POSITION,
        Arc::downgrade(world),
    ));
    pig.set_baby(true);
    pig.set_last_hurt_by_player(player.uuid(), 100);
    world
        .try_add_entity(Arc::clone(&pig) as SharedEntity)
        .expect("pig should attach to the loaded chunk");
    pig.die(&player_kill_source(&player));

    // Vanilla `LivingEntity.shouldDropLoot`/`shouldDropExperience` both reject
    // babies, which covers the whole animal family.
    assert_eq!(dropped_item_stacks(world).len(), 0);
    assert_eq!(experience_in(world), 0);
}

#[test]
fn baby_monsters_drop_loot_and_scaled_experience() {
    let world_fixture = death_loot_world("death_loot_baby_zombie");
    let world = &world_fixture.world;
    let player = player_holding(world, ItemStack::empty());
    let zombie = spawn_zombie(world, true);
    zombie.set_last_hurt_by_player(player.uuid(), 100);

    assert_eq!(zombie.xp_reward(), 5);
    zombie.die(&player_kill_source(&player));

    // Vanilla `Monster.shouldDropLoot` ignores `isBaby`, and
    // `Zombie.getBaseExperienceReward` scales the reward by 2.5.
    assert!(
        !dropped_item_stacks(world).is_empty(),
        "a baby monster still rolls its death table"
    );
    assert_eq!(experience_in(world), 12, "5 * 2.5 truncates to 12");
}

#[test]
fn vanishing_curse_equipment_is_never_dropped() {
    let world_fixture = death_loot_world("death_loot_vanishing_curse");
    let world = &world_fixture.world;
    let player = player_holding(world, ItemStack::empty());

    for _ in 0..32 {
        let pig = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            next_entity_id(),
            PIG_POSITION,
            Arc::downgrade(world),
        ));
        pig.set_guaranteed_drop(EquipmentSlot::Head);
        pig.living_base().equipment().lock().set(
            EquipmentSlot::Head,
            enchanted_item(
                &vanilla_items::DIAMOND_HELMET,
                Identifier::vanilla_static("vanishing_curse"),
                1,
            ),
        );
        pig.set_last_hurt_by_player(player.uuid(), 100);
        world
            .try_add_entity(Arc::clone(&pig) as SharedEntity)
            .expect("pig should attach to the loaded chunk");
        pig.die(&player_kill_source(&player));
    }

    // Vanilla `Mob.dropCustomDeathLoot` skips stacks with
    // `minecraft:prevent_equipment_drop` before it even rolls the chance.
    assert!(
        dropped_item_stacks(world)
            .iter()
            .all(|drop| !drop.is(&vanilla_items::DIAMOND_HELMET))
    );
}

#[test]
fn zero_drop_chance_equipment_is_never_dropped() {
    let world_fixture = death_loot_world("death_loot_zero_drop_chance");
    let world = &world_fixture.world;
    let player = player_holding(world, ItemStack::empty());

    for _ in 0..32 {
        let pig = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            next_entity_id(),
            PIG_POSITION,
            Arc::downgrade(world),
        ));
        pig.set_equipment_drop_chance(EquipmentSlot::Head, 0.0);
        pig.living_base().equipment().lock().set(
            EquipmentSlot::Head,
            ItemStack::new(&vanilla_items::DIAMOND_HELMET),
        );
        pig.set_last_hurt_by_player(player.uuid(), 100);
        world
            .try_add_entity(Arc::clone(&pig) as SharedEntity)
            .expect("pig should attach to the loaded chunk");
        pig.die(&player_kill_source(&player));
    }

    // Vanilla skips zero-chance slots before consulting any enchantment, so a
    // base chance of 0 can never be raised by `minecraft:equipment_drops`.
    assert!(
        dropped_item_stacks(world)
            .iter()
            .all(|drop| !drop.is(&vanilla_items::DIAMOND_HELMET))
    );
}

/// Vanilla `Mob.dropCustomDeathLoot`: a *preserved* slot (`DropChances.isPreserved`, i.e.
/// `setGuaranteedDrop`) skips the wear step and drops the stack exactly as worn.
#[test]
fn guaranteed_equipment_drops_unworn_on_a_player_kill() {
    let world_fixture = death_loot_world("death_loot_guaranteed_equipment");
    let world = &world_fixture.world;
    let player = player_holding(world, ItemStack::empty());

    let pig = Arc::new(PigEntity::new(
        &vanilla_entities::PIG,
        next_entity_id(),
        PIG_POSITION,
        Arc::downgrade(world),
    ));
    pig.set_guaranteed_drop(EquipmentSlot::Head);
    pig.living_base().equipment().lock().set(
        EquipmentSlot::Head,
        ItemStack::new(&vanilla_items::DIAMOND_HELMET),
    );
    pig.set_last_hurt_by_player(player.uuid(), 100);
    world
        .try_add_entity(Arc::clone(&pig) as SharedEntity)
        .expect("pig should attach to the loaded chunk");
    pig.die(&player_kill_source(&player));

    let helmet = dropped_item_stacks(world)
        .into_iter()
        .find(|drop| drop.is(&vanilla_items::DIAMOND_HELMET))
        .expect("a guaranteed drop is dropped");

    assert_eq!(
        helmet.get_damage_value(),
        0,
        "`!isPreserved` gates vanilla's wear, so a guaranteed drop keeps its damage"
    );
}

/// Vanilla `Mob.dropCustomDeathLoot`: a non-preserved slot that passes its drop roll is
/// worn by `maxDamage - rand(0..=rand(0..max(maxDamage - 3, 1)))`.
#[test]
fn worn_equipment_drops_with_vanilla_wear() {
    let world_fixture = death_loot_world("death_loot_worn_equipment");
    let world = &world_fixture.world;
    let player = player_holding(world, ItemStack::empty());

    let pig = Arc::new(PigEntity::new(
        &vanilla_entities::PIG,
        next_entity_id(),
        PIG_POSITION,
        Arc::downgrade(world),
    ));
    // A base chance of 1.0 is not preserved, so the wear step runs.
    pig.set_equipment_drop_chance(EquipmentSlot::Head, 1.0);
    pig.living_base().equipment().lock().set(
        EquipmentSlot::Head,
        ItemStack::new(&vanilla_items::DIAMOND_HELMET),
    );
    pig.set_last_hurt_by_player(player.uuid(), 100);
    world
        .try_add_entity(Arc::clone(&pig) as SharedEntity)
        .expect("pig should attach to the loaded chunk");
    pig.die(&player_kill_source(&player));

    let helmet = dropped_item_stacks(world)
        .into_iter()
        .find(|drop| drop.is(&vanilla_items::DIAMOND_HELMET))
        .expect("a chance-1.0 drop is dropped");

    // `lost` is at most `maxDamage - 4` (the inner bound minus one), so the dropped
    // helmet is at least four points damaged and never more than fully worn.
    let max_damage = helmet.get_max_damage();
    let least_wear = max_damage - (max_damage - 3).max(1) + 1;
    assert!(
        least_wear <= helmet.get_damage_value(),
        "vanilla wears the dropped stack: at least {least_wear} damage, got {}",
        helmet.get_damage_value()
    );
    assert!(helmet.get_damage_value() <= max_damage);
}

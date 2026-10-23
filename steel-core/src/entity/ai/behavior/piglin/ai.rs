//! Vanilla `PiglinAi`: the statics shared by the piglin behaviors, the activity
//! packages and the entity hooks.
//!
//! Vanilla reaches the brain through `body.getBrain()` everywhere. Steel holds the
//! brain lock for the whole brain tick, so the helpers that read or write memories
//! take the [`MemoryStore`] explicitly (from [`BehaviorContext::memories_mut`] on
//! the behavior side, from the locked [`Brain`] on the entity side). Helpers that
//! run only outside the brain tick (item pickup, interaction, hurt handling) lock
//! the brain themselves and are documented as such.

use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::CTakeItemEntity;
use steel_registry::data_components::vanilla_components::SwingAnimation;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::{EntityEquipmentRef, EntityRef, EntityRefFlags, LootContext};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_game_rules::UNIVERSAL_ANGER;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{sound_events, vanilla_entities, vanilla_items, vanilla_loot_tables};
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::value_providers::UniformIntProvider;
use steel_utils::{ChunkPos, Downcast as _};

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::BehaviorContext;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStore};
use crate::entity::ai::sensing::villager::follow_range;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::entities::ItemEntity;
use crate::entity::entities::mobs::hostile::hoglin::HoglinEntity;
use crate::entity::entities::mobs::hostile::piglin::{PiglinBruteEntity, PiglinEntity};
use crate::entity::{Entity, LivingEntity, Mob, RemovalReason, SharedEntity};
use crate::player::Player;
use crate::world::World;

/// Vanilla `PiglinAi.ANGER_DURATION`.
pub(crate) const ANGER_DURATION: i64 = 600;
/// Vanilla `PiglinAi.ADMIRE_DURATION`.
pub(crate) const ADMIRE_DURATION: i64 = 119;
/// Vanilla `PiglinAi.MAX_DISTANCE_TO_WALK_TO_ITEM`.
pub(crate) const MAX_DISTANCE_TO_WALK_TO_ITEM: i32 = 9;
/// Vanilla `PiglinAi.MAX_TIME_TO_WALK_TO_ITEM`.
pub(crate) const MAX_TIME_TO_WALK_TO_ITEM: i32 = 200;
/// Vanilla `PiglinAi.HOW_LONG_TIME_TO_DISABLE_ADMIRE_WALKING_IF_CANT_REACH_ITEM`.
pub(crate) const HOW_LONG_TIME_TO_DISABLE_ADMIRE_WALKING_IF_CANT_REACH_ITEM: i32 = 200;
/// Vanilla `PiglinAi.CELEBRATION_TIME`.
pub(crate) const CELEBRATION_TIME: i32 = 300;
/// Vanilla `PiglinAi.MAX_TIME_BETWEEN_HUNTS` (seconds).
pub(crate) const MAX_TIME_BETWEEN_HUNTS: i32 = 120;
/// Vanilla `PiglinAi.TIME_BETWEEN_HUNTS`, `TimeUtil.rangeOfSeconds(30, 120)`.
pub(crate) const TIME_BETWEEN_HUNTS: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30 * 20,
    max_inclusive: 120 * 20,
};
/// Vanilla `PiglinAi.BABY_FLEE_DURATION_AFTER_GETTING_HIT`.
pub(crate) const BABY_FLEE_DURATION_AFTER_GETTING_HIT: i64 = 100;
/// Vanilla `PiglinAi.HIT_BY_PLAYER_MEMORY_TIMEOUT`.
pub(crate) const HIT_BY_PLAYER_MEMORY_TIMEOUT: i64 = 400;
/// Vanilla `PiglinAi.MAX_WALK_DISTANCE_TO_START_RIDING`.
pub(crate) const MAX_WALK_DISTANCE_TO_START_RIDING: i32 = 8;
/// Vanilla `PiglinAi.RIDE_START_INTERVAL`, `TimeUtil.rangeOfSeconds(10, 40)`.
pub(crate) const RIDE_START_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 10 * 20,
    max_inclusive: 40 * 20,
};
/// Vanilla `PiglinAi.RIDE_DURATION`, `TimeUtil.rangeOfSeconds(10, 30)`.
pub(crate) const RIDE_DURATION: UniformIntProvider = UniformIntProvider {
    min_inclusive: 10 * 20,
    max_inclusive: 30 * 20,
};
/// Vanilla `PiglinAi.RETREAT_DURATION`, `TimeUtil.rangeOfSeconds(5, 20)`.
pub(crate) const RETREAT_DURATION: UniformIntProvider = UniformIntProvider {
    min_inclusive: 5 * 20,
    max_inclusive: 20 * 20,
};
/// Vanilla `PiglinAi.MELEE_ATTACK_COOLDOWN`.
pub(crate) const MELEE_ATTACK_COOLDOWN: i32 = 20;
/// Vanilla `PiglinAi.EAT_COOLDOWN`.
pub(crate) const EAT_COOLDOWN: i64 = 200;
/// Vanilla `PiglinAi.DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING`.
pub(crate) const DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING: i32 = 12;
/// Vanilla `PiglinAi.MAX_LOOK_DIST`.
pub(crate) const MAX_LOOK_DIST: f32 = 8.0;
/// Vanilla `PiglinAi.MAX_LOOK_DIST_FOR_PLAYER_HOLDING_LOVED_ITEM`.
pub(crate) const MAX_LOOK_DIST_FOR_PLAYER_HOLDING_LOVED_ITEM: f32 = 14.0;
/// Vanilla `PiglinAi.INTERACTION_RANGE`.
pub(crate) const INTERACTION_RANGE: i32 = 8;
/// Vanilla `PiglinAi.MIN_DESIRED_DIST_FROM_TARGET_WHEN_HOLDING_CROSSBOW`.
pub(crate) const MIN_DESIRED_DIST_FROM_TARGET_WHEN_HOLDING_CROSSBOW: i32 = 5;
/// Vanilla `PiglinAi.SPEED_WHEN_STRAFING_BACK_FROM_TARGET`.
pub(crate) const SPEED_WHEN_STRAFING_BACK_FROM_TARGET: f32 = 0.75;
/// Vanilla `PiglinAi.DESIRED_DISTANCE_FROM_ZOMBIFIED`.
pub(crate) const DESIRED_DISTANCE_FROM_ZOMBIFIED: f64 = 6.0;
/// Vanilla `PiglinAi.AVOID_ZOMBIFIED_DURATION`, `TimeUtil.rangeOfSeconds(5, 7)`.
pub(crate) const AVOID_ZOMBIFIED_DURATION: UniformIntProvider = UniformIntProvider {
    min_inclusive: 5 * 20,
    max_inclusive: 7 * 20,
};
/// Vanilla `PiglinAi.BABY_AVOID_NEMESIS_DURATION`, `TimeUtil.rangeOfSeconds(5, 7)`.
pub(crate) const BABY_AVOID_NEMESIS_DURATION: UniformIntProvider = UniformIntProvider {
    min_inclusive: 5 * 20,
    max_inclusive: 7 * 20,
};
/// Vanilla `PiglinAi.PROBABILITY_OF_CELEBRATION_DANCE`.
pub(crate) const PROBABILITY_OF_CELEBRATION_DANCE: f32 = 0.1;
/// Vanilla `PiglinAi.SPEED_MULTIPLIER_WHEN_AVOIDING`.
pub(crate) const SPEED_MULTIPLIER_WHEN_AVOIDING: f32 = 1.0;
/// Vanilla `PiglinAi.SPEED_MULTIPLIER_WHEN_RETREATING`.
pub(crate) const SPEED_MULTIPLIER_WHEN_RETREATING: f32 = 1.0;
/// Vanilla `PiglinAi.SPEED_MULTIPLIER_WHEN_MOUNTING`.
pub(crate) const SPEED_MULTIPLIER_WHEN_MOUNTING: f32 = 0.8;
/// Vanilla `PiglinAi.SPEED_MULTIPLIER_WHEN_GOING_TO_WANTED_ITEM`.
pub(crate) const SPEED_MULTIPLIER_WHEN_GOING_TO_WANTED_ITEM: f32 = 1.0;
/// Vanilla `PiglinAi.SPEED_MULTIPLIER_WHEN_GOING_TO_CELEBRATE_LOCATION`.
pub(crate) const SPEED_MULTIPLIER_WHEN_GOING_TO_CELEBRATE_LOCATION: f32 = 1.0;
/// Vanilla `PiglinAi.SPEED_MULTIPLIER_WHEN_DANCING`.
pub(crate) const SPEED_MULTIPLIER_WHEN_DANCING: f32 = 0.6;
/// Vanilla `PiglinAi.SPEED_MULTIPLIER_WHEN_IDLING`.
pub(crate) const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.6;

/// Vanilla `PiglinAi.isLovedItem`: `stack.is(ItemTags.PIGLIN_LOVED)`.
#[must_use]
pub(crate) fn is_loved_item(item_stack: &ItemStack) -> bool {
    item_stack.item().has_tag(&ItemTag::PIGLIN_LOVED)
}

/// Vanilla `PiglinAi.isBarterCurrency`: `stack.is(BARTERING_ITEM)`.
#[must_use]
pub(crate) fn is_barter_currency(item_stack: &ItemStack) -> bool {
    item_stack.is(&vanilla_items::GOLD_INGOT)
}

/// Vanilla `PiglinAi.isFood`: `stack.is(ItemTags.PIGLIN_FOOD)`.
#[must_use]
pub(crate) fn is_food(item_stack: &ItemStack) -> bool {
    item_stack.item().has_tag(&ItemTag::PIGLIN_FOOD)
}

/// Vanilla `AgeableMob.isAdult`.
#[must_use]
pub(crate) fn is_adult(body: &dyn Entity) -> bool {
    body.as_living_entity()
        .is_some_and(|living| !living.is_baby())
}

/// Vanilla `Piglin.canHunt`; `PiglinBrute.canHunt` is always `false`.
#[must_use]
pub(crate) fn can_hunt(body: &dyn Entity) -> bool {
    body.downcast_ref::<PiglinEntity>()
        .is_some_and(|piglin| !piglin.cannot_hunt())
}

/// Vanilla `PiglinAi.isAdmiringItem`.
#[must_use]
pub(crate) fn is_admiring_item(memories: &MemoryStore) -> bool {
    memories.has_value(&mem::ADMIRING_ITEM)
}

/// Vanilla `PiglinAi.isAdmiringDisabled`.
#[must_use]
pub(crate) fn is_admiring_disabled(memories: &MemoryStore) -> bool {
    memories.has_value(&mem::ADMIRING_DISABLED)
}

/// Vanilla `PiglinAi.hasEatenRecently`.
#[must_use]
pub(crate) fn has_eaten_recently(memories: &MemoryStore) -> bool {
    memories.has_value(&mem::ATE_RECENTLY)
}

/// Vanilla `PiglinAi.isNearRepellent`.
#[must_use]
pub(crate) fn is_near_repellent(memories: &MemoryStore) -> bool {
    memories.has_value(&mem::NEAREST_REPELLENT)
}

/// Vanilla `PiglinAi.seesPlayerHoldingLovedItem`.
#[must_use]
pub(crate) fn sees_player_holding_loved_item(memories: &MemoryStore) -> bool {
    memories.has_value(&mem::NEAREST_PLAYER_HOLDING_WANTED_ITEM)
}

/// Vanilla `PiglinAi.doesntSeeAnyPlayerHoldingLovedItem`.
#[must_use]
pub(crate) fn doesnt_see_any_player_holding_loved_item(memories: &MemoryStore) -> bool {
    !sees_player_holding_loved_item(memories)
}

/// Vanilla `PiglinAi.wasHurtRecently`: `brain.hasMemoryValue(HURT_BY)`.
#[must_use]
pub(crate) fn was_hurt_recently(memories: &MemoryStore) -> bool {
    memories.has_value(&mem::HURT_BY)
}

/// Vanilla `PiglinAi.isHoldingItemInOffHand`.
#[must_use]
pub(crate) fn is_holding_item_in_off_hand(body: &dyn Entity) -> bool {
    body.as_living_entity()
        .is_some_and(|living| !living.get_item_in_hand(InteractionHand::OffHand).is_empty())
}

/// Vanilla `PiglinAi.isNotHoldingLovedItemInOffHand`.
#[must_use]
pub(crate) fn is_not_holding_loved_item_in_off_hand(body: &dyn Entity) -> bool {
    body.as_living_entity().is_some_and(|living| {
        let stack = living.get_item_in_hand(InteractionHand::OffHand);
        stack.is_empty() || !is_loved_item(&stack)
    })
}

/// Vanilla `PiglinAi.isNearZombified`: the remembered zombified mob is within 6 blocks.
#[must_use]
pub(crate) fn is_near_zombified(memories: &MemoryStore, body: &SharedEntity) -> bool {
    memories
        .get(&mem::NEAREST_VISIBLE_ZOMBIFIED)
        .is_some_and(|zombified| {
            body.position().distance_squared(zombified.position())
                < DESIRED_DISTANCE_FROM_ZOMBIFIED * DESIRED_DISTANCE_FROM_ZOMBIFIED
        })
}

/// Vanilla `PiglinAi.isIdle`: `brain.isActive(Activity.IDLE)`.
///
/// Runs outside the brain tick only (ambient sound), so it locks the brain itself.
#[must_use]
pub(crate) fn is_idle(body: &dyn Entity) -> bool {
    body.as_mob()
        .and_then(Mob::brain)
        .is_some_and(|brain| brain.lock().is_active(Activity::Idle))
}

/// Vanilla `PiglinAi.hasCrossbow`: `body.isHolding(Items.CROSSBOW)`.
#[must_use]
pub(crate) fn has_crossbow(body: &SharedEntity) -> bool {
    let mut holding = false;
    body.with_weapon_item(&mut |item| {
        holding = item.is_some_and(|stack| stack.is(&vanilla_items::CROSSBOW));
    });
    holding
}

/// Vanilla `PiglinAi.isEntityAttackableIgnoringLineOfSight`.
#[must_use]
pub(crate) fn is_entity_attackable_ignoring_line_of_sight(
    level: &World,
    body: &dyn Entity,
    target: &SharedEntity,
) -> bool {
    let (Some(body_living), Some(target_living)) =
        (body.as_living_entity(), target.as_living_entity())
    else {
        return false;
    };
    let Some(range) = follow_range(body) else {
        return false;
    };
    TargetingConditions::for_combat()
        .range(range)
        .ignore_line_of_sight()
        .test(level, Some(body_living), target_living)
}

/// Vanilla `PiglinAi.stopWalking`: erase `WALK_TARGET` and stop navigation.
///
/// Runs outside the brain tick only.
pub(crate) fn stop_walking(body: &dyn Entity) {
    if let Some(brain) = body.as_mob().and_then(Mob::brain) {
        brain.lock().erase_memory(&mem::WALK_TARGET);
    }
    if let Some(pathfinder) = body.as_pathfinder_mob() {
        pathfinder.mob_base().navigation().lock().stop();
    }
}

/// Vanilla `PiglinAi.admireGoldItem`: `setMemoryWithExpiry(ADMIRING_ITEM, true, 119)`.
///
/// Runs outside the brain tick only.
pub(crate) fn admire_gold_item(body: &dyn Entity) {
    if let Some(brain) = body.as_mob().and_then(Mob::brain) {
        brain
            .lock()
            .set_memory_with_expiry(&mem::ADMIRING_ITEM, true, ADMIRE_DURATION);
    }
}

/// Vanilla `PiglinAi.eat`: `setMemoryWithExpiry(ATE_RECENTLY, true, 200)`.
///
/// Runs outside the brain tick only.
pub(crate) fn eat(body: &dyn Entity) {
    if let Some(brain) = body.as_mob().and_then(Mob::brain) {
        brain
            .lock()
            .set_memory_with_expiry(&mem::ATE_RECENTLY, true, EAT_COOLDOWN);
    }
}

/// Vanilla `PiglinAi.holdInOffHand`: drop the previous off-hand stack, then hold `item_stack`.
pub(crate) fn hold_in_off_hand(body: &dyn Entity, item_stack: ItemStack) {
    if is_holding_item_in_off_hand(body)
        && let Some(previous) = body
            .as_living_entity()
            .map(|living| living.get_item_in_hand(InteractionHand::OffHand))
    {
        body.spawn_at_location(previous, 0.0);
    }

    if let Some(piglin) = body.downcast_ref::<PiglinEntity>() {
        piglin.hold_in_off_hand(item_stack);
    } else if let Some(living) = body.as_living_entity() {
        living.set_item_in_hand(InteractionHand::OffHand, item_stack);
    }
}

/// Vanilla `PiglinAi.putInInventory`: store what fits, throw the remainder.
pub(crate) fn put_in_inventory(world: &World, body: &dyn Entity, item_stack: ItemStack) {
    let remainder = body
        .downcast_ref::<PiglinEntity>()
        .map_or(item_stack, |piglin| piglin.add_to_inventory(item_stack));
    throw_items_toward_random_pos(world, body, vec![remainder]);
}

/// Vanilla `PiglinAi.throwItems`: toward the nearest visible player, else a random nearby pos.
pub(crate) fn throw_items(
    world: &World,
    body: &dyn Entity,
    item_stacks: Vec<ItemStack>,
    nearest_visible_player: Option<DVec3>,
) {
    match nearest_visible_player {
        Some(position) => throw_items_toward_pos(body, item_stacks, position),
        None => throw_items_toward_random_pos(world, body, item_stacks),
    }
}

/// Vanilla `PiglinAi.throwItemsTowardRandomPos`.
pub(crate) fn throw_items_toward_random_pos(
    world: &World,
    body: &dyn Entity,
    item_stacks: Vec<ItemStack>,
) {
    let target_pos = get_random_nearby_pos(world, body);
    throw_items_toward_pos(body, item_stacks, target_pos);
}

/// Vanilla `PiglinAi.throwItemsTowardPos`: swing the off hand, then throw every stack.
pub(crate) fn throw_items_toward_pos(
    body: &dyn Entity,
    item_stacks: Vec<ItemStack>,
    target_pos: DVec3,
) {
    if item_stacks.is_empty() {
        return;
    }

    if let Some(living) = body.as_living_entity() {
        living.swing(InteractionHand::OffHand, SwingAnimation::DEFAULT, true);
    }

    for item_stack in item_stacks {
        throw_item(body, item_stack, target_pos + DVec3::new(0.0, 1.0, 0.0));
    }
}

/// Vanilla `BehaviorUtils.throwItem`, accepting a borrowed body.
///
/// Steel's shared `utils::throw_item` takes a `SharedEntity`; the piglin helpers are
/// reachable from `&self`-only entity methods, so the same body is re-implemented here
/// against `&dyn Entity`.
fn throw_item(thrower: &dyn Entity, item: ItemStack, target_pos: DVec3) {
    /// Vanilla `BehaviorUtils.throwItem`'s `throwVelocity` (`0.3F` on every axis).
    const THROW_VELOCITY: f64 = 0.3;
    /// Vanilla `BehaviorUtils.throwItem`'s `handYDistanceFromEye`.
    const HAND_Y_DISTANCE_FROM_EYE: f64 = 0.3;

    if item.is_empty() {
        return;
    }
    let Some(level) = thrower.level() else {
        return;
    };

    let mut velocity = target_pos - thrower.position();
    velocity = if velocity.length_squared() < 1.0e-5 {
        DVec3::ZERO
    } else {
        velocity.normalize() * THROW_VELOCITY
    };

    let position = DVec3::new(
        thrower.position().x,
        thrower.get_eye_y() - HAND_Y_DISTANCE_FROM_EYE,
        thrower.position().z,
    );

    let Some(item_entity) = level.spawn_item_with_velocity(position, item, velocity) else {
        return;
    };
    item_entity.set_thrower(thrower.uuid());
    item_entity.set_default_pickup_delay();
}

/// Vanilla `PiglinAi.getRandomNearbyPos` (`LandRandomPos.getPos(body, 4, 2)`).
fn get_random_nearby_pos(_world: &World, body: &dyn Entity) -> DVec3 {
    body.as_pathfinder_mob()
        .and_then(|pathfinder| {
            crate::entity::ai::behavior::random_pos::land_random_pos(pathfinder, 4, 2)
        })
        .unwrap_or_else(|| body.position())
}

/// Vanilla `PiglinAi.getBarterResponseItems`: rolls the `piglin_bartering` loot table
/// with this entity as `this_entity`.
pub(crate) fn get_barter_response_items(body: &dyn Entity) -> Vec<ItemStack> {
    let equipment: [ItemStack; EquipmentSlot::ALL.len()] = std::array::from_fn(|index| {
        let mut stack = ItemStack::empty();
        if let Some(living) = body.as_living_entity() {
            living.with_equipment_slot(EquipmentSlot::ALL[index], &mut |slot| {
                stack = slot.clone();
            });
        }
        stack
    });

    let entity_ref = EntityRef {
        entity_type: Some(&body.entity_type().key),
        flags: EntityRefFlags {
            is_on_fire: body.is_on_fire(),
            is_sneaking: body.is_crouching(),
            is_sprinting: body
                .as_living_entity()
                .is_some_and(LivingEntity::is_sprinting),
            is_swimming: body.is_swimming(),
            is_baby: body.as_living_entity().is_some_and(LivingEntity::is_baby),
        },
        equipment: Some(EntityEquipmentRef::new(&equipment)),
        custom_name: None,
        sheep_color: None,
        sheep_sheared: None,
        chicken_variant: None,
    };

    let position = body.position();
    let mut rng = LegacyRandom::from_seed(rand::random());
    let mut context = LootContext::new(&mut rng)
        .with_origin(position.x, position.y, position.z)
        .with_this_entity(entity_ref);

    vanilla_loot_tables::GAMEPLAY_PIGLIN_BARTERING.get_random_items(&mut context)
}

/// Vanilla `PiglinAi.pickUpItem`.
///
/// Runs outside the brain tick (the looting loop), so the brain is locked here.
pub(crate) fn pick_up_item(world: &Arc<World>, body: &dyn Entity, item_entity: &ItemEntity) {
    stop_walking(body);

    let item_stack = item_entity.get_item();
    let taken = if item_stack.is(&vanilla_items::GOLD_NUGGET) {
        let count = item_stack.count();
        take_from_item_entity(item_entity, count)
    } else {
        take_from_item_entity(item_entity, 1)
    };

    if taken.is_empty() {
        return;
    }

    world.broadcast_to_nearby(
        ChunkPos::from_entity_pos(item_entity.position()),
        CTakeItemEntity::new(item_entity.id(), body.id(), taken.count()),
        None,
    );

    if is_loved_item(&taken) {
        if let Some(brain) = body.as_mob().and_then(Mob::brain) {
            brain
                .lock()
                .erase_memory(&mem::TIME_TRYING_TO_REACH_ADMIRE_ITEM);
        }
        hold_in_off_hand(body, taken);
        admire_gold_item(body);
    } else if is_food(&taken) && !has_eaten_recently_locked(body) {
        eat(body);
    } else {
        let equipped = body
            .as_mob()
            .is_some_and(|mob| !mob.equip_item_if_possible(taken.clone()).is_empty());
        if !equipped {
            put_in_inventory(world, body, taken);
        }
    }
}

/// Reads `ATE_RECENTLY` outside the brain tick.
fn has_eaten_recently_locked(body: &dyn Entity) -> bool {
    body.as_mob()
        .and_then(Mob::brain)
        .is_some_and(|brain| has_eaten_recently(brain.lock().memories()))
}

/// Vanilla `PiglinAi.removeOneItemFromItemEntity` (or the whole stack for gold nuggets).
fn take_from_item_entity(item_entity: &ItemEntity, amount: i32) -> ItemStack {
    let mut source = item_entity.get_item();
    let removed = source.split(amount);
    if source.is_empty() {
        item_entity.set_removed(RemovalReason::Discarded);
    } else {
        item_entity.set_item(source);
    }
    removed
}

/// Vanilla `PiglinAi.stopHoldingOffHandItem`.
///
/// `throw_target` is the nearest visible player position the barter response is
/// thrown toward, or `None` to throw it toward a random nearby position.
pub(crate) fn stop_holding_off_hand_item(
    world: &World,
    body: &dyn Entity,
    bartering_enabled: bool,
    throw_target: Option<DVec3>,
) {
    let Some(living) = body.as_living_entity() else {
        return;
    };
    let item_stack = living.get_item_in_hand(InteractionHand::OffHand);
    living.set_item_in_hand(InteractionHand::OffHand, ItemStack::empty());

    if is_adult(body) {
        let barter_currency = is_barter_currency(&item_stack);
        if bartering_enabled && barter_currency {
            throw_items(world, body, get_barter_response_items(body), throw_target);
        } else if !barter_currency {
            let equipped = body
                .as_mob()
                .is_some_and(|mob| !mob.equip_item_if_possible(item_stack.clone()).is_empty());
            if !equipped {
                put_in_inventory(world, body, item_stack);
            }
        }
    } else {
        let equipped = body
            .as_mob()
            .is_some_and(|mob| !mob.equip_item_if_possible(item_stack.clone()).is_empty());
        if !equipped {
            let main_hand = living.get_item_in_hand(InteractionHand::MainHand);
            if is_loved_item(&main_hand) {
                put_in_inventory(world, body, main_hand);
            } else {
                throw_items_toward_random_pos(world, body, vec![main_hand]);
            }
            if let Some(piglin) = body.downcast_ref::<PiglinEntity>() {
                piglin.hold_in_main_hand(item_stack);
            } else {
                living.set_item_in_hand(InteractionHand::MainHand, item_stack);
            }
        }
    }
}

/// Vanilla `PiglinAi.cancelAdmiring`: drop the admired off-hand stack.
pub(crate) fn cancel_admiring(body: &dyn Entity) {
    let admiring = body
        .as_mob()
        .and_then(Mob::brain)
        .is_some_and(|brain| is_admiring_item(brain.lock().memories()));
    if !admiring {
        return;
    }
    let Some(living) = body.as_living_entity() else {
        return;
    };
    let off_hand = living.get_item_in_hand(InteractionHand::OffHand);
    if off_hand.is_empty() {
        return;
    }
    body.spawn_at_location(off_hand, 0.0);
    living.set_item_in_hand(InteractionHand::OffHand, ItemStack::empty());
}

/// Vanilla `PiglinAi.wantsToDance`.
#[must_use]
pub(crate) fn wants_to_dance(body: &dyn Entity, killed_target: &dyn Entity) -> bool {
    if killed_target.entity_type().key != vanilla_entities::HOGLIN.key {
        return false;
    }
    let game_time = body.level().map_or(0, |world| world.game_time());
    let mut random = LegacyRandom::from_seed(game_time as u64);
    random.next_f32() < PROBABILITY_OF_CELEBRATION_DANCE
}

/// Vanilla `PiglinAi.wantsToPickup`.
///
/// The two brain-memory gates (`ADMIRING_DISABLED` together with an `ATTACK_TARGET`)
/// are mirrored into a plain flag by the entity's activity update, because the item
/// sensor calls this hook while the brain lock is held.
#[must_use]
pub(crate) fn wants_to_pickup(body: &dyn Entity, item_stack: &ItemStack) -> bool {
    let Some(living) = body.as_living_entity() else {
        return false;
    };

    if living.is_baby()
        && item_stack
            .item()
            .has_tag(&ItemTag::IGNORED_BY_PIGLIN_BABIES)
    {
        return false;
    }
    if item_stack.item().has_tag(&ItemTag::PIGLIN_REPELLENTS) {
        return false;
    }
    if body
        .downcast_ref::<PiglinEntity>()
        .is_some_and(PiglinEntity::is_pickup_blocked)
    {
        return false;
    }

    if is_barter_currency(item_stack) {
        return is_not_holding_loved_item_in_off_hand(body);
    }

    let has_space = body
        .downcast_ref::<PiglinEntity>()
        .is_some_and(|piglin| piglin.can_add_to_inventory(item_stack));
    if item_stack.is(&vanilla_items::GOLD_NUGGET) {
        return has_space;
    }
    if is_food(item_stack) {
        return !has_eaten_recently_locked(body) && has_space;
    }
    if !is_loved_item(item_stack) {
        body.as_mob().is_some_and(|mob| {
            let slot = mob.get_equipment_slot_for_item(item_stack);
            let current = mob.equipment_in_slot(slot);
            mob.can_replace_current_item(item_stack, &current, slot)
        })
    } else {
        is_not_holding_loved_item_in_off_hand(body) && has_space
    }
}

/// Vanilla `PiglinAi.wantsToStopRiding`.
#[must_use]
pub(crate) fn wants_to_stop_riding(body: &dyn Entity, entity_being_ridden: &SharedEntity) -> bool {
    if entity_being_ridden.as_mob().is_none() {
        return false;
    }
    let ridden_is_baby = entity_being_ridden
        .as_living_entity()
        .is_some_and(LivingEntity::is_baby);

    !ridden_is_baby
        || !entity_being_ridden.is_alive()
        || was_hurt_recently_locked(body)
        || was_hurt_recently_locked(entity_being_ridden)
        || (entity_being_ridden.downcast_ref::<PiglinEntity>().is_some()
            && entity_being_ridden.vehicle().is_none())
}

/// Reads `HURT_BY` outside the brain tick.
fn was_hurt_recently_locked(body: &dyn Entity) -> bool {
    body.as_mob()
        .and_then(Mob::brain)
        .is_some_and(|brain| was_hurt_recently(brain.lock().memories()))
}

/// Vanilla `PiglinAi.isNearestValidAttackTarget`.
#[must_use]
pub(crate) fn is_nearest_valid_attack_target(
    ctx: &BehaviorContext<'_>,
    target: &SharedEntity,
) -> bool {
    find_nearest_valid_attack_target(ctx, ctx.body)
        .is_some_and(|nearest| nearest.id() == target.id())
}

/// Vanilla `PiglinAi.findNearestValidAttackTarget`.
pub(crate) fn find_nearest_valid_attack_target(
    ctx: &BehaviorContext<'_>,
    body: &SharedEntity,
) -> Option<SharedEntity> {
    if is_near_zombified(ctx.memories(), body) {
        return None;
    }

    if let Some(angry_at) = angry_at(ctx)
        && is_entity_attackable_ignoring_line_of_sight(ctx.level, body, &angry_at)
    {
        return Some(angry_at);
    }

    if ctx.has_memory_value(&mem::UNIVERSAL_ANGER)
        && let Some(player) = ctx.get_memory(&mem::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
    {
        return Some(player);
    }

    if let Some(nemesis) = ctx.get_memory(&mem::NEAREST_VISIBLE_NEMESIS) {
        return Some(nemesis);
    }

    ctx.get_memory(&mem::NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD)
        .filter(|player| is_entity_attackable_ignoring_line_of_sight(ctx.level, body, player))
}

/// Vanilla `PiglinBruteAi.findNearestValidAttackTarget`.
pub(crate) fn find_nearest_valid_attack_target_brute(
    ctx: &BehaviorContext<'_>,
    body: &SharedEntity,
) -> Option<SharedEntity> {
    if let Some(angry_at) = angry_at(ctx)
        && is_entity_attackable_ignoring_line_of_sight(ctx.level, body, &angry_at)
    {
        return Some(angry_at);
    }

    if let Some(player) = ctx.get_memory(&mem::NEAREST_VISIBLE_ATTACKABLE_PLAYER) {
        return Some(player);
    }

    ctx.get_memory(&mem::NEAREST_VISIBLE_NEMESIS)
}

/// Vanilla `BehaviorUtils.getLivingEntityFromUUIDMemory(ANGRY_AT)`.
fn angry_at(ctx: &BehaviorContext<'_>) -> Option<SharedEntity> {
    let uuid = ctx.get_memory(&mem::ANGRY_AT)?;
    let target = ctx.level.get_entity_by_uuid(&uuid)?;
    target.as_living_entity()?;
    Some(target)
}

/// Vanilla `PiglinAi.canAdmire`.
///
/// Runs outside the brain tick (player interaction), so it locks the brain itself.
#[must_use]
pub(crate) fn can_admire(body: &dyn Entity, item_stack: &ItemStack) -> bool {
    let Some(brain) = body.as_mob().and_then(Mob::brain) else {
        return false;
    };
    let brain = brain.lock();
    !is_admiring_disabled(brain.memories())
        && !is_admiring_item(brain.memories())
        && is_adult(body)
        && is_barter_currency(item_stack)
}

/// Vanilla `PiglinAi.mobInteract`: an adult piglin admires a gold ingot held out to it.
pub(crate) fn mob_interact(
    world: &World,
    body: &dyn Entity,
    player: &Player,
    hand: InteractionHand,
) -> InteractionResult {
    let player_held_item = player.inventory.lock().get_item_in_hand(hand);
    if !can_admire(body, &player_held_item) {
        return InteractionResult::Pass;
    }

    let taken = player_held_item.copy_with_count(1);
    if let Some(mob) = body.as_mob() {
        mob.use_player_item(player, hand);
    }
    hold_in_off_hand(body, taken);
    admire_gold_item(body);
    stop_walking(body);
    InteractionResult::Success
}

/// Vanilla `PiglinAi.wasHurtBy`.
///
/// Runs from `hurt_server` outside the brain tick, so it locks the brain itself.
pub(crate) fn was_hurt_by(level: &World, body: &dyn Entity, attacker: &dyn Entity) {
    if attacker.downcast_ref::<PiglinEntity>().is_some() {
        return;
    }

    if is_holding_item_in_off_hand(body) {
        stop_holding_off_hand_item(level, body, false, None);
    }

    let Some(brain) = body.as_mob().and_then(Mob::brain) else {
        return;
    };
    let is_baby = body.as_living_entity().is_some_and(LivingEntity::is_baby);

    {
        let mut brain = brain.lock();
        brain.erase_memory(&mem::CELEBRATE_LOCATION);
        brain.erase_memory(&mem::DANCING);
        brain.erase_memory(&mem::ADMIRING_ITEM);
        if attacker.downcast_ref::<Player>().is_some() {
            brain.set_memory_with_expiry(
                &mem::ADMIRING_DISABLED,
                true,
                HIT_BY_PLAYER_MEMORY_TIMEOUT,
            );
        }

        if let Some(avoid_target) = brain.get_memory(&mem::AVOID_TARGET)
            && avoid_target.entity_type().key != attacker.entity_type().key
        {
            brain.erase_memory(&mem::AVOID_TARGET);
        }

        if is_baby {
            brain.set_memory_with_expiry(
                &mem::AVOID_TARGET,
                attacker.clone(),
                BABY_FLEE_DURATION_AFTER_GETTING_HIT,
            );
        }
    }

    if is_baby {
        if is_entity_attackable_ignoring_line_of_sight(level, body, attacker) {
            let brain = brain.lock();
            broadcast_anger_target(level, body, attacker, brain.memories());
        }
        return;
    }

    if attacker.entity_type().key == vanilla_entities::HOGLIN.key
        && hoglins_outnumber_piglins_locked(body)
    {
        let mut brain = brain.lock();
        set_avoid_target_and_dont_hunt_for_a_while(&mut brain, body, attacker);
        broadcast_retreat(body, attacker, brain.memories());
        return;
    }

    maybe_retaliate(level, body, attacker);
}

/// Vanilla `PiglinAi.maybeRetaliate`.
pub(crate) fn maybe_retaliate(level: &World, body: &dyn Entity, attacker: &dyn Entity) {
    let Some(brain) = body.as_mob().and_then(Mob::brain) else {
        return;
    };

    if brain.lock().is_active(Activity::Avoid) {
        return;
    }
    if !is_entity_attackable_ignoring_line_of_sight(level, body, attacker) {
        return;
    }
    if is_other_target_much_further_away_than_current_attack_target(body, attacker, 4.0) {
        return;
    }

    let universal_anger = level.get_game_rule(&UNIVERSAL_ANGER);
    let is_player = attacker.downcast_ref::<Player>().is_some();

    let mut brain = brain.lock();
    if is_player && universal_anger {
        let target = get_nearest_visible_targetable_player(brain.memories())
            .unwrap_or_else(|| attacker.clone());
        set_anger_target(level, body, &target, brain.memories_mut());
        broadcast_universal_anger(level, body, brain.memories());
    } else {
        set_anger_target(level, body, attacker, brain.memories_mut());
        broadcast_anger_target(level, body, attacker, brain.memories());
    }
}

/// Vanilla `BehaviorUtils.isOtherTargetMuchFurtherAwayThanCurrentAttackTarget`.
fn is_other_target_much_further_away_than_current_attack_target(
    body: &dyn Entity,
    other_target: &SharedEntity,
    how_much_further_away: f64,
) -> bool {
    let Some(current) = body
        .as_mob()
        .and_then(Mob::brain)
        .and_then(|brain| brain.lock().get_memory(&mem::ATTACK_TARGET))
    else {
        return false;
    };

    let current_distance = body.position().distance_squared(current.position());
    let other_distance = body.position().distance_squared(other_target.position());
    other_distance > current_distance + how_much_further_away * how_much_further_away
}

/// Vanilla `PiglinAi.getSoundForCurrentActivity`.
#[must_use]
pub(crate) fn get_sound_for_current_activity(
    brain: &Brain,
    level: &World,
    body: &dyn Entity,
) -> Option<SoundEventRef> {
    let activity = brain.get_active_non_core_activity()?;
    Some(get_sound_for_activity(brain, level, body, activity))
}

/// Vanilla `PiglinAi.getSoundForActivity`.
fn get_sound_for_activity(
    brain: &Brain,
    level: &World,
    body: &dyn Entity,
    activity: Activity,
) -> SoundEventRef {
    if activity == Activity::Fight {
        return &sound_events::ENTITY_PIGLIN_ANGRY;
    }
    if is_converting(body) {
        return &sound_events::ENTITY_PIGLIN_RETREAT;
    }
    if activity == Activity::Avoid && is_near_avoid_target(brain, body) {
        return &sound_events::ENTITY_PIGLIN_RETREAT;
    }
    if activity == Activity::AdmireItem {
        return &sound_events::ENTITY_PIGLIN_ADMIRING_ITEM;
    }
    if activity == Activity::Celebrate {
        return &sound_events::ENTITY_PIGLIN_CELEBRATE;
    }
    if sees_player_holding_loved_item(brain.memories()) {
        return &sound_events::ENTITY_PIGLIN_JEALOUS;
    }
    let _ = level;
    if is_near_repellent(brain.memories()) {
        &sound_events::ENTITY_PIGLIN_RETREAT
    } else {
        &sound_events::ENTITY_PIGLIN_AMBIENT
    }
}

/// Vanilla `PiglinAi.isNearAvoidTarget`: the avoid target is within 12 blocks.
fn is_near_avoid_target(brain: &Brain, body: &dyn Entity) -> bool {
    brain
        .get_memory(&mem::AVOID_TARGET)
        .is_some_and(|avoid| body.position().distance_squared(avoid.position()) < 144.0)
}

/// Vanilla `AbstractPiglin.isConverting`, for the activity sound.
fn is_converting(body: &dyn Entity) -> bool {
    body.downcast_ref::<PiglinEntity>()
        .is_some_and(PiglinEntity::is_converting)
        || body
            .downcast_ref::<PiglinBruteEntity>()
            .is_some_and(PiglinBruteEntity::is_converting)
}

/// Vanilla `PiglinAi.getVisibleAdultPiglins`.
#[must_use]
pub(crate) fn get_visible_adult_piglins(memories: &MemoryStore) -> Vec<SharedEntity> {
    memories
        .get(&mem::NEAREST_VISIBLE_ADULT_PIGLINS)
        .unwrap_or_default()
}

/// Vanilla `PiglinAi.getAdultPiglins`.
#[must_use]
pub(crate) fn get_adult_piglins(memories: &MemoryStore) -> Vec<SharedEntity> {
    memories.get(&mem::NEARBY_ADULT_PIGLINS).unwrap_or_default()
}

/// Vanilla `PiglinAi.getAvoidTarget`.
#[must_use]
pub(crate) fn get_avoid_target(memories: &MemoryStore) -> Option<SharedEntity> {
    memories.get(&mem::AVOID_TARGET)
}

/// Vanilla `PiglinAi.getNearestVisibleTargetablePlayer`.
#[must_use]
pub(crate) fn get_nearest_visible_targetable_player(
    memories: &MemoryStore,
) -> Option<SharedEntity> {
    memories.get(&mem::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
}

/// Vanilla `PiglinAi.broadcastAngerTarget`.
pub(crate) fn broadcast_anger_target(
    level: &World,
    body: &dyn Entity,
    target: &SharedEntity,
    memories: &MemoryStore,
) {
    let _ = body;
    for piglin in get_adult_piglins(memories) {
        if target.entity_type().key == vanilla_entities::HOGLIN.key {
            let hoglin_huntable = target
                .downcast_ref::<HoglinEntity>()
                .is_some_and(|hoglin| !hoglin.cannot_be_hunted());
            if !can_hunt(&piglin) || !hoglin_huntable {
                continue;
            }
        }
        set_anger_target_if_closer_than_current(level, &piglin, target);
    }
}

/// Vanilla `PiglinAi.broadcastUniversalAnger`.
pub(crate) fn broadcast_universal_anger(level: &World, body: &dyn Entity, memories: &MemoryStore) {
    let _ = body;
    for piglin in get_adult_piglins(memories) {
        if let Some(brain) = piglin.as_mob().and_then(Mob::brain) {
            let mut brain = brain.lock();
            if let Some(player) = get_nearest_visible_targetable_player(brain.memories()) {
                set_anger_target(level, &piglin, &player, brain.memories_mut());
            }
        }
    }
}

/// Vanilla `PiglinAi.setAngerTarget`.
pub(crate) fn set_anger_target(
    level: &World,
    body: &dyn Entity,
    target: &SharedEntity,
    memories: &mut MemoryStore,
) {
    if !is_entity_attackable_ignoring_line_of_sight(level, body, target) {
        return;
    }

    memories.erase(&mem::CANT_REACH_WALK_TARGET_SINCE);
    memories.set_with_expiry(&mem::ANGRY_AT, target.uuid(), ANGER_DURATION);

    if target.entity_type().key == vanilla_entities::HOGLIN.key && can_hunt(body) {
        dont_kill_any_more_hoglins_for_a_while(memories);
    }

    if target.downcast_ref::<Player>().is_some() && level.get_game_rule(&UNIVERSAL_ANGER) {
        memories.set_with_expiry(&mem::UNIVERSAL_ANGER, true, ANGER_DURATION);
    }
}

/// Vanilla `PiglinAi.setAngerTargetIfCloserThanCurrent`.
fn set_anger_target_if_closer_than_current(
    level: &World,
    body: &dyn Entity,
    new_target: &SharedEntity,
) {
    let Some(brain) = body.as_mob().and_then(Mob::brain) else {
        return;
    };
    let mut brain = brain.lock();

    let current = get_anger_target(level, &brain);
    let nearest = match current {
        Some(current) => get_nearest_target(body, &current, new_target),
        None => new_target.clone(),
    };

    if !brain
        .get_memory(&mem::ANGRY_AT)
        .is_some_and(|uuid| uuid == nearest.uuid())
    {
        set_anger_target(level, body, &nearest, brain.memories_mut());
    }
}

/// Vanilla `PiglinAi.getAngerTarget`.
fn get_anger_target(level: &World, brain: &Brain) -> Option<SharedEntity> {
    let uuid = brain.get_memory(&mem::ANGRY_AT)?;
    let target = level.get_entity_by_uuid(&uuid)?;
    target.as_living_entity()?;
    Some(target)
}

/// Vanilla `BehaviorUtils.getNearestTarget`.
fn get_nearest_target(
    body: &SharedEntity,
    target1: &SharedEntity,
    target2: &SharedEntity,
) -> SharedEntity {
    let distance1 = body.position().distance_squared(target1.position());
    let distance2 = body.position().distance_squared(target2.position());
    if distance1 < distance2 {
        target1.clone()
    } else {
        target2.clone()
    }
}

/// Vanilla `PiglinAi.broadcastRetreat`.
pub(crate) fn broadcast_retreat(body: &dyn Entity, target: &SharedEntity, memories: &MemoryStore) {
    let _ = body;
    for piglin in get_visible_adult_piglins(memories) {
        if piglin.downcast_ref::<PiglinEntity>().is_some() {
            retreat_from_nearest_target(&piglin, target);
        }
    }
}

/// Vanilla `PiglinAi.retreatFromNearestTarget`.
fn retreat_from_nearest_target(body: &SharedEntity, new_avoid_target: &SharedEntity) {
    let Some(brain) = body.as_mob().and_then(Mob::brain) else {
        return;
    };
    let mut brain = brain.lock();

    let mut nearest = new_avoid_target.clone();
    if let Some(avoid) = brain.get_memory(&mem::AVOID_TARGET) {
        nearest = get_nearest_target(body, &avoid, &nearest);
    }
    if let Some(attack) = brain.get_memory(&mem::ATTACK_TARGET) {
        nearest = get_nearest_target(body, &attack, &nearest);
    }

    set_avoid_target_and_dont_hunt_for_a_while(&mut brain, body, &nearest);
}

/// Vanilla `PiglinAi.wantsToStopFleeing`.
pub(crate) fn wants_to_stop_fleeing(memories: &MemoryStore, body: &dyn Entity) -> bool {
    let Some(avoided) = memories.get(&mem::AVOID_TARGET) else {
        return true;
    };

    if avoided.entity_type().key == vanilla_entities::HOGLIN.key {
        return !hoglins_outnumber_piglins(memories, body);
    }

    if is_zombified(&avoided) {
        return !memories
            .get(&mem::NEAREST_VISIBLE_ZOMBIFIED)
            .is_some_and(|zombified| zombified.id() == avoided.id());
    }

    false
}

/// Vanilla `PiglinAi.isZombified`: zombified piglins and zoglins.
#[must_use]
pub(crate) fn is_zombified(entity: &SharedEntity) -> bool {
    entity.entity_type().key == vanilla_entities::ZOMBIFIED_PIGLIN.key
        || entity.entity_type().key == vanilla_entities::ZOGLIN.key
}

/// Vanilla `PiglinAi.hoglinsOutnumberPiglins`.
pub(crate) fn hoglins_outnumber_piglins(memories: &MemoryStore, _body: &dyn Entity) -> bool {
    let piglin_count = memories.get(&mem::VISIBLE_ADULT_PIGLIN_COUNT).unwrap_or(0) + 1;
    let hoglin_count = memories.get(&mem::VISIBLE_ADULT_HOGLIN_COUNT).unwrap_or(0);
    hoglin_count > piglin_count
}

/// Vanilla `PiglinAi.hoglinsOutnumberPiglins`, reading the brain outside its tick.
fn hoglins_outnumber_piglins_locked(body: &dyn Entity) -> bool {
    body.as_mob()
        .and_then(Mob::brain)
        .is_some_and(|brain| hoglins_outnumber_piglins(brain.lock().memories(), body))
}

/// Vanilla `PiglinAi.setAvoidTargetAndDontHuntForAWhile`.
pub(crate) fn set_avoid_target_and_dont_hunt_for_a_while(
    brain: &mut Brain,
    body: &dyn Entity,
    target: &SharedEntity,
) {
    brain.erase_memory(&mem::ANGRY_AT);
    brain.erase_memory(&mem::ATTACK_TARGET);
    brain.erase_memory(&mem::WALK_TARGET);
    let duration = i64::from(sample(RETREAT_DURATION));
    brain.set_memory_with_expiry(&mem::AVOID_TARGET, target.clone(), duration);
    dont_kill_any_more_hoglins_for_a_while(brain.memories_mut());
}

/// Vanilla `PiglinAi.dontKillAnyMoreHoglinsForAWhile`.
pub(crate) fn dont_kill_any_more_hoglins_for_a_while(memories: &mut MemoryStore) {
    let duration = i64::from(sample(TIME_BETWEEN_HUNTS));
    memories.set_with_expiry(&mem::HUNTED_RECENTLY, true, duration);
}

/// Samples a vanilla `UniformInt` with Steel's `rand`-seeded `LegacyRandom`
/// (the same distribution, different entropy source).
#[must_use]
pub(crate) fn sample(provider: UniformIntProvider) -> i32 {
    let mut random = LegacyRandom::from_seed(rand::random());
    provider.sample(&mut random)
}

/// Vanilla `PiglinAi.isPlayerHoldingLovedItem`, re-exported from the piglin sensor.
pub(crate) use crate::entity::ai::sensing::piglin_specific::is_player_holding_loved_item;
/// Vanilla `PiglinAi.isWearingSafeArmor`, re-exported from the piglin sensor.
pub(crate) use crate::entity::ai::sensing::piglin_specific::is_wearing_safe_armor;

/// Vanilla `PiglinAi.findNearbyAdultPiglins`, re-exported from the piglin sensor.
pub(crate) use crate::entity::ai::sensing::piglin_specific::find_nearby_adult_piglins;

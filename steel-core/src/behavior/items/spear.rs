//! Spear item behavior (`Item$Properties.spear`): the kinetic-weapon use tick.
//!
//! Vanilla has no `SpearItem` class — every spear is a plain `Item` whose
//! `KINETIC_WEAPON` data component drives the attack. The thrust itself lives
//! in `KineticWeapon.damageEntities`, which `ItemStack.onUseTick` calls every
//! tick of the use for **players and mobs alike** (`SpearUseGoal` /
//! `SpearAttack` only start the use; the damage comes from the shared use
//! tick). This module mirrors that method against the verified 26.3 bytecode.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::CSetEntityMotion;
use steel_registry::data_components::components::{AttackRange, KineticWeaponCondition};
use steel_registry::data_components::vanilla_components::{ATTACK_RANGE, KINETIC_WEAPON};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _, vanilla_attributes, vanilla_damage_types};
use steel_utils::WorldAabb;
use steel_utils::entity_events::EntityStatus;
use steel_utils::types::GameType;

use crate::behavior::advancement_hooks::raise_spear_mobs;
use crate::behavior::{ITEM_BEHAVIORS, ItemBehavior};
use crate::enchantment_helper::{self, EnchantmentDamageContext};
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, LivingEntity, LivingEntityRef, SharedEntity};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::game_mode::piercing_ray_hit_t;
use crate::world::{ClipBlockShape, ClipFluid, World};

/// Vanilla `KineticWeapon.getMotion`: the entity's per-tick known speed scaled
/// to blocks per second, following the root vehicle for non-player passengers.
fn kinetic_motion(entity: &dyn Entity) -> DVec3 {
    if entity.as_player().is_none()
        && entity.is_passenger()
        && let Some(root) = entity.root_vehicle()
    {
        return root.known_speed() * 20.0;
    }
    entity.known_speed() * 20.0
}

/// Vanilla `LivingEntity.getAttackRangeWith`: the stack's `ATTACK_RANGE`
/// component, or `AttackRange.defaultFor(user)` = `(0, interaction range, 0,
/// interaction range, 0, 1)`.
fn attack_range_with(user: &dyn LivingEntity, stack: &ItemStack) -> AttackRange {
    if let Some(range) = stack.get(ATTACK_RANGE) {
        return range.clone();
    }
    let interaction_range =
        user.attributes()
            .lock()
            .required_value(vanilla_attributes::ENTITY_INTERACTION_RANGE) as f32;
    AttackRange {
        min_reach: 0.0,
        max_reach: interaction_range,
        min_creative_reach: 0.0,
        max_creative_reach: interaction_range,
        hitbox_margin: 0.0,
        mob_factor: 1.0,
    }
}

/// Vanilla `AttackRange.effectiveMinRange`.
fn effective_min_range(range: &AttackRange, user: &dyn Entity, creative: bool) -> f64 {
    if user.as_player().is_some() {
        f64::from(if creative {
            range.min_creative_reach
        } else {
            range.min_reach
        })
    } else {
        f64::from(range.min_reach) * f64::from(range.mob_factor)
    }
}

/// Vanilla `AttackRange.effectiveMaxRange`.
fn effective_max_range(range: &AttackRange, user: &dyn Entity, creative: bool) -> f64 {
    if user.as_player().is_some() {
        f64::from(if creative {
            range.max_creative_reach
        } else {
            range.max_reach
        })
    } else {
        f64::from(range.max_reach) * f64::from(range.mob_factor)
    }
}

/// Vanilla `LivingEntity.getEyePosition`: feet plus `getEyeY`.
fn eye_position(entity: &dyn Entity) -> DVec3 {
    let position = entity.position();
    DVec3::new(position.x, entity.get_eye_y(), position.z)
}

// GAP(spear): vanilla checks `Entity.isInvulnerableToPiercingWeapon` here,
// which a few entity types override (see the matching sulfur-cube note). Steel
// has no such hook yet, so the general invulnerability flag stands in; an
// entity that is piercing-immune but otherwise vulnerable is currently hit.
fn can_hit_entity(user: &dyn Entity, candidate: &dyn Entity) -> bool {
    if candidate.is_invulnerable() || !candidate.is_alive() {
        return false;
    }
    // Vanilla also accepts the `Interaction` entity type here; Steel has no
    // such type, so the check is folded into the block above.
    if !candidate.can_be_hit_by_projectile() {
        return false;
    }
    if let (Some(user_player), Some(candidate_player)) = (user.as_player(), candidate.as_player())
        && !user_player.can_harm_player(candidate_player)
    {
        return false;
    }
    !user.is_passenger_of_same_vehicle(candidate)
}

/// Vanilla `ProjectileUtil.getHitEntitiesAlong` for the kinetic thrust: the
/// entities the thrust reaches, nearest first, or an empty list when a block
/// stops the ray before its end.
fn kinetic_hit_entities(user: &SharedEntity, stack: &ItemStack) -> Vec<SharedEntity> {
    let entity = user.as_ref();
    let Some(world) = entity.level() else {
        return Vec::new();
    };
    let look = entity.look_angle();
    if look.length_squared() <= f64::EPSILON {
        return Vec::new();
    }

    let creative = entity
        .as_player()
        .is_some_and(|player| player.game_mode() == GameType::Creative);
    let range = attack_range_with(
        entity
            .as_living_entity()
            .expect("a kinetic weapon is only used by living entities"),
        stack,
    );
    let eye = eye_position(entity);
    let from = eye + look * effective_min_range(&range, entity, creative);
    let movement_extension = entity.known_movement().dot(look).max(0.0);
    let to = eye + look * (effective_max_range(&range, entity, creative) + movement_extension);

    let block_hit = world.clip(from, to, ClipBlockShape::Collider, ClipFluid::None);
    if !block_hit.is_miss() && eye.distance_squared(block_hit.location) < eye.distance_squared(to) {
        return Vec::new();
    }

    let margin = f64::from(range.hitbox_margin);
    let search_area = WorldAabb::new(from.x, from.y, from.z, from.x, from.y, from.z)
        .inflate_xyz(margin, margin, margin)
        .expand_towards(to - from)
        .inflate(1.0);

    let mut hits = world
        .get_entities_in_aabb_matching(&search_area, |candidate| {
            // Vanilla queries `Level.getEntities(user, …)`, which skips the
            // thrusting entity itself; the ray would otherwise start inside its
            // own box and stab the user.
            candidate.id() != entity.id() && can_hit_entity(entity, candidate)
        })
        .into_iter()
        .filter_map(|candidate| {
            piercing_ray_hit_t(&world, candidate.bounding_box(), from, to, margin)
                .map(|hit_t| (hit_t, candidate))
        })
        .collect::<Vec<_>>();
    hits.sort_by(|(left, _), (right, _)| left.total_cmp(right));
    hits.into_iter().map(|(_, candidate)| candidate).collect()
}

/// Vanilla `ItemStack.getDamageSource(user)`: the `DAMAGE_TYPE` component when
/// present (spears carry `minecraft:spear`), else the item's own override, else
/// a plain mob attack.
fn stab_damage_source(user: &SharedEntity, stack: &ItemStack) -> DamageSource {
    if let Some(damage_type) = stack.get_damage_type() {
        return DamageSource::direct(damage_type, Arc::clone(user));
    }
    if let Some(source) = ITEM_BEHAVIORS
        .get_behavior(stack.item())
        .get_item_damage_source(user)
    {
        return source;
    }
    DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(user))
}

/// Vanilla `LivingEntity.causeExtraKnockback`: knock the target away along the
/// attacker's facing and dampen the attacker's own horizontal velocity.
fn cause_extra_knockback(
    attacker: &SharedEntity,
    target: &dyn Entity,
    knockback_amount: f64,
    old_movement: DVec3,
) {
    if knockback_amount > 0.0 {
        let yaw_radians = f64::from(attacker.rotation().0.to_radians());
        let yaw_sin = yaw_radians.sin();
        let yaw_cos = yaw_radians.cos();
        if let Some(living_target) = target.as_living_entity() {
            living_target.knockback(knockback_amount, yaw_sin, -yaw_cos);
        } else {
            target.push_impulse(DVec3::new(
                -yaw_sin * knockback_amount,
                0.1,
                yaw_cos * knockback_amount,
            ));
        }

        let velocity = attacker.velocity();
        attacker.set_velocity(DVec3::new(velocity.x * 0.6, velocity.y, velocity.z * 0.6));
        if let Some(user) = attacker.as_living_entity() {
            user.set_sprinting(false);
        }
    }

    if let Some(player) = target.as_player()
        && target.hurt_marked()
    {
        let velocity = target.velocity();
        player.send_packet(CSetEntityMotion::new(target.id(), velocity));
        target.clear_hurt_mark();
        target.set_velocity(old_movement);
    }
}

/// Vanilla `LivingEntity.getKnockback`: the attack-knockback attribute plus the
/// weapon's knockback enchantment, halved.
fn stab_knockback(
    user: &SharedEntity,
    target: &dyn Entity,
    stack: &ItemStack,
    damage_source: &DamageSource,
) -> f64 {
    let Some(user_living) = user.as_living_entity() else {
        return 0.0;
    };
    let attack_knockback = user_living
        .attributes()
        .lock()
        .get_base_value(vanilla_attributes::ATTACK_KNOCKBACK)
        .unwrap_or(0.0);
    let context = EnchantmentDamageContext::new(
        target.entity_type(),
        Some(user.entity_type()),
        Some(user.entity_type()),
        damage_source,
    );
    f64::from(enchantment_helper::modify_knockback(
        stack,
        &context,
        attack_knockback as f32,
    )) / 2.0
}

/// Vanilla `LivingEntity.stabAttack`: one thrust against one entity.
fn stab_attack(
    user: &SharedEntity,
    slot: EquipmentSlot,
    target: &SharedEntity,
    base_damage: f32,
    deals_damage: bool,
    deals_knockback: bool,
    dismounts: bool,
) -> bool {
    let entity = user.as_ref();
    let target_entity = target.as_ref();
    let Some(world) = entity.level() else {
        return false;
    };
    let Some(user_living) = entity.as_living_entity() else {
        return false;
    };

    let weapon = {
        let mut stack = ItemStack::empty();
        user_living.with_equipment_slot(slot, &mut |item| stack = item.clone());
        stack
    };
    let damage_source = stab_damage_source(user, &weapon);
    let enchantment_context = EnchantmentDamageContext::new(
        target_entity.entity_type(),
        Some(entity.entity_type()),
        Some(entity.entity_type()),
        &damage_source,
    );
    let damage = enchantment_helper::modify_damage(&weapon, &enchantment_context, base_damage);
    let old_movement = target_entity.velocity();

    let mut affected = deals_knockback;
    let hurt = deals_damage && target_entity.hurt(&world, &damage_source, damage);
    affected |= hurt;
    if deals_knockback {
        cause_extra_knockback(user, target_entity, 0.4, old_movement);
        cause_extra_knockback(
            user,
            target_entity,
            stab_knockback(user, target_entity, &weapon, &damage_source),
            old_movement,
        );
    }
    if dismounts
        && target_entity.is_passenger()
        && !REGISTRY.entity_types.is_in_tag(
            target_entity.entity_type(),
            &EntityTypeTag::CANNOT_BE_DISMOUNTED_BY_ITEM_USAGE,
        )
    {
        affected = true;
        target_entity.stop_riding();
    }

    if let Some(living_target) = target_entity.as_living_entity() {
        let mut stack = weapon;
        ITEM_BEHAVIORS.get_behavior(stack.item()).hurt_enemy(
            &mut stack,
            living_target,
            user_living,
        );
    }
    if hurt {
        enchantment_helper::do_post_piercing_attack_effects(&world, user_living);
    }
    if !affected {
        return false;
    }

    user_living.set_last_hurt_mob(Some(target));
    true
}

impl SpearItem {
    /// Vanilla `KineticWeapon.damageEntities`, run from `ItemStack.onUseTick`.
    fn kinetic_damage_entities(
        world: &Arc<World>,
        user: &SharedEntity,
        stack: &ItemStack,
        ticks_remaining: i32,
    ) {
        let Some(entity) = user.as_living_entity() else {
            return;
        };
        let Some(kinetic) = stack.get(KINETIC_WEAPON) else {
            return;
        };

        // `stack.getUseDuration(user) - useTicks`, then the wind-up delay.
        let use_duration = ITEM_BEHAVIORS
            .get_behavior(stack.item())
            .get_use_duration(stack, entity);
        let mut ticks_used = use_duration - ticks_remaining;
        if ticks_used < kinetic.delay_ticks() {
            return;
        }
        ticks_used -= kinetic.delay_ticks();

        let look = entity.look_angle();
        let attacker_speed = look.dot(kinetic_motion(user.as_ref()));
        let speed_scale = if entity.as_player().is_some() {
            1.0
        } else {
            0.2
        };
        // Vanilla `getAttributeBaseValue`: 0 when the entity type has no such
        // instance (e.g. a pig caught by the sweep), which Steel's
        // `required_value` would panic on.
        let attacker_damage = entity
            .attributes()
            .lock()
            .get_base_value(vanilla_attributes::ATTACK_DAMAGE)
            .unwrap_or(0.0);

        let mut hit_something = false;
        for target in kinetic_hit_entities(user, stack) {
            let target_entity = target.as_ref();
            if entity.was_recently_stabbed(target_entity.id(), kinetic.contact_cooldown_ticks()) {
                continue;
            }
            entity.remember_stabbed_entity(target_entity);

            let target_speed = look.dot(kinetic_motion(target_entity));
            let relative_speed = (attacker_speed - target_speed).max(0.0);

            let passes = |condition: Option<&KineticWeaponCondition>| {
                condition.is_some_and(|condition| {
                    condition.test(ticks_used, attacker_speed, relative_speed, speed_scale)
                })
            };
            let dismounts = passes(kinetic.dismount_conditions());
            let knocks_back = passes(kinetic.knockback_conditions());
            let deals_damage = passes(kinetic.damage_conditions());
            if !dismounts && !knocks_back && !deals_damage {
                continue;
            }

            let damage = attacker_damage as f32
                + (relative_speed * f64::from(kinetic.damage_multiplier())).floor() as f32;
            hit_something |= stab_attack(
                user,
                EquipmentSlot::MainHand,
                &target,
                damage,
                deals_damage,
                knocks_back,
                dismounts,
            );
        }

        if !hit_something {
            return;
        }

        entity.broadcast_entity_event(EntityStatus::KineticHit);
        if let Some(player) = entity.as_player() {
            // Vanilla counts the remembered stabs whose target is a living
            // entity (`stabbedEntities(e -> e instanceof LivingEntity)`).
            let count = entity.stabbed_living_entities();
            raise_spear_mobs(player, world, count);
        }
    }
}

/// Vanilla spear item: a plain `Item` whose `KINETIC_WEAPON` component drives
/// the thrust on every use tick.
///
/// Spears have no vanilla item class of their own (all seven are plain items
/// built by `Item$Properties.spear`), so the behavior registers by item name
/// instead of by class.
#[item_behavior(
    items = "wooden_spear, stone_spear, copper_spear, iron_spear, golden_spear, diamond_spear, netherite_spear"
)]
pub struct SpearItem;

impl ItemBehavior for SpearItem {
    fn on_use_tick(
        &self,
        world: &Arc<World>,
        user: LivingEntityRef<'_>,
        stack: &mut ItemStack,
        ticks_remaining: i32,
    ) {
        let user_entity = SharedEntity::clone(user.entity());
        Self::kinetic_damage_entities(world, &user_entity, stack, ticks_remaining);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::data_components::components::{KineticWeapon, KineticWeaponCondition};
    use steel_registry::data_components::vanilla_components::KINETIC_WEAPON;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::ChunkPos;
    use steel_utils::locks::SyncMutex;
    use steel_utils::types::InteractionHand;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::{
        EntityBase, LivingEntity, LivingEntityBase, LivingEntityRef, impl_test_downcast_type,
    };
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
    use steel_registry::entity_type::EntityTypeRef;

    const USE_DURATION: i32 = 72000;

    /// Living test double with a world-backed base for `on_use_tick`.
    struct SpearTestMob {
        base: EntityBase,
        living_base: LivingEntityBase,
        health: SyncMutex<f32>,
    }

    impl SpearTestMob {
        fn shared(id: i32, world: &Arc<World>, position: DVec3) -> SharedEntity {
            Arc::new(Self {
                // A zombie stand-in for the vanilla attack-damage attribute
                // the thrust reads through `getAttributeBaseValue`.
                base: EntityBase::new(
                    id,
                    position,
                    vanilla_entities::ZOMBIE.dimensions,
                    Arc::downgrade(world),
                ),
                living_base: LivingEntityBase::new(&vanilla_entities::ZOMBIE),
                health: SyncMutex::new(10.0),
            })
        }
    }

    impl_test_downcast_type!(SpearTestMob);

    impl Entity for SpearTestMob {
        fn base(&self) -> &EntityBase {
            &self.base
        }

        fn entity_type(&self) -> EntityTypeRef {
            &vanilla_entities::ZOMBIE
        }
    }

    impl LivingEntity for SpearTestMob {
        fn living_base(&self) -> &LivingEntityBase {
            &self.living_base
        }

        fn get_health(&self) -> f32 {
            *self.health.lock()
        }

        fn set_health(&self, health: f32) {
            *self.health.lock() = health;
        }
    }

    use crate::test_support::TestWorld;

    fn fixture(name: &'static str) -> (TestWorld, SharedEntity, SharedEntity) {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world(name);
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let user = SpearTestMob::shared(1, world, DVec3::new(0.5, 65.0, 0.5));
        let target = SpearTestMob::shared(2, world, DVec3::new(0.5, 65.0, 2.0));
        world
            .try_add_entity(Arc::clone(&user))
            .expect("user joins the world");
        world
            .try_add_entity(Arc::clone(&target))
            .expect("target joins the world");
        (world_fixture, user, target)
    }

    fn thrust_stack() -> ItemStack {
        // Zero speed gates so the thrust lands at rest; `delay_ticks` 8 like
        // the netherite spear.
        let mut stack = ItemStack::new(&vanilla_items::NETHERITE_SPEAR);
        let kinetic = KineticWeapon::new(
            10,
            8,
            Some(KineticWeaponCondition::new(50, 0.0, 0.0).expect("valid condition")),
            Some(KineticWeaponCondition::new(50, 0.0, 0.0).expect("valid condition")),
            Some(KineticWeaponCondition::new(50, 0.0, 0.0).expect("valid condition")),
            0.38,
            1.2,
            None,
            None,
        )
        .expect("valid kinetic weapon");
        stack.set(KINETIC_WEAPON, kinetic);
        stack
    }

    fn use_tick(user: &SharedEntity, stack: &mut ItemStack, ticks_used: i32, world: &Arc<World>) {
        let user_ref = LivingEntityRef::new(user).expect("the thrust user is a living entity");
        ITEM_BEHAVIORS.get_behavior(stack.item()).on_use_tick(
            world,
            user_ref,
            stack,
            USE_DURATION - ticks_used,
        );
    }

    fn living(entity: &SharedEntity) -> &dyn LivingEntity {
        entity
            .as_living_entity()
            .expect("test mobs are living entities")
    }

    #[test]
    fn spear_use_tick_thrusts_only_once_past_the_wind_up_and_tracks_recent_stabs() {
        let (world_fixture, user, target) = fixture("spear_use_tick_thrust");
        let world = &world_fixture.world;
        let user_ref = living(&user);
        user_ref.set_rotation((0.0, 0.0));

        let mut stack = thrust_stack();
        assert_eq!(
            living(&target).get_health(),
            10.0,
            "the target starts unhurt"
        );

        // The full vanilla path starts with `startUsingItem`, which creates the
        // fresh recent-stab map for a kinetic-weapon use.
        user_ref.set_item_in_hand(InteractionHand::MainHand, stack.clone());
        user_ref.start_using_item(InteractionHand::MainHand);
        assert!(user_ref.is_using_item());

        // Inside the 8-tick wind-up (`delay_ticks`) nothing lands.
        use_tick(&user, &mut stack, 7, world);
        assert_eq!(living(&target).get_health(), 10.0);

        // The first thrust past the wind-up lands and remembers the target.
        use_tick(&user, &mut stack, 8, world);
        let after_first = living(&target).get_health();
        assert!(
            after_first < 10.0,
            "the thrust past `delay_ticks` must hurt the target"
        );
        assert!(
            user_ref.was_recently_stabbed(target.id(), 10),
            "the stab is remembered for `contact_cooldown_ticks`"
        );

        // The same target is skipped until the contact cooldown elapses.
        use_tick(&user, &mut stack, 9, world);
        assert_eq!(living(&target).get_health(), after_first);

        // A fresh mob in the same spot is still hit.
        let second = SpearTestMob::shared(3, world, DVec3::new(0.5, 65.0, 2.0));
        world
            .try_add_entity(Arc::clone(&second))
            .expect("second target joins the world");
        use_tick(&user, &mut stack, 10, world);
        assert!(living(&second).get_health() < 10.0);
        assert_eq!(living(&target).get_health(), after_first);

        // Ending the use clears the map, matching vanilla `stopUsingItem`.
        user_ref.stop_using_item();
        assert!(!user_ref.was_recently_stabbed(target.id(), 10));
    }
}

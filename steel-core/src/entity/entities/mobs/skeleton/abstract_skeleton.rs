//! Vanilla `AbstractSkeleton`
//! (`net.minecraft.world.entity.monster.skeleton.AbstractSkeleton`).
//!
//! Vanilla's `Skeleton`, `Stray`, `Bogged`, `WitherSkeleton` and `Parched` all extend
//! `AbstractSkeleton`, which owns the sun-avoidance goals, the bow/melee weapon switch, the
//! shared spawn equipment and the ranged attack. Rust has no inheritance, so that shared
//! body lives here as a capability trait in the same style as [`Monster`] and
//! [`Animal`](crate::entity::Animal): every concrete skeleton struct delegates its
//! entity-trait hooks to these defaults.

use std::sync::Arc;

use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{sound_events, vanilla_entities, vanilla_items};
use steel_utils::random::Random;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::{Difficulty, InteractionHand};
use steel_utils::{BlockPos, BlockStateId, Identifier};

use crate::entity::ai::goal::{
    AvoidEntityGoal, FleeSunGoal, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal,
    NearestAttackableTargetGoal, RandomLookAroundGoal, RangedAttackMob, RangedBowAttackGoal,
    RestrictSunGoal, WaterAvoidingRandomStrollGoal, is_goal,
};
use crate::entity::{
    ENTITIES, EntitySpawnReason, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster,
    SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::world::World;

/// Vanilla `AbstractSkeleton.HARD_ATTACK_INTERVAL`.
const HARD_ATTACK_INTERVAL: i32 = 20;
/// Vanilla `AbstractSkeleton.NORMAL_ATTACK_INTERVAL`.
const NORMAL_ATTACK_INTERVAL: i32 = 40;
/// Vanilla `AbstractSkeleton.finalizeSpawn`: `0.55F * difficulty.getSpecialMultiplier()`.
const CAN_PICK_UP_LOOT_CHANCE: f32 = 0.55;
/// Vanilla `AbstractSkeleton` constructor: `new RangedBowAttackGoal<>(this, 1.0, 20, 15.0F)`.
const BOW_SPEED_MODIFIER: f64 = 1.0;
const BOW_ATTACK_RADIUS: f32 = 15.0;
/// Vanilla `AbstractSkeleton` melee goal: `new MeleeAttackGoal(this, 1.2, false)`.
const MELEE_SPEED_MODIFIER: f64 = 1.2;
/// Vanilla `Monster`'s default experience reward, which `MobBase` does not seed.
const XP_REWARD: i32 = 5;
/// Vanilla `LivingEntity.getStepHeight` fallback used by [`Entity::max_up_step`].
pub(crate) const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `AbstractSkeleton.getArrow` payload, minus the projectile entity.
///
/// Steel's `Arrow` entity is owned by the projectile lane; until `ENTITIES` exposes an
/// `arrow` factory this carries the vanilla per-type payload (`Arrow.addEffect` and
/// `AbstractArrow.igniteForSeconds`) so each subclass' `get_arrow` override stays exact.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::derive_partial_eq_without_eq,
    reason = "the vanilla payload carries f32 power and ignition seconds, which are not `Eq`"
)]
pub struct SkeletonArrow {
    /// Vanilla `AbstractArrow.setBaseDamageFromMob(power)`.
    pub power: f32,
    /// Vanilla `Arrow.addEffect` (`Stray` slowness, `Bogged` poison, `Parched` weakness).
    pub effect: Option<MobEffectInstance>,
    /// Vanilla `AbstractArrow.igniteForSeconds` (`WitherSkeleton`).
    pub ignite_seconds: Option<f32>,
}

impl SkeletonArrow {
    /// Vanilla `AbstractSkeleton.getArrow`: the released arrow with no extra effect.
    ///
    /// Subclasses extend this result in their `get_arrow` overrides; a trait default
    /// cannot be reached from an override, so the base payload lives here.
    #[must_use]
    pub(crate) const fn base(power: f32) -> Self {
        Self {
            power,
            effect: None,
            ignite_seconds: None,
        }
    }
}

/// Shared behavior of vanilla's `AbstractSkeleton` subclasses.
pub trait AbstractSkeleton: Monster + RangedAttackMob {
    /// Vanilla `AbstractSkeleton.registerGoals`.
    fn register_goals(&self)
    where
        Self: Sized,
    {
        {
            let mut goal_selector = self.mob_base().goal_selector().lock();
            goal_selector.add_goal(2, RestrictSunGoal::new());
            goal_selector.add_goal(3, FleeSunGoal::new(1.0));
            goal_selector.add_goal(
                3,
                AvoidEntityGoal::with_selector(6.0, 1.0, 1.2, |target, _| {
                    target.entity_type() == &vanilla_entities::WOLF
                }),
            );
            goal_selector.add_goal(5, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(6, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(6, RandomLookAroundGoal::new());
        }

        let mut target_selector = self.mob_base().target_selector().lock();
        // Vanilla `new HurtByTargetGoal(this)` alerts the same mob type.
        target_selector.add_goal(
            1,
            HurtByTargetGoal::new().set_alert_others(std::iter::empty()),
        );
        target_selector.add_goal(
            2,
            NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
        );
        target_selector.add_goal(
            3,
            NearestAttackableTargetGoal::new(true, |entity, _| {
                entity.entity_type() == &vanilla_entities::IRON_GOLEM
            }),
        );
        // Vanilla `Turtle.BABY_ON_LAND_SELECTOR`.
        target_selector.add_goal(
            3,
            NearestAttackableTargetGoal::new_with_interval(10, true, false, |entity, _| {
                entity.entity_type() == &vanilla_entities::TURTLE
                    && entity.is_baby()
                    && !entity.is_in_water()
            }),
        );
    }

    /// Registers the vanilla goals and then re-evaluates the weapon goal, mirroring the
    /// order of the `Mob` and `AbstractSkeleton` constructors.
    fn setup_goals(&self)
    where
        Self: Sized,
    {
        self.register_goals();
        self.reassess_weapon_goal();
    }

    /// Vanilla `AbstractSkeleton.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData>
    where
        Self: Sized,
    {
        let mut random = LegacyRandom::from_seed(rand::random());
        let difficulty_modifier = world
            .current_difficulty_at(self.block_position())
            .special_multiplier();

        let group_data = self.finalize_spawn_mob_base(world, spawn_reason, group_data);

        self.populate_default_equipment_slots(&mut random, difficulty_modifier);
        self.populate_default_equipment_enchantments(world, &mut random, difficulty_modifier);
        self.reassess_weapon_goal();
        self.set_can_pick_up_loot(
            random.next_f32() < CAN_PICK_UP_LOOT_CHANCE * difficulty_modifier,
        );

        // Steel has no `SpecialDates` calendar, so vanilla's Halloween pumpkin-head roll is
        // not applied.

        group_data
    }

    /// Vanilla `AbstractSkeleton.reassessWeaponGoal`: swaps the priority-4 bow and melee
    /// goals to match the weapon in hand.
    fn reassess_weapon_goal(&self)
    where
        Self: Sized,
    {
        let Some(world) = self.level() else {
            return;
        };

        let mut goal_selector = self.mob_base().goal_selector().lock();
        goal_selector.remove_all_goals(self, |goal| {
            is_goal::<RangedBowAttackGoal>(goal) || is_goal::<MeleeAttackGoal>(goal)
        });

        if self.get_weapon_holding_hand(&vanilla_items::BOW).is_some() {
            // Vanilla mutates its single `bowGoal` field with `setMinAttackInterval`; Steel
            // removes and re-adds the goal, so the interval goes to the constructor instead.
            let min_attack_interval = if world.difficulty() == Difficulty::Hard {
                self.get_hard_attack_interval()
            } else {
                self.get_attack_interval()
            };
            goal_selector.add_goal(
                4,
                RangedBowAttackGoal::new(
                    BOW_SPEED_MODIFIER,
                    min_attack_interval,
                    BOW_ATTACK_RADIUS,
                ),
            );
        } else {
            // Vanilla's anonymous `MeleeAttackGoal` subclass only adds the aggressive flag
            // around `start`/`stop`; Steel's `MeleeAttackGoal` already sets it there.
            goal_selector.add_goal(4, MeleeAttackGoal::new(MELEE_SPEED_MODIFIER, false));
        }
    }

    /// Vanilla `AbstractSkeleton.performRangedAttack`.
    ///
    /// Vanilla builds `getArrow(getProjectile(bow), power, bow)` and releases it through
    /// `Projectile.spawnProjectileUsingShoot(1.6F, rangedAttackUncertainty(level))`.
    /// Steel's `Arrow` entity is owned by the projectile lane: until `ENTITIES` exposes an
    /// `arrow` factory there is nothing to shoot, so the shot is not released (the bow goal
    /// keeps its vanilla charge window) and only the vanilla shot sound plays.
    fn perform_ranged_attack(&self, _target: &SharedEntity, _power: f32) -> bool {
        let mut random = LegacyRandom::from_seed(rand::random());
        self.play_sound(
            &sound_events::ENTITY_SKELETON_SHOOT,
            1.0,
            1.0 / (random.next_f32() * 0.4 + 0.8),
        );

        ENTITIES.has_factory(&vanilla_entities::ARROW)
    }

    /// Vanilla `AbstractSkeleton.playStepSound`.
    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(self.get_step_sound(), 0.15, 1.0);
    }

    /// Vanilla `AbstractSkeleton.rideTick`: a ridden skeleton mirrors its pathfinder
    /// mount's body rotation.
    fn ride_tick(self: Arc<Self>)
    where
        Self: Sized,
    {
        Arc::clone(&self).default_ride_tick();
        if let Some(living) = self.as_living_entity() {
            living.reset_fall_distance();
        }

        if let Some(vehicle) = self.controlled_vehicle()
            && let Some(mount) = vehicle.as_pathfinder_mob()
        {
            self.set_y_body_rot(mount.y_body_rot());
        }
    }

    /// Vanilla `AbstractSkeleton.populateDefaultEquipmentSlots`.
    fn populate_default_equipment_slots(
        &self,
        random: &mut impl Random,
        difficulty_modifier: f32,
    ) {
        // Vanilla calls `Mob.populateDefaultEquipmentSlots` first, then forces a bow.
        crate::entity::entities::mobs::spawn_equipment::populate_default_equipment_slots(
            self,
            random,
            difficulty_modifier,
        );
        self.living_base()
            .equipment()
            .lock()
            .set(EquipmentSlot::MainHand, ItemStack::new(&vanilla_items::BOW));
    }

    /// Vanilla `Mob.populateDefaultEquipmentEnchantments`.
    fn populate_default_equipment_enchantments(
        &self,
        _world: &World,
        random: &mut (impl Random + steel_registry::enchantment::table::EnchantmentRandom),
        difficulty_modifier: f32,
    ) {
        crate::entity::entities::mobs::spawn_equipment::populate_default_equipment_enchantments(
            self,
            random,
            difficulty_modifier,
        );
    }

    /// Vanilla `AbstractSkeleton.canUseNonMeleeWeapon`.
    fn can_use_non_melee_weapon(&self, item_stack: &ItemStack) -> bool {
        item_stack.is(&vanilla_items::BOW)
    }

    /// Vanilla `AbstractSkeleton.getPreferredWeaponType`.
    fn get_preferred_weapon_type(&self) -> Option<Identifier> {
        Some(ItemTag::SKELETON_PREFERRED_WEAPONS)
    }

    /// Vanilla `AbstractSkeleton.wantsToPickUp`: skeletons never pick up spears.
    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        !item_stack.item().has_tag(&ItemTag::SPEARS) && Mob::wants_to_pick_up(self, item_stack)
    }

    /// Vanilla `AbstractSkeleton.getArrow` (through `ProjectileUtil.getMobArrow`).
    fn get_arrow(
        &self,
        _projectile: &ItemStack,
        power: f32,
        _firing_weapon: Option<&ItemStack>,
    ) -> SkeletonArrow {
        SkeletonArrow::base(power)
    }

    /// Vanilla `AbstractSkeleton.getHardAttackInterval`.
    fn get_hard_attack_interval(&self) -> i32 {
        HARD_ATTACK_INTERVAL
    }

    /// Vanilla `AbstractSkeleton.getAttackInterval`.
    fn get_attack_interval(&self) -> i32 {
        NORMAL_ATTACK_INTERVAL
    }

    /// Vanilla `AbstractSkeleton.getStepSound`.
    fn get_step_sound(&self) -> SoundEventRef;

    /// Vanilla `AbstractSkeleton.isShaking`.
    fn is_shaking(&self) -> bool {
        self.is_fully_frozen()
    }

    /// Vanilla `ProjectileUtil.getWeaponHoldingHand`: the hand holding `item`, main hand
    /// first, or `None` when neither hand holds it.
    fn get_weapon_holding_hand(&self, item: ItemRef) -> Option<InteractionHand> {
        if self.get_item_in_hand(InteractionHand::MainHand).is(item) {
            Some(InteractionHand::MainHand)
        } else if self.get_item_in_hand(InteractionHand::OffHand).is(item) {
            Some(InteractionHand::OffHand)
        } else {
            None
        }
    }
}

/// Builds the shared living/mob state every skeleton body owns.
///
/// Attributes come from `entities.json` through [`LivingEntityBase::new`]; the experience
/// reward is vanilla `Monster`'s default, which Steel's `MobBase` does not seed.
pub(crate) fn skeleton_bases(entity_type: EntityTypeRef) -> (LivingEntityBase, MobBase) {
    let living_base = LivingEntityBase::new(entity_type);
    let mob_base = MobBase::new();
    mob_base.set_xp_reward(XP_REWARD);
    (living_base, mob_base)
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_mob_effects};
    use steel_utils::Identifier;

    use super::{AbstractSkeleton, SkeletonArrow};
    use crate::entity::entities::mobs::skeleton::{
        BoggedEntity, ParchedEntity, SkeletonEntity, StrayEntity, WitherSkeletonEntity,
    };

    fn arrow_of(mob: &impl AbstractSkeleton) -> SkeletonArrow {
        mob.get_arrow(&ItemStack::empty(), 1.0, None)
    }

    /// The yielded effect's registry key and duration, for arrow-payload assertions.
    fn effect_key_and_duration(arrow: &SkeletonArrow) -> Option<(Identifier, i32)> {
        arrow
            .effect
            .as_ref()
            .map(|effect| (effect.effect().key.clone(), effect.duration()))
    }

    #[test]
    fn skeleton_family_arrows_carry_their_vanilla_effects() {
        init_vanilla_registry();

        let skeleton =
            SkeletonEntity::new(&vanilla_entities::SKELETON, 1, DVec3::ZERO, Weak::new());
        let plain = arrow_of(&skeleton);
        assert!(plain.effect.is_none());
        assert!(plain.ignite_seconds.is_none());

        let stray = StrayEntity::new(&vanilla_entities::STRAY, 2, DVec3::ZERO, Weak::new());
        let slowness = arrow_of(&stray);
        assert_eq!(
            effect_key_and_duration(&slowness),
            Some((vanilla_mob_effects::SLOWNESS.key.clone(), 600)),
            "vanilla `Stray.getArrow` adds 600-tick slowness"
        );

        let bogged = BoggedEntity::new(&vanilla_entities::BOGGED, 3, DVec3::ZERO, Weak::new());
        let poison = arrow_of(&bogged);
        assert_eq!(
            effect_key_and_duration(&poison),
            Some((vanilla_mob_effects::POISON.key.clone(), 100)),
            "vanilla `Bogged.getArrow` adds 100-tick poison"
        );

        let parched = ParchedEntity::new(&vanilla_entities::PARCHED, 4, DVec3::ZERO, Weak::new());
        let weakness = arrow_of(&parched);
        assert_eq!(
            effect_key_and_duration(&weakness),
            Some((vanilla_mob_effects::WEAKNESS.key.clone(), 600)),
            "vanilla `Parched.getArrow` adds 600-tick weakness"
        );

        let wither = WitherSkeletonEntity::new(
            &vanilla_entities::WITHER_SKELETON,
            5,
            DVec3::ZERO,
            Weak::new(),
        );
        let burning = arrow_of(&wither);
        assert!(burning.ignite_seconds.is_some_and(|seconds| seconds > 0.0));
        assert!(burning.effect.is_none());
    }
}

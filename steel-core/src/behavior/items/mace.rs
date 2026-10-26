use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::CSetEntityMotion;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{level_events, sound_events, vanilla_damage_types};

use crate::behavior::ItemBehavior;
use crate::enchantment_helper::{self, EnchantmentDamageContext};
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, LivingEntity, SharedEntity};
use crate::inventory::equipment::EquipmentSlot;

/// Vanilla mace item combat behavior.
#[item_behavior]
pub struct MaceItem;

impl MaceItem {
    const SMASH_ATTACK_FALL_THRESHOLD: f64 = 1.5;
    const SMASH_ATTACK_HEAVY_THRESHOLD: f64 = 5.0;
    const SMASH_ATTACK_KNOCKBACK_RADIUS: f64 = 3.5;
    const SMASH_ATTACK_KNOCKBACK_POWER: f64 = 0.7;
    const SMASH_ATTACK_PARTICLE_DATA: i32 = 750;

    /// Vanilla `MaceItem.getAttackDamageBonus`'s piecewise fall-distance term.
    ///
    /// Verified against the 26.3 jar: `4 * d` up to 3 blocks, `12 + 2 * (d -
    /// 3)` up to 8, then `22 + (d - 8)`. Vanilla adds the density
    /// `modifyFallBasedDamage` term on top; that step lives in
    /// `get_attack_damage_bonus` below.
    fn smash_attack_damage(fall_distance: f64) -> f64 {
        if fall_distance <= 3.0 {
            4.0 * fall_distance
        } else if fall_distance <= 8.0 {
            12.0 + 2.0 * (fall_distance - 3.0)
        } else {
            22.0 + fall_distance - 8.0
        }
    }

    /// Vanilla `MaceItem.getKnockbackPower` for a nearby entity at `distance`.
    fn knockback_power_for(fall_distance: f64, distance: f64, knockback_resistance: f64) -> f64 {
        let heavy_multiplier = if fall_distance > Self::SMASH_ATTACK_HEAVY_THRESHOLD {
            2.0
        } else {
            1.0
        };
        (Self::SMASH_ATTACK_KNOCKBACK_RADIUS - distance)
            * Self::SMASH_ATTACK_KNOCKBACK_POWER
            * heavy_multiplier
            * (1.0 - knockback_resistance)
    }

    fn can_smash_attack(attacker: &dyn LivingEntity) -> bool {
        attacker.fall_distance() > Self::SMASH_ATTACK_FALL_THRESHOLD && !attacker.is_fall_flying()
    }

    fn calculate_impact_position(attacker: &dyn LivingEntity) -> DVec3 {
        if let Some(impact_pos) = attacker.current_impulse_impact_pos()
            && impact_pos.y <= attacker.position().y
        {
            return impact_pos;
        }

        attacker.position()
    }

    fn smash_sound(target: &dyn LivingEntity, attacker: &dyn LivingEntity) -> SoundEventRef {
        if !target.on_ground() {
            return &sound_events::ITEM_MACE_SMASH_AIR;
        }

        if attacker.fall_distance() > Self::SMASH_ATTACK_HEAVY_THRESHOLD {
            &sound_events::ITEM_MACE_SMASH_GROUND_HEAVY
        } else {
            &sound_events::ITEM_MACE_SMASH_GROUND
        }
    }

    fn knockback_power(
        attacker: &dyn LivingEntity,
        nearby: &dyn LivingEntity,
        direction: DVec3,
    ) -> f64 {
        Self::knockback_power_for(
            attacker.fall_distance(),
            direction.length(),
            nearby.knockback_resistance(),
        )
    }

    fn should_knockback(
        attacker: &dyn LivingEntity,
        target: &dyn LivingEntity,
        nearby: &dyn LivingEntity,
    ) -> bool {
        if nearby.is_spectator() || nearby.id() == attacker.id() || nearby.id() == target.id() {
            return false;
        }
        if attacker.is_allied_to(nearby) || nearby.is_tame_owned_by(target) {
            return false;
        }
        if nearby.is_marker_armor_stand() {
            return false;
        }
        if nearby
            .as_player()
            .is_some_and(|player| player.has_infinite_materials() && player.is_flying())
        {
            return false;
        }

        target.position().distance_squared(nearby.position())
            <= Self::SMASH_ATTACK_KNOCKBACK_RADIUS * Self::SMASH_ATTACK_KNOCKBACK_RADIUS
    }

    fn apply_knockback(attacker: &dyn LivingEntity, target: &dyn LivingEntity) {
        let Some(world) = attacker.level() else {
            return;
        };

        let event_pos = target
            .on_pos(1.0e-5)
            .unwrap_or_else(|| target.block_position());
        world.level_event(
            level_events::PARTICLES_SMASH_ATTACK,
            event_pos,
            Self::SMASH_ATTACK_PARTICLE_DATA,
            None,
        );

        let search_box = target
            .bounding_box()
            .inflate(Self::SMASH_ATTACK_KNOCKBACK_RADIUS);
        for nearby in world.get_entities_in_aabb_matching(&search_box, |entity| {
            let Some(nearby) = entity.as_living_entity() else {
                return false;
            };
            Self::should_knockback(attacker, target, nearby)
        }) {
            let Some(nearby_living) = nearby.as_living_entity() else {
                continue;
            };
            let direction = nearby_living.position() - target.position();
            let knockback_power = Self::knockback_power(attacker, nearby_living, direction);
            if knockback_power <= 0.0 {
                continue;
            }

            let horizontal = if direction.length_squared() > 0.0 {
                direction.normalize() * knockback_power
            } else {
                DVec3::ZERO
            };
            nearby_living.push_impulse(DVec3::new(horizontal.x, 0.7, horizontal.z));
            if let Some(player) = nearby_living.as_player() {
                player.send_packet(CSetEntityMotion::new(
                    nearby_living.id(),
                    nearby_living.velocity(),
                ));
            }
        }
    }
}

impl ItemBehavior for MaceItem {
    fn get_item_damage_source(&self, attacker: &SharedEntity) -> Option<DamageSource> {
        let living = attacker.as_living_entity()?;
        Self::can_smash_attack(living)
            .then(|| DamageSource::direct(&vanilla_damage_types::MACE_SMASH, Arc::clone(attacker)))
    }

    fn get_attack_damage_bonus(
        &self,
        attacker: &dyn LivingEntity,
        victim: &dyn Entity,
        _base_damage: f32,
        damage_source: &DamageSource,
    ) -> f32 {
        if !Self::can_smash_attack(attacker) {
            return 0.0;
        }

        let fall_distance = attacker.fall_distance();
        let damage = Self::smash_attack_damage(fall_distance);
        let context = EnchantmentDamageContext::new(
            victim.entity_type(),
            Some(attacker.entity_type()),
            Some(attacker.entity_type()),
            damage_source,
        );
        let mut damage_per_fallen_block = 0.0;
        attacker.with_equipment_slot(EquipmentSlot::MainHand, &mut |item| {
            damage_per_fallen_block =
                enchantment_helper::modify_smash_damage_per_fallen_block(item, &context, 0.0);
        });
        (damage + f64::from(damage_per_fallen_block) * fall_distance) as f32
    }

    fn hurt_enemy(
        &self,
        _stack: &mut ItemStack,
        target: &dyn LivingEntity,
        attacker: &dyn LivingEntity,
    ) {
        if !Self::can_smash_attack(attacker) {
            return;
        }

        let velocity = attacker.velocity();
        attacker.set_velocity(DVec3::new(velocity.x, 0.01, velocity.z));
        attacker.set_ignore_fall_damage_from_current_impulse(
            true,
            Self::calculate_impact_position(attacker),
        );
        if let Some(player) = attacker.as_player() {
            player.send_packet(CSetEntityMotion::new(attacker.id(), attacker.velocity()));
        }

        if let Some(world) = attacker.level() {
            world.play_sound_at(
                Self::smash_sound(target, attacker),
                attacker.sound_source(),
                attacker.position(),
                1.0,
                1.0,
                None,
            );
        }
        Self::apply_knockback(attacker, target);
    }

    fn post_hurt_enemy(
        &self,
        _stack: &mut ItemStack,
        _target: &dyn LivingEntity,
        attacker: &dyn LivingEntity,
    ) {
        if Self::can_smash_attack(attacker) {
            attacker.reset_fall_distance();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::data_components::vanilla_components::KINETIC_WEAPON;
    use steel_registry::entity_type::EntityTypeRef;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::ChunkPos;
    use steel_utils::locks::SyncMutex;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::{EntityBase, LivingEntityBase, impl_test_downcast_type};
    use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;

    /// Minimal living test double with a world-backed base, so `hurt_enemy` can
    /// play its smash sound and run the vanilla area knockback.
    struct SmashTestMob {
        base: EntityBase,
        living_base: LivingEntityBase,
        health: SyncMutex<f32>,
    }

    impl SmashTestMob {
        fn new(id: i32, world: &Arc<World>, position: DVec3) -> Self {
            Self {
                base: EntityBase::new(
                    id,
                    position,
                    vanilla_entities::PIG.dimensions,
                    Arc::downgrade(world),
                ),
                living_base: LivingEntityBase::new(&vanilla_entities::PIG),
                health: SyncMutex::new(10.0),
            }
        }

        fn shared(id: i32, world: &Arc<World>, position: DVec3) -> SharedEntity {
            Arc::new(Self::new(id, world, position))
        }
    }

    impl_test_downcast_type!(SmashTestMob);

    impl Entity for SmashTestMob {
        fn base(&self) -> &EntityBase {
            &self.base
        }

        fn entity_type(&self) -> EntityTypeRef {
            &vanilla_entities::PIG
        }
    }

    impl LivingEntity for SmashTestMob {
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

    /// A loaded world with an attacker at x=0.5 and a target at x=1.5, both in
    /// chunk (0, 0) so entity queries see them.
    fn fixture(name: &'static str) -> (TestWorld, SharedEntity, SharedEntity) {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world(name);
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let attacker = SmashTestMob::shared(1, world, DVec3::new(0.5, 65.0, 0.5));
        let target = SmashTestMob::shared(2, world, DVec3::new(1.5, 65.0, 0.5));
        world
            .try_add_entity(Arc::clone(&attacker))
            .expect("attacker joins the world");
        world
            .try_add_entity(Arc::clone(&target))
            .expect("target joins the world");
        (world_fixture, attacker, target)
    }

    fn living(entity: &SharedEntity) -> &dyn LivingEntity {
        entity
            .as_living_entity()
            .expect("test mobs are living entities")
    }

    #[test]
    fn smash_damage_matches_the_vanilla_piecewise_curve() {
        // Vanilla `getAttackDamageBonus` branch points (fall distance 3 and 8).
        assert_eq!(MaceItem::smash_attack_damage(1.5), 6.0);
        assert_eq!(MaceItem::smash_attack_damage(2.0), 8.0);
        assert_eq!(MaceItem::smash_attack_damage(3.0), 12.0);
        assert_eq!(MaceItem::smash_attack_damage(5.0), 16.0);
        assert_eq!(MaceItem::smash_attack_damage(8.0), 22.0);
        assert_eq!(MaceItem::smash_attack_damage(10.0), 24.0);
    }

    #[test]
    fn smash_window_requires_fall_distance_above_the_threshold() {
        let (_world, attacker, _target) = fixture("mace_smash_window");
        let attacker_ref = living(&attacker);
        attacker_ref.set_fall_distance(1.5);
        assert!(
            !MaceItem::can_smash_attack(attacker_ref),
            "exactly 1.5 blocks is not a smash (vanilla uses `>`)"
        );
        attacker_ref.set_fall_distance(1.6);
        assert!(MaceItem::can_smash_attack(attacker_ref));
    }

    #[test]
    fn smash_bonus_is_zero_without_a_smash_window() {
        let (_world, attacker, target) = fixture("mace_no_smash_no_bonus");
        living(&attacker).set_fall_distance(1.4);
        let source = DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(&attacker));
        let bonus =
            MaceItem.get_attack_damage_bonus(living(&attacker), target.as_ref(), 0.0, &source);
        assert_eq!(bonus, 0.0);
    }

    #[test]
    fn smash_bonus_scales_with_fall_distance_past_the_threshold() {
        let (_world, attacker, target) = fixture("mace_smash_bonus_scales");
        let source = DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(&attacker));
        for (fall_distance, expected) in
            [(2.0_f64, 8.0_f32), (3.0, 12.0), (6.0, 18.0), (10.0, 24.0)]
        {
            living(&attacker).set_fall_distance(fall_distance);
            let bonus =
                MaceItem.get_attack_damage_bonus(living(&attacker), target.as_ref(), 0.0, &source);
            assert_eq!(
                bonus, expected,
                "fall distance {fall_distance} should add {expected}"
            );
        }
    }

    #[test]
    fn knockback_power_is_heavy_past_five_blocks() {
        // Vanilla `getKnockbackPower`: `(3.5 - d) * 0.7 * (fall > 5 ? 2 : 1)`.
        let light = MaceItem::knockback_power_for(5.0, 1.0, 0.0);
        let heavy = MaceItem::knockback_power_for(6.0, 1.0, 0.0);
        assert!((light - 1.75).abs() < 1e-12);
        assert!((heavy - 2.0 * light).abs() < 1e-12);
        assert_eq!(MaceItem::knockback_power_for(6.0, 1.0, 1.0), 0.0);
        assert!(MaceItem::knockback_power_for(6.0, 3.5, 0.0) <= 0.0);
    }

    #[test]
    fn should_knockback_skips_the_attacker_and_the_hit_target() {
        let (_world, attacker, target) = fixture("mace_knockback_predicate");
        let attacker_ref = living(&attacker);
        let target_ref = living(&target);
        assert!(!MaceItem::should_knockback(
            attacker_ref,
            target_ref,
            attacker_ref
        ));
        assert!(!MaceItem::should_knockback(
            attacker_ref,
            target_ref,
            target_ref
        ));
    }

    #[test]
    fn should_knockback_includes_a_nearby_bystander_and_excludes_distant_ones() {
        let (world_fixture, attacker, target) = fixture("mace_knockback_bystander");
        let world = &world_fixture.world;
        let near = SmashTestMob::shared(3, world, DVec3::new(2.5, 65.0, 0.5));
        let far = SmashTestMob::shared(4, world, DVec3::new(9.5, 65.0, 0.5));
        world
            .try_add_entity(Arc::clone(&near))
            .expect("nearby bystander joins the world");
        world
            .try_add_entity(Arc::clone(&far))
            .expect("distant bystander joins the world");
        assert!(MaceItem::should_knockback(
            living(&attacker),
            living(&target),
            living(&near)
        ));
        assert!(!MaceItem::should_knockback(
            living(&attacker),
            living(&target),
            living(&far)
        ));
    }

    #[test]
    fn hurt_enemy_smash_flattens_the_fall_but_leaves_the_hit_target_alone() {
        let (_world, attacker, target) = fixture("mace_hurt_enemy_smash");
        let attacker_ref = living(&attacker);
        let target_ref = living(&target);
        attacker_ref.set_fall_distance(6.0);
        attacker_ref.set_velocity(DVec3::new(1.0, -10.0, 0.0));
        target_ref.set_velocity(DVec3::ZERO);

        let mut stack = ItemStack::new(&vanilla_items::MACE);
        MaceItem.hurt_enemy(&mut stack, target_ref, attacker_ref);

        assert_eq!(
            attacker_ref.velocity().y,
            0.01,
            "vanilla sets the smasher's vertical delta to 0.01"
        );
        assert_eq!(attacker_ref.velocity().x, 1.0);
        // `MaceItem.knockback` calls `knockbackPredicate(attacker, target)`, which
        // rejects the attacker *and* the hit target (verified in the 26.3
        // bytecode): the victim's knockback comes from the attack path that
        // triggered `hurtEnemy`, never from the smash wave. Only bystanders
        // inside `SMASH_ATTACK_KNOCKBACK_RADIUS` are pushed.
        assert_eq!(
            target_ref.velocity(),
            DVec3::ZERO,
            "the smash wave must skip the hit target"
        );
        assert!(!MaceItem::should_knockback(
            attacker_ref,
            target_ref,
            target_ref
        ));

        MaceItem.post_hurt_enemy(&mut stack, target_ref, attacker_ref);
        assert_eq!(attacker_ref.fall_distance(), 0.0);
    }

    #[test]
    fn hurt_enemy_smash_knocks_back_a_nearby_bystander_too() {
        let (world_fixture, attacker, target) = fixture("mace_hurt_enemy_bystander");
        let world = &world_fixture.world;
        let bystander = SmashTestMob::shared(3, world, DVec3::new(2.5, 65.0, 0.5));
        world
            .try_add_entity(Arc::clone(&bystander))
            .expect("bystander joins the world");
        let attacker_ref = living(&attacker);
        attacker_ref.set_fall_distance(6.0);

        let mut stack = ItemStack::new(&vanilla_items::MACE);
        MaceItem.hurt_enemy(&mut stack, living(&target), attacker_ref);

        assert!(
            living(&bystander).velocity().length_squared() > 0.0,
            "the area smash knockback reaches bystanders inside 3.5 blocks"
        );
        assert!(
            living(&bystander).velocity().y >= 0.7,
            "vanilla pushes bystanders with a 0.7 upward delta"
        );
        assert!(
            !MaceItem::should_knockback(living(&attacker), living(&target), living(&target)),
            "the hit target is never in the smash wave"
        );
    }

    #[test]
    fn hurt_enemy_without_a_smash_window_leaves_velocity_alone() {
        let (_world, attacker, target) = fixture("mace_hurt_enemy_no_smash");
        let attacker_ref = living(&attacker);
        attacker_ref.set_fall_distance(1.0);
        attacker_ref.set_velocity(DVec3::new(1.0, -2.0, 0.0));
        let mut stack = ItemStack::new(&vanilla_items::MACE);
        MaceItem.hurt_enemy(&mut stack, living(&target), attacker_ref);
        assert_eq!(attacker_ref.velocity(), DVec3::new(1.0, -2.0, 0.0));
        assert_eq!(living(&target).velocity(), DVec3::ZERO);
        MaceItem.post_hurt_enemy(&mut stack, living(&target), attacker_ref);
        assert_eq!(attacker_ref.fall_distance(), 1.0);
    }

    #[test]
    fn mace_damage_source_is_smash_only_inside_the_window() {
        let (_world, attacker, _target) = fixture("mace_damage_source");
        let attacker_ref = living(&attacker);
        attacker_ref.set_fall_distance(6.0);
        let source = MaceItem
            .get_item_damage_source(&attacker)
            .expect("a smash attack uses the mace smash damage type");
        assert!(ptr::eq(
            source.damage_type,
            &raw const vanilla_damage_types::MACE_SMASH
        ));
        attacker_ref.set_fall_distance(1.0);
        assert!(MaceItem.get_item_damage_source(&attacker).is_none());
    }

    #[test]
    fn mace_has_no_kinetic_component_while_every_spear_does() {
        init_vanilla_registry();
        assert!(!ItemStack::new(&vanilla_items::MACE).has(KINETIC_WEAPON));
        for spear in [
            &vanilla_items::COPPER_SPEAR,
            &vanilla_items::DIAMOND_SPEAR,
            &vanilla_items::GOLDEN_SPEAR,
            &vanilla_items::IRON_SPEAR,
            &vanilla_items::NETHERITE_SPEAR,
            &vanilla_items::STONE_SPEAR,
            &vanilla_items::WOODEN_SPEAR,
        ] {
            assert!(
                spear.components.get(KINETIC_WEAPON).is_some(),
                "{} should carry the kinetic weapon component",
                spear.key
            );
        }
    }
}

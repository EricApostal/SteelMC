//! Trident item behavior (`TridentItem`).
//!
//! Mirrors vanilla `TridentItem`: holding a trident charges it for up to 72000
//! ticks; releasing after the 10-tick throw threshold either throws a
//! [`ThrownTridentEntity`] (carrying the stack's enchantments and loyalty) or —
//! when the stack carries `Riptide` and the player is in water or rain — starts
//! the vanilla auto-spin-attack. Either path damages the stack without ever
//! breaking it and awards the `used` stat.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{REGISTRY, RegistryExt as _, sound_events, vanilla_entities};
use steel_utils::{BlockPos, Downcast as _};

use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::item::{ItemBehavior, ItemUseAnimation};
use crate::entity::entities::ThrownTridentEntity;
use crate::entity::entities::objects::projectiles::{AbstractArrow as _, Pickup};
use crate::entity::projectile::spawn_projectile_from_rotation;
use crate::entity::{Entity, LivingEntity, SharedEntity, next_entity_id};
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::world::World;

/// Vanilla `TridentItem.THROW_THRESHOLD_TIME`: the minimum hold before the
/// trident can be released into a throw or spin attack.
const THROW_THRESHOLD_TIME: i32 = 10;

/// Vanilla `TridentItem.getUseDuration`.
const USE_DURATION: i32 = 72000;

/// Vanilla `TridentItem.PROJECTILE_SHOOT_POWER`: the thrown trident's speed.
const PROJECTILE_SHOOT_POWER: f32 = 2.5;

/// Vanilla `TridentItem.releaseUsing` throw uncertainty.
const THROW_UNCERTAINTY: f32 = 1.0;

/// Vanilla `Player.startAutoSpinAttack` duration, in ticks.
const SPIN_ATTACK_DURATION: i32 = 20;

/// Vanilla `TridentItem.BASE_DAMAGE`: passed to `startAutoSpinAttack` as the
/// spin-attack damage.
const SPIN_ATTACK_DAMAGE: f32 = 8.0;

/// Vanilla `TridentItem.releaseUsing` ground launch height (`1.1999999F`).
const GROUND_SPIN_LAUNCH: f64 = 1.199_999_9;

/// Behavior for the trident item.
#[item_behavior(class = "TridentItem")]
pub struct TridentItem;

impl ItemBehavior for TridentItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        // Vanilla `TridentItem.use`: a nearly broken trident cannot be used,
        // and a trident with `Riptide` only starts its use in water or rain.
        let stack = context.inv.with_item(|item| item.clone());
        if stack.next_damage_will_break() {
            return InteractionResult::Fail;
        }

        if spin_attack_strength(&stack) > 0.0 && !is_in_water_or_rain(context.player.as_ref()) {
            return InteractionResult::Fail;
        }

        context.player.start_using_item(context.hand);
        InteractionResult::Consume
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::Trident
    }

    fn get_use_duration(&self, _stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        USE_DURATION
    }

    fn release_using(
        &self,
        stack: &mut ItemStack,
        world: &Arc<World>,
        user: &dyn LivingEntity,
        time_left: i32,
    ) -> bool {
        // Vanilla `TridentItem.releaseUsing` only acts for players.
        let Some(player) = user.as_player() else {
            return false;
        };

        let time_held = USE_DURATION - time_left;
        if time_held < THROW_THRESHOLD_TIME {
            return false;
        }

        let riptide_strength = spin_attack_strength(stack);
        // Vanilla proceeds when the trident has no `Riptide`, or when the
        // player is wet and not riding; otherwise the release is a no-op.
        if riptide_strength > 0.0 && (!is_in_water_or_rain(player) || player.is_passenger()) {
            return false;
        }

        if stack.next_damage_will_break() {
            return false;
        }

        // Vanilla `EnchantmentHelper.pickHighestLevel(TRIDENT_SOUND)`, falling
        // back to `SoundEvents.TRIDENT_THROW`.
        let sound = trident_sound(stack);
        player.award_stat(&vanilla_stat_types::ITEM_USED, stack.item());

        // Steel is server-only, so vanilla's `level instanceof ServerLevel`
        // branch always applies.
        let has_infinite_materials = player.has_infinite_materials();
        stack.hurt_without_breaking(1, has_infinite_materials);

        if riptide_strength == 0.0 {
            throw_trident(world, player, stack, sound);
        } else {
            start_spin_attack(world, player, stack, riptide_strength, sound);
        }

        true
    }
}

/// Vanilla `TridentItem.releaseUsing`'s throw branch: consumes one trident,
/// shoots a [`ThrownTridentEntity`] from the player's rotation, marks it
/// creative-only for infinite-materials players and plays the throw sound at
/// the projectile.
fn throw_trident(world: &Arc<World>, player: &Player, stack: &mut ItemStack, sound: SoundEventRef) {
    let has_infinite_materials = player.has_infinite_materials();
    let thrown_item = stack.consume_and_return(1, has_infinite_materials);

    let Some(owner) = world.players.get_by_entity_id(player.id()) else {
        return;
    };
    let owner_entity: SharedEntity = owner;

    let mut spawn_item = thrown_item.clone();
    let spawned = spawn_projectile_from_rotation(
        world,
        player,
        &mut spawn_item,
        0.0,
        PROJECTILE_SHOOT_POWER,
        THROW_UNCERTAINTY,
        || {
            ThrownTridentEntity::from_owner(
                &vanilla_entities::TRIDENT,
                next_entity_id(),
                &owner_entity,
                Arc::downgrade(world),
                thrown_item.clone(),
            )
        },
    );

    let mut sound_source = (player.id(), player.position());
    if let Some(entity) = &spawned {
        sound_source = (entity.id(), entity.position());
        if has_infinite_materials
            && let Some(trident) = entity.downcast_ref::<ThrownTridentEntity>()
        {
            trident
                .abstract_arrow_base()
                .set_pickup(Pickup::CreativeOnly);
        }
    }

    // Vanilla: `level.playSound(null, trident, sound.value(), PLAYERS, 1.0F, 1.0F)`,
    // the entity overload that sends `ClientboundSoundEntityPacket`.
    world.play_sound_from_entity(
        sound,
        SoundSource::Players,
        sound_source.0,
        sound_source.1,
        1.0,
        1.0,
        None,
    );
}

/// Vanilla `TridentItem.releaseUsing`'s `Riptide` branch: launches the player
/// along their look direction, starts the auto-spin-attack and plays the
/// riptide sound.
fn start_spin_attack(
    world: &Arc<World>,
    player: &Player,
    stack: &ItemStack,
    riptide_strength: f32,
    sound: SoundEventRef,
) {
    // Vanilla computes the launch vector from the degrees yaw/pitch directly.
    let (yaw, pitch) = player.rotation();
    let yaw_radians = yaw.to_radians();
    let pitch_radians = pitch.to_radians();
    let xd = -yaw_radians.sin() * pitch_radians.cos();
    let yd = -pitch_radians.sin();
    let zd = yaw_radians.cos() * pitch_radians.cos();
    let distance = (xd * xd + yd * yd + zd * zd).sqrt();
    let scale = riptide_strength / distance;

    player.push_impulse(DVec3::new(
        f64::from(xd * scale),
        f64::from(yd * scale),
        f64::from(zd * scale),
    ));
    player.start_auto_spin_attack(SPIN_ATTACK_DURATION, SPIN_ATTACK_DAMAGE, stack.clone());

    if player.on_ground()
        && let Some(arc) = world.players.get_by_entity_id(player.id())
    {
        let _: Option<MoveResult> = arc.move_entity(
            MoverType::SelfMovement,
            DVec3::new(0.0, GROUND_SPIN_LAUNCH, 0.0),
        );
    }

    // Vanilla: `level.playSound(null, player, sound.value(), PLAYERS, 1.0F, 1.0F)`.
    world.play_sound_from_entity(
        sound,
        SoundSource::Players,
        player.id(),
        player.position(),
        1.0,
        1.0,
        None,
    );
}

/// Vanilla `EnchantmentHelper.getTridentSpinAttackStrength`: folds the
/// `trident_spin_attack_strength` value effect of every enchantment on `stack`
/// over a base of `0.0`.
#[must_use]
pub fn spin_attack_strength(stack: &ItemStack) -> f32 {
    stack
        .apply_unconditional_enchantment_value_effects(
            EnchantmentEffectComponent::TridentSpinAttackStrength,
            0.0,
        )
        .max(0.0)
}

/// Vanilla `EnchantmentHelper.pickHighestLevel(stack, TRIDENT_SOUND)`, falling
/// back to `SoundEvents.TRIDENT_THROW` when no enchantment carries the
/// component.
///
/// The picked entry is `list[min(level, list.len()) - 1]` of the enchantment
/// with the highest level.
#[must_use]
pub fn trident_sound(stack: &ItemStack) -> SoundEventRef {
    let mut picked: Option<(u32, SoundEventRef)> = None;

    if let Some(enchantments) = stack.get_enchantments() {
        for (key, level) in enchantments.iter() {
            if *level == 0 {
                continue;
            }
            let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
                continue;
            };
            let list = enchantment.effects.trident_sound;
            if list.is_empty() {
                continue;
            }

            if picked.is_none_or(|(best_level, _)| *level > best_level) {
                let index = (*level as usize).min(list.len()) - 1;
                picked = Some((*level, list[index]));
            }
        }
    }

    picked.map_or(&sound_events::ITEM_TRIDENT_THROW, |(_, sound)| sound)
}

/// Vanilla `Entity.isInWaterOrRain`: wet, or rained on at the feet or the top
/// of the hitbox.
fn is_in_water_or_rain(entity: &dyn Entity) -> bool {
    if entity.is_in_water() {
        return true;
    }

    let Some(world) = entity.level() else {
        return false;
    };

    let pos = entity.block_position();
    world.is_raining_at(pos)
        || world.is_raining_at(BlockPos::new(
            pos.x(),
            entity.bounding_box().max_y().floor() as i32,
            pos.z(),
        ))
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::entity_type::EntityTypeRef;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_items};
    use steel_utils::locks::SyncMutex;
    use steel_utils::types::{InteractionHand, UpdateFlags};
    use steel_utils::{BlockPos, ChunkPos, Identifier, WorldAabb};

    use super::*;
    use crate::behavior::{ITEM_BEHAVIORS, init_behaviors};
    use crate::entity::{
        Entity, EntityBase, LivingEntity, LivingEntityBase, impl_test_downcast_type,
    };
    use crate::player::Player;
    use crate::player::game_mode;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };
    use crate::world::World;

    /// A living test double: `Player.attack` needs a `LivingEntity` target whose
    /// health actually moves.
    struct SpinAttackTarget {
        base: EntityBase,
        living_base: LivingEntityBase,
        health: SyncMutex<f32>,
    }

    impl SpinAttackTarget {
        fn shared(id: i32, world: &Arc<World>, position: DVec3) -> SharedEntity {
            Arc::new(Self {
                base: EntityBase::new(
                    id,
                    position,
                    vanilla_entities::ZOMBIE.dimensions,
                    Arc::downgrade(world),
                ),
                living_base: LivingEntityBase::new(&vanilla_entities::ZOMBIE),
                health: SyncMutex::new(20.0),
            })
        }
    }

    impl_test_downcast_type!(SpinAttackTarget);

    impl Entity for SpinAttackTarget {
        fn base(&self) -> &EntityBase {
            &self.base
        }

        fn entity_type(&self) -> EntityTypeRef {
            &vanilla_entities::ZOMBIE
        }
    }

    impl LivingEntity for SpinAttackTarget {
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

    struct TridentFixture {
        world: TestWorld,
        player: Arc<Player>,
        player_entity: SharedEntity,
    }

    /// Builds a world with the surrounding full chunks loaded and a registered
    /// player holding `stack`.
    fn fixture(name: &'static str, stack: ItemStack) -> TridentFixture {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world(name);
        let world = &world_fixture.world;
        // `EntityFluidContact::has_fluid_and_loaded` walks the chunks around the
        // entity, so the 3x3 square has to be loaded for `is_in_water` to work.
        for offset_x in -1..=1 {
            for offset_z in -1..=1 {
                insert_ready_full_chunk(world, ChunkPos::new(offset_x, offset_z));
            }
        }

        let player = TestPlayerBuilder::new(Arc::clone(world), "TridentHolder", 31).build();
        let player_entity: SharedEntity = Arc::<Player>::clone(&player);
        player
            .try_set_position(DVec3::new(0.5, 65.0, 0.5))
            .expect("player spawns at a fixed test position");
        player.set_rotation((0.0, 0.0));
        world.players.insert(Arc::clone(&player));
        world
            .try_add_entity(Arc::clone(&player_entity))
            .expect("player joins the world");
        player
            .inventory
            .lock()
            .set_item_in_hand(InteractionHand::MainHand, stack);

        TridentFixture {
            world: world_fixture,
            player,
            player_entity,
        }
    }

    fn trident_with(enchantment: &'static str, level: u32) -> ItemStack {
        let mut stack = ItemStack::new(&vanilla_items::TRIDENT);
        stack.set_enchantments(&[(Identifier::vanilla_static(enchantment), level)], false);
        stack
    }

    /// Charges the held trident through the real interaction + tick loop.
    fn charge_to_threshold(fixture: &TridentFixture) {
        let result = game_mode::use_item(
            &fixture.player,
            &fixture.world.world,
            InteractionHand::MainHand,
        );
        assert_ne!(
            result,
            InteractionResult::Fail,
            "right-clicking a trident must start the charge (the riptide gate rejects dry players)"
        );
        assert!(
            result.consumes_action(),
            "right-clicking a trident consumes the interaction"
        );
        assert!(fixture.player.is_using_item());

        for _ in 0..THROW_THRESHOLD_TIME {
            Arc::clone(&fixture.player).tick();
        }
    }

    /// A charged release throws a trident at the vanilla speed along the look
    /// direction, carrying the stack's enchantments, and consumes the stack in
    /// survival.
    #[test]
    fn charged_release_throws_a_trident_with_the_vanilla_velocity_and_enchantments() {
        let fixture = fixture("trident_throw", trident_with("loyalty", 3));
        let world = &fixture.world.world;
        charge_to_threshold(&fixture);

        fixture.player.release_using_item(&fixture.player_entity);

        let tridents = world.get_entities_in_aabb_matching(
            &WorldAabb::new(-8.0, 60.0, -8.0, 8.0, 70.0, 8.0),
            |entity| entity.entity_type() == &vanilla_entities::TRIDENT,
        );
        assert_eq!(tridents.len(), 1, "exactly one trident is thrown");
        let thrown = &tridents[0];

        // Vanilla `ThrownTrident(Level, LivingEntity, ItemStack)` spawns at the
        // thrower's eye height minus 0.1.
        let expected_spawn = DVec3::new(0.5, fixture.player.get_eye_y() - 0.1, 0.5);
        assert!(
            (thrown.position() - expected_spawn).length() < 1e-9,
            "trident spawns at the thrower's eyes: {:?}",
            thrown.position()
        );

        // Vanilla `Projectile.spawnProjectileFromRotation(..., 2.5F, 1.0F)`.
        let velocity = thrown.velocity();
        assert!(
            (velocity.length() - f64::from(PROJECTILE_SHOOT_POWER)).abs() < 0.05,
            "thrown trident flies at the vanilla power: {velocity:?}"
        );
        assert!(
            velocity.x.abs() < 0.05 && velocity.y.abs() < 0.05 && velocity.z > 2.4,
            "thrown trident leaves along the thrower's look direction: {velocity:?}"
        );

        let entity = thrown
            .downcast_ref::<ThrownTridentEntity>()
            .expect("the spawned entity is a thrown trident");
        assert_eq!(
            entity.loyalty(),
            3,
            "the thrown trident carries the stack's enchantments"
        );

        let hand_is_empty = {
            let inventory = fixture.player.inventory.lock();
            inventory
                .get_item_in_hand(InteractionHand::MainHand)
                .is_empty()
        };
        assert!(hand_is_empty, "a survival throw consumes the trident");
    }

    /// Releasing damages the stack exactly once and never breaks it.
    #[test]
    fn release_damages_the_trident_without_ever_breaking_it() {
        let fixture = fixture("trident_durability", ItemStack::empty());
        let world = &fixture.world.world;
        let behavior = ITEM_BEHAVIORS.get_behavior(&vanilla_items::TRIDENT);

        let mut stack = ItemStack::new(&vanilla_items::TRIDENT);
        let max_damage = stack.get_max_damage();
        assert!(max_damage > 0, "the trident is a damageable item");

        assert!(behavior.release_using(
            &mut stack,
            world,
            fixture.player.as_ref(),
            USE_DURATION - THROW_THRESHOLD_TIME,
        ));
        assert!(
            stack.is_empty(),
            "the survival release consumes the single trident"
        );

        // The hand stack is emptied by the throw, so read the durability damage
        // from the stack that was actually thrown: vanilla `consumeAndReturn`
        // copies the already-damaged stack.
        let thrown = world.get_entities_in_aabb_matching(
            &WorldAabb::new(-8.0, 60.0, -8.0, 8.0, 70.0, 8.0),
            |entity| entity.entity_type() == &vanilla_entities::TRIDENT,
        );
        assert_eq!(thrown.len(), 1, "the release throws the damaged trident");
        let thrown_stack = thrown[0]
            .downcast_ref::<ThrownTridentEntity>()
            .expect("the spawned entity is a thrown trident")
            .abstract_arrow_base()
            .pickup_item_stack();
        assert_eq!(
            thrown_stack.get_damage_value(),
            1,
            "one release adds one point of durability damage"
        );
        assert_eq!(thrown_stack.count(), 1);

        // Vanilla pre-checks `nextDamageWillBreak` and `hurtWithoutBreaking`
        // clamps at `maxDamage - 1`, so a nearly broken trident never fires.
        let mut nearly_broken = ItemStack::new(&vanilla_items::TRIDENT);
        nearly_broken.set_damage_value(max_damage - 1);
        assert!(
            !behavior.release_using(
                &mut nearly_broken,
                world,
                fixture.player.as_ref(),
                USE_DURATION - THROW_THRESHOLD_TIME,
            ),
            "a trident that would break on use is not thrown"
        );
        assert_eq!(
            nearly_broken.get_damage_value(),
            max_damage - 1,
            "an unusable trident keeps its durability"
        );
        assert_eq!(
            world
                .get_entities_in_aabb_matching(
                    &WorldAabb::new(-8.0, 60.0, -8.0, 8.0, 70.0, 8.0),
                    |entity| entity.entity_type() == &vanilla_entities::TRIDENT,
                )
                .len(),
            1,
            "the nearly broken trident is not thrown"
        );
    }

    /// A trident with `Riptide`, released while in water, starts the
    /// auto-spin-attack, which damages the first living entity it sweeps
    /// through.
    #[test]
    fn riptide_in_water_performs_the_spin_attack_damage_on_a_nearby_entity() {
        let fixture = fixture("trident_riptide", trident_with("riptide", 1));
        let world = &fixture.world.world;

        // Submerge the player so `Entity.isInWaterOrRain` is true. The column is
        // tall enough that the player cannot sink out of it while charging.
        let water = vanilla_blocks::WATER.default_state();
        for y in 58..=68 {
            assert!(
                world.set_block(BlockPos::new(0, y, 0), water, UpdateFlags::UPDATE_ALL),
                "water column at y={y}"
            );
        }
        // Fluid contact is cached and refreshed once per tick; the water was
        // placed after the player existed, so tick once before the use.
        assert_eq!(
            world.get_block_state(BlockPos::new(0, 65, 0)),
            water,
            "the water column covers the player's block"
        );
        Arc::clone(&fixture.player).tick();
        let _ = fixture.player.refresh_fluid_contact();
        assert!(
            fixture.player.is_in_water(),
            "the player is submerged at {:?} (box {:?})",
            fixture.player.position(),
            fixture.player.bounding_box()
        );

        // One block away, so the very first spin-attack sweep of the travelled
        // box overlaps the target.
        let target = SpinAttackTarget::shared(32, world, DVec3::new(0.5, 65.0, 1.0));
        world
            .try_add_entity(Arc::clone(&target))
            .expect("target joins the world");
        let target_health = || {
            target
                .as_living_entity()
                .expect("the target is a living entity")
                .get_health()
        };
        assert_eq!(target_health(), 20.0, "the target starts unhurt");

        charge_to_threshold(&fixture);
        fixture.player.release_using_item(&fixture.player_entity);

        assert!(
            fixture.player.is_auto_spin_attack(),
            "riptide in water starts the auto-spin-attack"
        );

        // Vanilla `LivingEntity.aiStep` sweeps the travelled box every tick of
        // the 20-tick attack.
        for _ in 0..SPIN_ATTACK_DURATION {
            Arc::clone(&fixture.player).tick();
        }

        assert!(
            target_health() < 20.0,
            "the spin attack damages the entity it sweeps through: {}",
            target_health()
        );
        assert!(
            !fixture.player.is_auto_spin_attack(),
            "the spin attack ends once it lands a hit"
        );
    }
}

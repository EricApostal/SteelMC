//! Vanilla explosions (`Explosion`, `ServerExplosion`, `Level.explode`).
//!
//! Mirrors `/tmp/vanilla/src/world/level/Explosion.java`,
//! `ServerExplosion.java`, `ExplosionDamageCalculator.java`,
//! `EntityBasedExplosionDamageCalculator.java`,
//! `SimpleExplosionDamageCalculator.java` and the `Level`/`ServerLevel`
//! `explode` entry points in one module, because Steel's `World` is the loaded
//! level (`ServerLevel` + `Level`).
//!
//! Deliberate differences from vanilla (each one line):
//! - Vanilla `RandomSource.create()` per level is replaced by the project's
//!   usual per-call `LegacyRandom::from_seed(rand::random())` (`World` has no
//!   level-random field); vanilla's level random is itself non-deterministic.
//! - `EntityBasedExplosionDamageCalculator` is not modeled: no Steel entity
//!   overrides `getBlockExplosionResistance`/`shouldBlockExplode` yet, so the
//!   base calculator is behaviour-identical (see
//!   `Entity::ignoreExplosion`/`onExplosionHit` note below).
//! - `Entity.ignoreExplosion` and `Entity.onExplosionHit` are not yet hooks on
//!   the reserved `Entity` trait, so vanilla's per-entity opt-outs
//!   (`ItemEntity`, `ArmorStand`, `Warden`, vehicles, block-attached entities)
//!   and `ServerPlayer.onExplosionHit` are not applied.
//! - `Entity.doTeamsAllowDamage` needs the server scoreboard, which `World`
//!   cannot reach; damage is always applied, matching vanilla's no-team case.
//! - Block-entity contents still do not drop on explosion removal (same gap as
//!   `World::destroy_block_with_limit`).

use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packet_traits::{ClientPacket as _, EncodedPacket};
use steel_protocol::packets::game::{CExplode, ExplosionBlockParticle};
use steel_protocol::utils::ConnectionProtocol;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::fluid::FluidState;
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::{ParticleData, ParticleTypeRef};
use steel_registry::sound_event::SoundEventHolder;
use steel_registry::vanilla_damage_types;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_game_rules::{
    BLOCK_EXPLOSION_DROP_DECAY, MOB_EXPLOSION_DROP_DECAY, MOB_GRIEFING, TNT_EXPLOSION_DROP_DECAY,
};
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_blocks, vanilla_game_events,
    vanilla_particle_types,
};
use steel_utils::random::{Random as _, RandomSource, legacy_random::LegacyRandom};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, Identifier, WorldAabb};

use crate::behavior::fluid::FLUID_BEHAVIORS;
use crate::behavior::{BLOCK_BEHAVIORS, BlockLootContext};
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::explosives::PrimedTntEntity;
use crate::entity::{Entity, SharedEntity};
use crate::player::connection::NetworkConnection as _;
use crate::world::game_event::GameEventContext;
use crate::world::{ClipBlockShape, ClipFluid, World};

/// Vanilla `Explosion.BlockInteraction`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockInteraction {
    /// Blocks are left untouched.
    Keep,
    /// Blocks are destroyed, drops are not decayed by distance.
    Destroy,
    /// Blocks are destroyed, drops decay with distance.
    DestroyWithDecay,
    /// Blocks are untouched but block triggers still fire.
    TriggerBlock,
}

impl BlockInteraction {
    /// Vanilla `BlockInteraction.shouldAffectBlocklikeEntities`.
    #[must_use]
    pub const fn should_affect_blocklike_entities(self) -> bool {
        matches!(self, Self::Destroy | Self::DestroyWithDecay)
    }
}

/// Vanilla `Level.ExplosionInteraction`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExplosionInteraction {
    /// Blocks and entities are unaffected.
    None,
    /// Blocks are destroyed per the block-explosion game rules.
    Block,
    /// Blocks are destroyed per the mob-griefing rule.
    Mob,
    /// Blocks are destroyed per the TNT-explosion game rules.
    Tnt,
    /// Blocks are not destroyed, but triggers (e.g. buttons) still fire.
    Trigger,
}

/// Vanilla `Explosion` accessor surface exposed to damage calculators and hooks.
pub struct ExplosionInfo<'a> {
    /// Explosion center.
    pub center: DVec3,
    /// Explosion radius.
    pub radius: f32,
    /// Block interaction mode.
    pub block_interaction: BlockInteraction,
    /// Whether the explosion may place fire.
    pub fire: bool,
    /// Vanilla `Explosion.getDirectSourceEntity`.
    pub source: Option<&'a SharedEntity>,
    /// Vanilla `Explosion.getIndirectSourceEntity`.
    pub indirect_source: Option<&'a SharedEntity>,
    /// Damage source applied to hit entities.
    pub damage_source: &'a DamageSource,
}

/// Vanilla `ExplosionDamageCalculator`.
///
/// Every method has vanilla's base body as its default, matching
/// `ExplosionDamageCalculator`'s concrete implementation.
pub trait ExplosionDamageCalculator: Send + Sync {
    /// Vanilla `getBlockExplosionResistance`.
    fn block_explosion_resistance(
        &self,
        _explosion: &ExplosionInfo<'_>,
        _world: &World,
        _pos: BlockPos,
        state: BlockStateId,
        fluid: FluidState,
    ) -> Option<f32> {
        base_block_explosion_resistance(state, fluid)
    }

    /// Vanilla `shouldBlockExplode`.
    fn should_block_explode(
        &self,
        _explosion: &ExplosionInfo<'_>,
        _world: &World,
        _pos: BlockPos,
        _state: BlockStateId,
        _power: f32,
    ) -> bool {
        true
    }

    /// Vanilla `shouldDamageEntity`.
    fn should_damage_entity(&self, _explosion: &ExplosionInfo<'_>, _entity: &dyn Entity) -> bool {
        true
    }

    /// Vanilla `getKnockbackMultiplier`.
    fn knockback_multiplier(&self, _entity: &dyn Entity) -> f32 {
        1.0
    }

    /// Vanilla `getEntityDamageAmount`.
    fn entity_damage_amount(
        &self,
        explosion: &ExplosionInfo<'_>,
        entity: &dyn Entity,
        exposure: f32,
    ) -> f32 {
        let distance =
            entity.distance_to_sqr(explosion.center).sqrt() / f64::from(explosion.radius * 2.0);
        entity_damage_amount(explosion.radius, distance, exposure)
    }
}

/// Vanilla `ExplosionDamageCalculator` with no overrides.
pub struct DefaultExplosionDamageCalculator;

impl ExplosionDamageCalculator for DefaultExplosionDamageCalculator {}

/// The shared fallback instance for explosions without a source entity,
/// mirroring vanilla `ServerExplosion.EXPLOSION_DAMAGE_CALCULATOR`.
pub static DEFAULT_EXPLOSION_DAMAGE_CALCULATOR: DefaultExplosionDamageCalculator =
    DefaultExplosionDamageCalculator;

/// Vanilla `SimpleExplosionDamageCalculator`.
///
/// Used by the wind-charge family and by the `explode` enchantment effect.
pub struct SimpleExplosionDamageCalculator {
    /// Vanilla `explodesBlocks`.
    pub explodes_blocks: bool,
    /// Vanilla `damagesEntities`.
    pub damages_entities: bool,
    /// Vanilla `knockbackMultiplier`; `None` keeps the base multiplier.
    pub knockback_multiplier: Option<f32>,
    /// Vanilla `immuneBlocks`; a present tag makes non-tag blocks transparent to
    /// the ray and blocks the blast everywhere better than bedrock.
    pub immune_blocks: Option<&'static Identifier>,
}

impl ExplosionDamageCalculator for SimpleExplosionDamageCalculator {
    fn block_explosion_resistance(
        &self,
        _explosion: &ExplosionInfo<'_>,
        _world: &World,
        _pos: BlockPos,
        state: BlockStateId,
        fluid: FluidState,
    ) -> Option<f32> {
        let Some(immune_blocks) = self.immune_blocks else {
            return base_block_explosion_resistance(state, fluid);
        };
        if state.get_block().has_tag(immune_blocks) {
            // Vanilla `SimpleExplosionDamageCalculator`: 3600000.0F for immune blocks.
            Some(3_600_000.0)
        } else {
            None
        }
    }

    fn should_block_explode(
        &self,
        _explosion: &ExplosionInfo<'_>,
        _world: &World,
        _pos: BlockPos,
        _state: BlockStateId,
        _power: f32,
    ) -> bool {
        self.explodes_blocks
    }

    fn should_damage_entity(&self, _explosion: &ExplosionInfo<'_>, _entity: &dyn Entity) -> bool {
        self.damages_entities
    }

    fn knockback_multiplier(&self, entity: &dyn Entity) -> f32 {
        let creative_flying = entity
            .as_player()
            .is_some_and(|player| player.is_flying_player());
        if creative_flying {
            0.0
        } else {
            self.knockback_multiplier.unwrap_or(1.0)
        }
    }
}

/// Vanilla `ExplosionDamageCalculator.getBlockExplosionResistance`.
#[must_use]
pub fn base_block_explosion_resistance(state: BlockStateId, fluid: FluidState) -> Option<f32> {
    if state.is_air() && fluid.is_empty() {
        return None;
    }
    let block_resistance = state.get_block().config.explosion_resistance;
    let fluid_resistance = FLUID_BEHAVIORS
        .get_behavior(fluid.fluid_id)
        .explosion_resistance();
    Some(block_resistance.max(fluid_resistance))
}

/// Vanilla `ExplosionDamageCalculator.getEntityDamageAmount`.
#[must_use]
pub fn entity_damage_amount(radius: f32, distance: f64, exposure: f32) -> f32 {
    let double_radius = f64::from(radius) * 2.0;
    let power = (1.0 - distance) * f64::from(exposure);
    ((power * power + power) / 2.0 * 7.0 * double_radius + 1.0) as f32
}

/// Vanilla `Explosion.getDefaultDamageSource`.
///
/// `source` is the direct entity; the indirect source is the TNT owner, the
/// living entity itself, or a projectile's living owner.
#[must_use]
pub fn default_explosion_damage_source(source: Option<&SharedEntity>) -> DamageSource {
    let indirect = indirect_source_entity(source);
    let damage_type = if source.is_some() && indirect.is_some() {
        &vanilla_damage_types::PLAYER_EXPLOSION
    } else {
        &vanilla_damage_types::EXPLOSION
    };
    let mut damage_source = DamageSource::environment(damage_type);
    if let Some(source) = source {
        damage_source = damage_source.with_direct_entity(Arc::clone(source));
    }
    if let Some(indirect) = indirect {
        damage_source = damage_source.with_causing_entity(indirect);
    }
    damage_source
}

/// Vanilla `Explosion.getIndirectSourceEntity`.
#[must_use]
pub fn indirect_source_entity(source: Option<&SharedEntity>) -> Option<SharedEntity> {
    let source = source?;
    if let Some(primed_tnt) = source.as_ref().downcast_ref::<PrimedTntEntity>() {
        return primed_tnt.get_owner();
    }
    if source.as_living_entity().is_some() {
        return Some(Arc::clone(source));
    }
    if let Some(projectile) = source.as_projectile()
        && let Some(owner) = projectile.get_owner()
        && owner.as_living_entity().is_some()
    {
        return Some(owner);
    }
    None
}

/// Vanilla `DamageSources.badRespawnPointExplosion`.
#[must_use]
pub fn bad_respawn_point_damage_source(boom_pos: DVec3) -> DamageSource {
    DamageSource::environment(&vanilla_damage_types::BAD_RESPAWN_POINT)
        .with_source_position(boom_pos)
}

/// Vanilla `BlockBehaviour.onExplosionHit`'s default body.
///
/// Drops the block's loot (decayed by distance for `DESTROY_WITH_DECAY`),
/// spawns its after-break effects, removes the block, then runs the block's
/// `wasExploded` hook. Blocks that override `onExplosionHit` call this to keep
/// vanilla's `super.onExplosionHit` behaviour.
pub fn default_on_explosion_hit(
    state: BlockStateId,
    world: &Arc<World>,
    pos: BlockPos,
    explosion: &ExplosionInfo<'_>,
    on_drop: &mut dyn FnMut(ItemStack, BlockPos),
) {
    if state.is_air() || explosion.block_interaction == BlockInteraction::TriggerBlock {
        return;
    }

    let drop_experience = explosion.indirect_source.is_some_and(|source| {
        source
            .as_player()
            .is_some_and(|player| player.has_infinite_materials())
    });
    let context = BlockLootContext::new(world, pos)
        .with_entity(explosion.source.map(AsRef::as_ref))
        .with_explosion(explosion.radius);
    for item in context.get_drops(state) {
        if !item.is_empty() {
            on_drop(item, pos);
        }
    }
    BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .spawn_after_break(state, world, pos, &ItemStack::empty(), drop_experience);

    world.set_block(
        pos,
        vanilla_blocks::AIR.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .was_exploded(world, pos, explosion);
}

/// Vanilla `ServerExplosion`: one in-flight explosion.
struct ServerExplosion<'a> {
    world: &'a Arc<World>,
    source: Option<SharedEntity>,
    center: DVec3,
    radius: f32,
    fire: bool,
    block_interaction: BlockInteraction,
    damage_source: DamageSource,
    damage_calculator: &'a dyn ExplosionDamageCalculator,
    indirect_source: Option<SharedEntity>,
    random: RandomSource,
    /// Vanilla `hitPlayers`: player entity id to the knockback sent to that client.
    hit_players: Vec<(i32, DVec3)>,
}

impl<'a> ServerExplosion<'a> {
    fn info(&self) -> ExplosionInfo<'_> {
        ExplosionInfo {
            center: self.center,
            radius: self.radius,
            block_interaction: self.block_interaction,
            fire: self.fire,
            source: self.source.as_ref(),
            indirect_source: self.indirect_source.as_ref(),
            damage_source: &self.damage_source,
        }
    }

    /// Vanilla `ServerExplosion.interactsWithBlocks`.
    const fn interacts_with_blocks(&self) -> bool {
        !matches!(self.block_interaction, BlockInteraction::Keep)
    }

    /// Vanilla `ServerExplosion.isSmall`.
    const fn is_small(&self) -> bool {
        self.radius < 2.0 || !self.interacts_with_blocks()
    }

    /// Vanilla `ServerExplosion.canTriggerBlocks`.
    fn can_trigger_blocks(&self) -> bool {
        if self.block_interaction != BlockInteraction::TriggerBlock {
            return false;
        }
        let is_wind_charge = self.source.as_ref().is_some_and(|source| {
            source.entity_type() == &steel_registry::vanilla_entities::BREEZE_WIND_CHARGE
        });
        if is_wind_charge {
            self.world.get_game_rule(&MOB_GRIEFING)
        } else {
            true
        }
    }

    /// Vanilla `ServerExplosion.shouldAffectBlocklikeEntities`.
    fn should_affect_blocklike_entities(&self) -> bool {
        let mob_griefing = self.world.get_game_rule(&MOB_GRIEFING);
        let is_wind_charge = self.source.as_ref().is_some_and(|source| {
            source.entity_type() == &steel_registry::vanilla_entities::BREEZE_WIND_CHARGE
                || source.entity_type() == &steel_registry::vanilla_entities::WIND_CHARGE
        });
        if is_wind_charge {
            // Vanilla: wind charges never affect blocklike entities.
            return false;
        }
        mob_griefing || self.block_interaction.should_affect_blocklike_entities()
    }

    /// Vanilla `ServerExplosion.explode`.
    fn explode(&mut self) -> i32 {
        self.world.game_event_at(
            &vanilla_game_events::EXPLODE,
            self.center,
            &GameEventContext::new(self.source.as_deref(), None),
        );

        let mut to_blow = self.calculate_exploded_positions();
        self.hurt_entities();
        if self.interacts_with_blocks() {
            self.interact_with_blocks(&mut to_blow);
        }
        if self.fire {
            self.create_fire(&to_blow);
        }

        i32::try_from(to_blow.len()).unwrap_or(i32::MAX)
    }

    /// Vanilla `ServerExplosion.calculateExplodedPositions`.
    fn calculate_exploded_positions(&mut self) -> Vec<BlockPos> {
        let mut to_blow = Vec::new();
        let mut seen = rustc_hash::FxHashSet::default();
        for_each_shell_cell(&mut |xx, yy, zz| {
            if !(xx == 0 || xx == 15 || yy == 0 || yy == 15 || zz == 0 || zz == 15) {
                return;
            }
            let mut xd = (xx as f32 / 15.0 * 2.0 - 1.0) as f64;
            let mut yd = (yy as f32 / 15.0 * 2.0 - 1.0) as f64;
            let mut zd = (zz as f32 / 15.0 * 2.0 - 1.0) as f64;
            let d = (xd * xd + yd * yd + zd * zd).sqrt();
            xd /= d;
            yd /= d;
            zd /= d;

            let mut remaining_power = self.radius * (0.7 + self.random.next_f32() * 0.6);
            let mut xp = self.center.x;
            let mut yp = self.center.y;
            let mut zp = self.center.z;

            const STEP_SIZE: f32 = 0.3;

            while remaining_power > 0.0 {
                let pos = BlockPos::containing(xp, yp, zp);
                let state = self.world.get_block_state(pos);
                let fluid = state.get_fluid_state();
                if !self.world.is_in_world_bounds(pos) {
                    break;
                }

                let resistance = self.damage_calculator.block_explosion_resistance(
                    &self.info(),
                    self.world,
                    pos,
                    state,
                    fluid,
                );
                if let Some(resistance) = resistance {
                    remaining_power -= (resistance + 0.3) * 0.3;
                }

                if remaining_power > 0.0
                    && self.damage_calculator.should_block_explode(
                        &self.info(),
                        self.world,
                        pos,
                        state,
                        remaining_power,
                    )
                    && seen.insert(pos)
                {
                    to_blow.push(pos);
                }

                xp += xd * f64::from(STEP_SIZE);
                yp += yd * f64::from(STEP_SIZE);
                zp += zd * f64::from(STEP_SIZE);
                remaining_power -= 0.225_000_01;
            }
        });
        to_blow
    }

    /// Vanilla `ServerExplosion.hurtEntities`.
    fn hurt_entities(&mut self) {
        if self.radius < 1.0e-5 {
            return;
        }

        let double_radius = f64::from(self.radius) * 2.0;
        let sweep = WorldAabb::new(
            (self.center.x - double_radius - 1.0).floor(),
            (self.center.y - double_radius - 1.0).floor(),
            (self.center.z - double_radius - 1.0).floor(),
            (self.center.x + double_radius + 1.0).floor(),
            (self.center.y + double_radius + 1.0).floor(),
            (self.center.z + double_radius + 1.0).floor(),
        );

        for entity in self.world.get_entities_in_aabb(&sweep) {
            // Vanilla `level.getEntities(source, box)` excludes the direct source.
            if self
                .source
                .as_ref()
                .is_some_and(|source| Arc::ptr_eq(source, &entity))
            {
                continue;
            }

            let distance = entity.distance_to_sqr(self.center).sqrt() / double_radius;
            if distance > 1.0 {
                continue;
            }

            let is_primed_tnt = entity.as_ref().downcast_ref::<PrimedTntEntity>().is_some();
            let position = entity.position();
            let origin = if is_primed_tnt {
                position
            } else {
                DVec3::new(position.x, entity.get_eye_y(), position.z)
            };
            let direction = normalize(origin - self.center);

            let should_damage = self
                .damage_calculator
                .should_damage_entity(&self.info(), entity.as_ref());
            let knockback_multiplier = self.damage_calculator.knockback_multiplier(entity.as_ref());
            let exposure = if !should_damage && knockback_multiplier == 0.0 {
                0.0
            } else {
                seen_percent(self.world, self.center, entity.as_ref())
            };

            if should_damage {
                let amount = self.damage_calculator.entity_damage_amount(
                    &self.info(),
                    entity.as_ref(),
                    exposure,
                );
                entity.hurt(self.world, &self.damage_source, amount);
            }

            let knockback_resistance = entity.as_living_entity().map_or(0.0, |living| {
                living.attributes().lock().required_value(
                    steel_registry::vanilla_attributes::EXPLOSION_KNOCKBACK_RESISTANCE,
                )
            });
            let knockback_power = (1.0 - distance)
                * f64::from(exposure)
                * f64::from(knockback_multiplier)
                * (1.0 - knockback_resistance);
            let knockback = direction * knockback_power;
            entity.push_impulse(knockback);

            if let Some(player) = entity.as_player() {
                if !player.is_spectator()
                    && (!player.has_infinite_materials() || !player.is_flying_player())
                {
                    self.hit_players.push((player.id(), knockback));
                }
            } else if REGISTRY.entity_types.is_in_tag(
                entity.entity_type(),
                &EntityTypeTag::REDIRECTABLE_PROJECTILE,
            ) && let Some(projectile) = entity.as_projectile()
            {
                projectile.set_owner_entity(self.damage_source.causing_entity());
            }
        }
    }

    /// Vanilla `ServerExplosion.interactWithBlocks`.
    fn interact_with_blocks(&mut self, to_blow: &mut [BlockPos]) {
        shuffle(to_blow, &mut self.random);
        let mut stacks: Vec<(BlockPos, ItemStack)> = Vec::new();
        for pos in to_blow.iter().copied() {
            let state = self.world.get_block_state(pos);
            let mut on_drop = |stack: ItemStack, position: BlockPos| {
                add_or_append_stack(&mut stacks, stack, position);
            };
            BLOCK_BEHAVIORS
                .get_behavior(state.get_block())
                .on_explosion_hit(state, self.world, pos, &self.info(), &mut on_drop);
        }

        for (pos, stack) in stacks {
            self.world.pop_resource(pos, stack);
        }
    }

    /// Vanilla `ServerExplosion.createFire`.
    fn create_fire(&mut self, to_blow: &[BlockPos]) {
        for pos in to_blow.iter().copied() {
            if self.random.next_i32_bounded(3) != 0 {
                continue;
            }
            if !self.world.get_block_state(pos).is_air() {
                continue;
            }
            let below = pos.below();
            if !self.world.get_block_state(below).is_solid_render() {
                continue;
            }
            let fire = crate::behavior::blocks::FireBlock::get_state(self.world.as_ref(), pos);
            self.world.set_block(pos, fire, UpdateFlags::UPDATE_ALL);
        }
    }
}

/// Vanilla `ServerExplosion.getSeenPercent`.
fn seen_percent(world: &World, center: DVec3, entity: &dyn Entity) -> f32 {
    let bb = entity.bounding_box();
    let x_size = 1.0 / ((bb.max_x() - bb.min_x()) * 2.0 + 1.0);
    let y_size = 1.0 / ((bb.max_y() - bb.min_y()) * 2.0 + 1.0);
    let z_size = 1.0 / ((bb.max_z() - bb.min_z()) * 2.0 + 1.0);
    if x_size < 0.0 || y_size < 0.0 || z_size < 0.0 {
        return 0.0;
    }

    let x_offset = (1.0 - (1.0 / x_size).floor() * x_size) / 2.0;
    let z_offset = (1.0 - (1.0 / z_size).floor() * z_size) / 2.0;

    let mut hits = 0;
    let mut count = 0;
    let mut xx = 0.0;
    while xx <= 1.0 {
        let mut yy = 0.0;
        while yy <= 1.0 {
            let mut zz = 0.0;
            while zz <= 1.0 {
                let from = DVec3::new(
                    lerp(xx, bb.min_x(), bb.max_x()) + x_offset,
                    lerp(yy, bb.min_y(), bb.max_y()),
                    lerp(zz, bb.min_z(), bb.max_z()) + z_offset,
                );
                if world
                    .clip(from, center, ClipBlockShape::Collider, ClipFluid::None)
                    .is_miss()
                {
                    hits += 1;
                }
                count += 1;
                zz += z_size;
            }
            yy += y_size;
        }
        xx += x_size;
    }

    hits as f32 / count as f32
}

/// Vanilla `Vec3.normalize`.
fn normalize(vector: DVec3) -> DVec3 {
    if vector.length_squared() < 1.0e-4 {
        return DVec3::ZERO;
    }
    vector / vector.length()
}

/// Vanilla `Mth.lerp`.
const fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

/// Vanilla `Util.shuffle`.
fn shuffle<T>(items: &mut [T], random: &mut RandomSource) {
    for i in (1..items.len()).rev() {
        let j = random.next_i32_bounded(i as i32 + 1);
        let Ok(j) = usize::try_from(j) else {
            continue;
        };
        items.swap(i, j);
    }
}

/// Vanilla `ItemEntity.areMergable`.
fn are_mergable(this: &ItemStack, other: &ItemStack) -> bool {
    if other.count() + this.count() > other.max_stack_size() {
        return false;
    }
    ItemStack::is_same_item_same_components(this, other)
}

/// Vanilla `ServerExplosion.addOrAppendStack`.
fn add_or_append_stack(stacks: &mut Vec<(BlockPos, ItemStack)>, stack: ItemStack, pos: BlockPos) {
    let mut stack = stack;
    for (_, existing) in stacks.iter_mut() {
        if are_mergable(existing, &stack) {
            // Vanilla `ItemEntity.merge(stack, input, 16)`.
            let delta = (existing.max_stack_size().min(16) - existing.count()).min(stack.count());
            *existing = existing.copy_with_count(existing.count() + delta);
            let _ = stack.split(delta);
        }
        if stack.is_empty() {
            return;
        }
    }
    if !stack.is_empty() {
        stacks.push((pos, stack));
    }
}

/// Iterates vanilla's fixed 16x16x16 destination shell.
fn for_each_shell_cell(f: &mut impl FnMut(u8, u8, u8)) {
    for xx in 0..16u8 {
        for yy in 0..16u8 {
            for zz in 0..16u8 {
                f(xx, yy, zz);
            }
        }
    }
}

impl World {
    /// Vanilla `Level.explode(source, x, y, z, r, interaction)`.
    ///
    /// Blocks are destroyed, no fire is placed, and the damage source is
    /// derived from `source` via [`default_explosion_damage_source`].
    pub fn explode(
        self: &Arc<Self>,
        source: Option<SharedEntity>,
        x: f64,
        y: f64,
        z: f64,
        radius: f32,
        interaction: ExplosionInteraction,
    ) -> i32 {
        self.explode_with_fire(source, x, y, z, radius, false, interaction)
    }

    /// Vanilla `Level.explode(source, x, y, z, r, fire, interaction)`.
    pub fn explode_with_fire(
        self: &Arc<Self>,
        source: Option<SharedEntity>,
        x: f64,
        y: f64,
        z: f64,
        radius: f32,
        fire: bool,
        interaction: ExplosionInteraction,
    ) -> i32 {
        self.explode_full(source, None, None, x, y, z, radius, fire, interaction)
    }

    /// Vanilla's full `Level.explode` overload.
    ///
    /// `damage_source` falls back to [`default_explosion_damage_source`], and
    /// `damage_calculator` falls back to the base calculator (vanilla
    /// `DefaultExplosionDamageCalculator`, or the entity-based calculator when a
    /// source entity is present, which is behaviour-identical in Steel).
    ///
    /// Returns the number of destroyed blocks, like vanilla `ServerExplosion.explode`.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors vanilla Level.explode's parameter list"
    )]
    pub fn explode_full(
        self: &Arc<Self>,
        source: Option<SharedEntity>,
        damage_source: Option<DamageSource>,
        damage_calculator: Option<&dyn ExplosionDamageCalculator>,
        x: f64,
        y: f64,
        z: f64,
        radius: f32,
        fire: bool,
        interaction: ExplosionInteraction,
    ) -> i32 {
        let block_interaction = match interaction {
            ExplosionInteraction::None => BlockInteraction::Keep,
            ExplosionInteraction::Block => self.explosion_destroy_type(&BLOCK_EXPLOSION_DROP_DECAY),
            ExplosionInteraction::Mob => {
                if self.get_game_rule(&MOB_GRIEFING) {
                    self.explosion_destroy_type(&MOB_EXPLOSION_DROP_DECAY)
                } else {
                    BlockInteraction::Keep
                }
            }
            ExplosionInteraction::Tnt => self.explosion_destroy_type(&TNT_EXPLOSION_DROP_DECAY),
            ExplosionInteraction::Trigger => BlockInteraction::TriggerBlock,
        };

        let center = DVec3::new(x, y, z);
        let damage_source =
            damage_source.unwrap_or_else(|| default_explosion_damage_source(source.as_ref()));
        let mut explosion = ServerExplosion {
            world: self,
            indirect_source: indirect_source_entity(source.as_ref()),
            source,
            center,
            radius,
            fire,
            block_interaction,
            damage_source,
            damage_calculator: damage_calculator.unwrap_or(&DEFAULT_EXPLOSION_DAMAGE_CALCULATOR),
            random: RandomSource::Legacy(LegacyRandom::from_seed(rand::random::<u64>())),
            hit_players: Vec::new(),
        };

        let block_count = explosion.explode();
        let is_small = explosion.is_small();
        let hit_players = std::mem::take(&mut explosion.hit_players);
        let source_is_silent = explosion
            .source
            .as_ref()
            .is_some_and(|source| source.is_silent());
        let _ = explosion.can_trigger_blocks();
        let _ = explosion.should_affect_blocklike_entities();

        self.send_explosion(
            center,
            radius,
            block_count,
            is_small,
            &hit_players,
            source_is_silent,
        );
        block_count
    }

    /// Vanilla `Level.explode` with a `Vec3` center.
    pub fn explode_at(
        self: &Arc<Self>,
        source: Option<SharedEntity>,
        center: DVec3,
        radius: f32,
        fire: bool,
        interaction: ExplosionInteraction,
    ) -> i32 {
        self.explode_with_fire(
            source,
            center.x,
            center.y,
            center.z,
            radius,
            fire,
            interaction,
        )
    }

    /// Detonates an explosion centered on `source`, mirroring vanilla callers
    /// that pass the entity's own coordinates.
    pub fn explode_entity(
        self: &Arc<Self>,
        source: &SharedEntity,
        radius: f32,
        fire: bool,
        interaction: ExplosionInteraction,
    ) -> i32 {
        let position = source.position();
        self.explode_at(
            Some(Arc::clone(source)),
            position,
            radius,
            fire,
            interaction,
        )
    }

    /// Vanilla `ServerLevel.getDestroyType`.
    fn explosion_destroy_type(
        &self,
        game_rule: &steel_registry::game_rules::GameRule<bool>,
    ) -> BlockInteraction {
        if self.get_game_rule(game_rule) {
            BlockInteraction::DestroyWithDecay
        } else {
            BlockInteraction::Destroy
        }
    }

    /// Vanilla `ServerLevel.explode`'s `ClientboundExplodePacket` broadcast.
    fn send_explosion(
        &self,
        center: DVec3,
        radius: f32,
        block_count: i32,
        is_small: bool,
        hit_players: &[(i32, DVec3)],
        source_is_silent: bool,
    ) {
        const MAX_DISTANCE_SQ: f64 = 64.0 * 64.0;

        let explosion_particle: ParticleTypeRef = if is_small {
            &vanilla_particle_types::EXPLOSION
        } else {
            &vanilla_particle_types::EXPLOSION_EMITTER
        };
        let block_particles = vec![
            ExplosionBlockParticle {
                particle: ParticleData::simple(&vanilla_particle_types::POOF),
                scaling: 0.5,
                speed: 1.0,
                weight: 1,
            },
            ExplosionBlockParticle {
                particle: ParticleData::simple(&vanilla_particle_types::SMOKE),
                scaling: 1.0,
                speed: 1.0,
                weight: 1,
            },
        ];

        self.players.iter_players(|_, player| {
            if player.distance_to_sqr(center) >= MAX_DISTANCE_SQ {
                return true;
            }
            let player_id = player.id();
            let knockback = hit_players
                .iter()
                .find_map(|(id, knockback)| (*id == player_id).then_some(*knockback));
            let packet = CExplode {
                center,
                radius,
                block_count,
                player_knockback: knockback,
                explosion_particle: ParticleData::simple(explosion_particle),
                explosion_sound: SoundEventHolder::Registry(&sound_events::ENTITY_GENERIC_EXPLODE),
                block_particles: block_particles.clone(),
                play_sound: !source_is_silent,
            };
            if let Ok(encoded) =
                EncodedPacket::from_bare(packet, self.compression, ConnectionProtocol::Play)
            {
                player.connection.send_encoded(encoded);
            }
            true
        });
    }
}

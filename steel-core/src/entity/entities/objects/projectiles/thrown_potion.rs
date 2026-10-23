//! Shared `AbstractThrownPotion` behavior for thrown splash/lingering potions.
//!
//! Vanilla runs the common half of a thrown potion (splash range, water-related
//! entity effects, block extinguishing and the potion splash events) in the
//! abstract `AbstractThrownPotion` class, then calls the abstract
//! `onHitAsPotion` hook that [`ThrownSplashPotionEntity`] and
//! [`ThrownLingeringPotionEntity`] implement. Steel mirrors that as
//! [`AbstractThrownPotion`]: a trait over the
//! `Projectile → ThrowableProjectile → ThrowableItemProjectile` stack.
//!
//! This module also owns the potion-data helpers that vanilla keeps on
//! `PotionContents` (`hasEffects`, `hasInstantEffects`, `getColor` and the
//! potion-tag test) plus the distance-falloff duration scaling shared with the
//! lingering cloud. Those belong upstream once `PotionContents` grows them; see
//! the doc comments on each helper.
//!
//! [`ThrownSplashPotionEntity`]: super::thrown_splash_potion::ThrownSplashPotionEntity
//! [`ThrownLingeringPotionEntity`]: super::thrown_lingering_potion::ThrownLingeringPotionEntity

use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::data_components::PotionContents;
use steel_registry::data_components::vanilla_components::{POTION_CONTENTS, POTION_DURATION_SCALE};
use steel_registry::item_stack::ItemStack;
use steel_registry::level_events::{
    PARTICLES_INSTANT_POTION_SPLASH, PARTICLES_SPELL_POTION_SPLASH, SOUND_EXTINGUISH_FIRE,
    SOUND_INSTANT_POTION_SPLASH, SOUND_SPELL_POTION_SPLASH,
};
use steel_registry::sound_events::BLOCK_CANDLE_EXTINGUISH;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_damage_types;
use steel_registry::vanilla_game_events;
use steel_registry::vanilla_potion_tags::PotionTag;
use steel_registry::{REGISTRY, TaggedRegistryExt, vanilla_entities};
use steel_utils::BlockPos;
use steel_utils::types::UpdateFlags;

use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, Projectile, ProjectileHit, RemovalReason, SharedEntity, ThrowableItemProjectile,
};
use crate::world::game_event::GameEventContext;
use crate::world::{ClipHitResult, World};

/// Vanilla `AbstractThrownPotion.SPLASH_RANGE`.
pub const SPLASH_RANGE: f64 = 4.0;

/// Vanilla `AbstractThrownPotion.SPLASH_RANGE_SQ`.
pub const SPLASH_RANGE_SQ: f64 = 16.0;

/// Vanilla `AbstractThrownPotion.getDefaultGravity`.
pub const THROWN_POTION_GRAVITY: f64 = 0.05;

/// Vanilla `ThrowableItemProjectile` spawn offset below the thrower's eye.
pub const THROWN_ITEM_SPAWN_EYE_OFFSET: f64 = 0.1;

/// Vanilla `AbstractCandleBlock`/`CampfireBlock` lit-state property.
const LIT: &BoolProperty = &BlockStateProperties::LIT;

/// Vanilla `PotionContents.getColorOr`'s default potion color.
pub const DEFAULT_POTION_COLOR: i32 = -13083194;

/// Behavior shared by vanilla `AbstractThrownPotion` subclasses.
pub trait AbstractThrownPotion: ThrowableItemProjectile {
    /// Vanilla `AbstractThrownPotion.getDefaultGravity`.
    fn thrown_potion_default_gravity(&self) -> f64 {
        THROWN_POTION_GRAVITY
    }

    /// Vanilla `AbstractThrownPotion.onHitBlock`: base block-hit handling then,
    /// for a `douses_fire` potion, extinguishing fire around the impact.
    fn on_hit_block_thrown_potion(self: Arc<Self>, hit: &ClipHitResult) {
        Arc::clone(&self).projectile_on_hit_block(hit);

        let Some(world) = self.level() else {
            return;
        };
        let contents = potion_contents(&self.get_item());
        if !potion_is(&contents, &PotionTag::DOUSES_FIRE) {
            return;
        }

        // Vanilla evaluates the offsets inside the `blockHitPos.relative(hitDirection)`
        // chain: the next block on the hit face, the hit block itself, then the four
        // horizontal neighbors of the next block.
        let effect_pos = hit.direction.relative(hit.block_pos);
        douse_fire(&world, effect_pos, self.as_entity_event_source());
        douse_fire(&world, hit.block_pos, self.as_entity_event_source());
        for direction in steel_utils::Direction::HORIZONTAL {
            douse_fire(
                &world,
                direction.relative(effect_pos),
                self.as_entity_event_source(),
            );
        }
    }

    /// Vanilla `AbstractThrownPotion.onHit`: base hit handling, the
    /// water-related entity effects, the subclass potion application, the splash
    /// level events, then discard.
    fn on_hit_thrown_potion(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);

        let Some(world) = self.level() else {
            return;
        };

        let potion_item = self.get_item();
        let contents = potion_contents(&potion_item);
        self.affect_entities_around(&world, &contents);
        if potion_has_effects(&contents) {
            Arc::clone(&self).on_hit_as_potion(&world, &potion_item, hit);
        }

        let block_pos = self.block_position();
        if potion_has_instant_effects(&contents) {
            world.level_event(
                PARTICLES_INSTANT_POTION_SPLASH,
                block_pos,
                potion_color(&contents),
                None,
            );
            if !self.is_silent() {
                world.level_event(SOUND_INSTANT_POTION_SPLASH, block_pos, 0, None);
            }
        } else {
            world.level_event(
                PARTICLES_SPELL_POTION_SPLASH,
                block_pos,
                potion_color(&contents),
                None,
            );
            if !self.is_silent() {
                world.level_event(SOUND_SPELL_POTION_SPLASH, block_pos, 0, None);
            }
        }

        self.set_removed(RemovalReason::Discarded);
    }

    /// Vanilla `AbstractThrownPotion.affectEntitiesAround`: water potions hurt
    /// water-sensitive entities and extinguish burning ones.
    fn affect_entities_around(&self, world: &Arc<World>, contents: &PotionContents) {
        let hurts_water_sensitive_entities =
            potion_is(contents, &PotionTag::HURTS_WATER_SENSITIVE_ENTITIES);
        let extinguishes_entities = potion_is(contents, &PotionTag::EXTINGUISHES_ENTITIES);
        if !hurts_water_sensitive_entities && !extinguishes_entities {
            return;
        }

        // Vanilla filters with `WATER_SENSITIVE_OR_ON_FIRE`.
        let aabb = self
            .bounding_box()
            .inflate_xyz(SPLASH_RANGE, 2.0, SPLASH_RANGE);
        let entities = world.get_entities_in_aabb_matching(&aabb, |entity| {
            is_sensitive_to_water(entity) || entity.is_on_fire()
        });

        for entity in entities {
            if self.distance_to_sqr(entity.position()) >= SPLASH_RANGE_SQ {
                continue;
            }

            if hurts_water_sensitive_entities && is_sensitive_to_water(entity.as_ref()) {
                // Vanilla `damageSources().indirectMagic(this, this.getOwner())`.
                let mut source = DamageSource::environment(&vanilla_damage_types::INDIRECT_MAGIC)
                    .with_direct_entity(Arc::clone(&entity));
                if let Some(owner) = self.get_owner() {
                    source = source.with_causing_entity(owner);
                }
                entity.hurt(world, &source, 1.0);
            }

            if extinguishes_entities && entity.is_on_fire() && entity.is_alive() {
                extinguish_entity_fire(entity.as_ref());
            }
        }

        // Vanilla also rehydrates axolotls inside the same box. Steel has no
        // axolotl entity or `rehydrate` capability yet, so the
        // `rehydrates_axolotls` branch has nothing to act on (reported gap).
    }

    /// Vanilla `AbstractThrownPotion.onHitAsPotion`.
    fn on_hit_as_potion(
        self: Arc<Self>,
        world: &Arc<World>,
        potion_item: &ItemStack,
        hit: &ProjectileHit,
    );
}

/// Returns vanilla `PotionContents.hasEffects` for a potion-bearing stack.
#[must_use]
pub fn potion_has_effects(contents: &PotionContents) -> bool {
    !contents.custom_effects().is_empty()
        || contents
            .potion()
            .is_some_and(|potion| !potion.value().effects.is_empty())
}

/// Returns vanilla `Potion.hasInstantEffects` for a potion's base effects.
///
/// Vanilla asks each `MobEffect` whether it is instantaneous; Steel stores that
/// on the registered behavior, so an effect is instantaneous exactly when its
/// behavior exposes the `InstantaneousMobEffect` half.
#[must_use]
pub fn potion_has_instant_effects(contents: &PotionContents) -> bool {
    contents.all_effects().iter().any(|effect| {
        crate::behavior::MOB_EFFECT_BEHAVIORS
            .get_behavior(effect.effect())
            .as_instantaneous()
            .is_some()
    })
}

/// Returns whether `contents` is `tag` without custom effects, mirroring vanilla
/// `PotionContents.is(TagKey<Potion>)`.
#[must_use]
pub fn potion_is(contents: &PotionContents, tag: &steel_utils::Identifier) -> bool {
    if !contents.custom_effects().is_empty() {
        return false;
    }
    let Some(potion) = contents.potion() else {
        return false;
    };
    REGISTRY.potions.is_in_tag(potion.value(), tag)
}

/// Returns vanilla `PotionContents.getColor`, including its default color.
#[must_use]
pub fn potion_color(contents: &PotionContents) -> i32 {
    if let Some(custom_color) = contents.custom_color() {
        return custom_color;
    }

    // Vanilla `PotionContents.getColorOptional`: a weighted average of the
    // visible effects' RGB channels, weighted by amplifier + 1.
    let mut red = 0i32;
    let mut green = 0i32;
    let mut blue = 0i32;
    let mut total_weight = 0i32;
    for effect in contents.all_effects() {
        if !effect.show_particles() {
            continue;
        }
        let color = effect.effect().color;
        let amplifier = effect.amplifier() + 1;
        red += amplifier * i32::from(color.red());
        green += amplifier * i32::from(color.green());
        blue += amplifier * i32::from(color.blue());
        total_weight += amplifier;
    }

    if total_weight == 0 {
        return DEFAULT_POTION_COLOR;
    }
    let red = (red / total_weight) as u32;
    let green = (green / total_weight) as u32;
    let blue = (blue / total_weight) as u32;
    (0xff00_0000u32 | (red << 16) | (green << 8) | blue) as i32
}

/// Returns the `potion_contents` component of a thrown potion stack.
#[must_use]
pub fn potion_contents(stack: &ItemStack) -> PotionContents {
    stack.get_or_default(POTION_CONTENTS, PotionContents::empty())
}

/// Returns the `potion_duration_scale` component of a thrown potion stack.
#[must_use]
pub fn potion_duration_scale(stack: &ItemStack) -> f32 {
    stack.get_or_default(POTION_DURATION_SCALE, 1.0)
}

/// Vanilla `LivingEntity.isSensitiveToWater`, which `Blaze`, `Enderman`,
/// `SnowGolem`, and `Strider` each override to `true`.
///
/// `LivingEntity` has no `is_sensitive_to_water` capability yet, so the vanilla
/// overrides are mirrored here by entity type. Move this to a trait method with
/// those four implementations once the capability lands.
#[must_use]
pub fn is_sensitive_to_water(entity: &dyn Entity) -> bool {
    let entity_type = entity.entity_type();
    entity_type == &vanilla_entities::BLAZE
        || entity_type == &vanilla_entities::ENDERMAN
        || entity_type == &vanilla_entities::SNOW_GOLEM
        || entity_type == &vanilla_entities::STRIDER
}

/// Vanilla `Entity.extinguishFire`: the extinguish sound for a burning entity,
/// then clearing its fire.
///
/// The sound is client-local in vanilla (`Entity.playSound`), so only the fire
/// state is server-owned here.
pub fn extinguish_entity_fire(entity: &dyn Entity) {
    entity.clear_fire();
}

/// Vanilla `AbstractThrownPotion.douseFire`: destroys a burning fire block,
/// extinguishes a lit candle, or douses a lit campfire.
fn douse_fire(world: &Arc<World>, pos: BlockPos, source: &dyn Entity) {
    let state = world.get_block_state(pos);
    let block = state.get_block();

    if block.has_tag(&BlockTag::FIRE) {
        world.destroy_block_by_entity(pos, false, source);
        return;
    }

    let lit = state.try_get_value(LIT).is_some_and(|lit| lit);
    if !lit {
        return;
    }

    // `CANDLES`/`CANDLE_CAKES` cover vanilla's `AbstractCandleBlock` instances.
    if block.has_tag(&BlockTag::CANDLES) || block.has_tag(&BlockTag::CANDLE_CAKES) {
        let extinguished = state.set_value(LIT, false);
        world.set_block(pos, extinguished, UpdateFlags::UPDATE_ALL_IMMEDIATE);
        world.play_sound(
            &BLOCK_CANDLE_EXTINGUISH,
            SoundSource::Blocks,
            pos,
            1.0,
            1.0,
            None,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(None, Some(extinguished)),
        );
        return;
    }

    if block.has_tag(&BlockTag::CAMPFIRES) {
        world.level_event(SOUND_EXTINGUISH_FIRE, pos, 0, None);
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(Some(source), Some(state)),
        );
        world.set_block(pos, state.set_value(LIT, false), UpdateFlags::UPDATE_ALL);
    }
}

/// Scales `duration` by `scale` like vanilla `MobEffectInstance.withScaledDuration`:
/// the infinite-duration sentinel (`-1`) and a zero duration are returned
/// unchanged, and a finite result never rounds below one tick.
///
/// Mirrors the private `potion_contents::scale_effect_duration`, which the
/// projectile and cloud code cannot reach; unify them once that helper is
/// `pub(crate)`.
#[must_use]
pub fn scale_effect_duration(duration: i32, scale: f32) -> i32 {
    if duration == -1 || duration == 0 {
        return duration;
    }
    ((f64::from(duration) * f64::from(scale)).floor() as i32).max(1)
}

/// Returns the splash effect box of a thrown potion for one hit: the projectile
/// box moved to the hit location, inflated by vanilla's splash range.
#[must_use]
pub fn splash_effect_box(projectile: &dyn Entity, hit_location: DVec3) -> steel_utils::WorldAabb {
    projectile
        .bounding_box()
        .translate(hit_location - projectile.position())
        .inflate_xyz(SPLASH_RANGE, 2.0, SPLASH_RANGE)
}

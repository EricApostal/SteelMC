//! Vanilla `EyeblossomBlock` and the local environment-attribute bridge.
//!
//! Open and closed eyeblossoms are separate blocks that swap at day/night
//! boundaries, driven by the `minecraft:gameplay/eyeblossom_open` environment
//! attribute, and poison bees that fly through them.
//!
//! Deviation: Steel has no public environment-attribute getter (`world::environment`
//! is private to `world`), so [`bool_attribute`] / [`float_attribute`] sample the
//! dimension's timelines directly (owner: `world/environment`). Vanilla's client-side
//! `animateTick` idle sound is not implemented: the block trait has no client
//! animation hook (owner: `behavior/block`).

use std::sync::Arc;
use std::str::FromStr as _;

use glam::DVec3;
use rand::{Rng, RngExt};
use steel_macros::block_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::particle_type::{ParticleData, TrailParticleOption};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::timeline::{Ease, KeyframeValue, Track, TimelineRef};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{
    REGISTRY, RegistryExt as _, TaggedRegistryExt as _, sound_events, vanilla_blocks,
    vanilla_entities, vanilla_game_events, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::types::{Difficulty, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Direction, Identifier, RgbColor};

use crate::behavior::block::BlockBehavior;
use crate::behavior::blocks::vegetation::vegetation_block::survival_update_shape;
use crate::behavior::blocks::vegetation::Vegetation;
use crate::behavior::context::BlockPlaceContext;
use crate::entity::{Entity, InsideBlockEffectCollector, LivingEntity, MobEffectInstance};
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, ScheduledTickAccess, World};

use super::{BlockRef, default_surviving_state, survives_on_tag};

/// Vanilla `EnvironmentAttributes.EYEBLOSSOM_OPEN` key.
pub(crate) const EYEBLOSSOM_OPEN_ATTRIBUTE: &str = "minecraft:gameplay/eyeblossom_open";

#[derive(Clone, Copy)]
/// Vanilla open/closed eyeblossom type from `classes.json`.
pub enum EyeblossomType {
    /// Emits open-eyeblossom effects and transforms closed at daytime.
    Open,
    /// Emits closed-eyeblossom effects and transforms open at nighttime.
    Closed,
}

impl EyeblossomType {
    /// Vanilla `Type.open`.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    /// Vanilla `Type.fromBoolean`.
    #[must_use]
    pub const fn from_boolean(open: bool) -> Self {
        if open { Self::Open } else { Self::Closed }
    }

    /// Vanilla `Type.transform`.
    #[must_use]
    pub const fn transform(self) -> Self {
        Self::from_boolean(!self.is_open())
    }

    /// Vanilla `Type.block`.
    #[must_use]
    pub const fn block(self) -> BlockRef {
        if self.is_open() {
            &vanilla_blocks::OPEN_EYEBLOSSOM
        } else {
            &vanilla_blocks::CLOSED_EYEBLOSSOM
        }
    }

    /// Vanilla `Type.state`.
    #[must_use]
    pub fn state(self) -> BlockStateId {
        self.block().default_state()
    }

    /// Vanilla `Type.longSwitchSound` (random-tick transform sound).
    #[must_use]
    pub const fn long_switch_sound(self) -> SoundEventRef {
        if self.is_open() {
            &sound_events::EYEBLOSSOM_OPEN_LONG
        } else {
            &sound_events::EYEBLOSSOM_CLOSE_LONG
        }
    }

    /// Vanilla `Type.shortSwitchSound` (scheduled-tick transform sound).
    #[must_use]
    pub const fn short_switch_sound(self) -> SoundEventRef {
        if self.is_open() {
            &sound_events::EYEBLOSSOM_OPEN
        } else {
            &sound_events::EYEBLOSSOM_CLOSE
        }
    }

    /// Vanilla `Type.particleColor`.
    #[must_use]
    pub const fn particle_color(self) -> i32 {
        if self.is_open() { 16_545_810 } else { 6_250_335 }
    }

    /// Vanilla `Type.spawnTransformParticle`.
    pub fn spawn_transform_particle(
        self,
        world: &Arc<World>,
        pos: BlockPos,
        rng: &mut dyn Rng,
    ) {
        let start = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()) + 0.5,
            f64::from(pos.z()) + 0.5,
        );
        let lifetime = 0.5 + rng.random::<f64>();
        let velocity = DVec3::new(
            rng.random::<f64>() - 0.5,
            rng.random::<f64>() + 1.0,
            rng.random::<f64>() - 0.5,
        );
        let target = start + velocity * lifetime;
        let particle = TrailParticleOption::new(
            target,
            RgbColor::new(self.particle_color()),
            (20.0 * lifetime) as i32,
        );
        world.send_particles(
            ParticleData::new(&vanilla_particle_types::TRAIL, particle),
            start,
            1,
            DVec3::ZERO,
            0.0,
        );
    }
}

/// Behavior for vanilla eyeblossom blocks.
#[block_behavior]
pub struct EyeblossomBlock {
    block: BlockRef,
    #[json_arg(r#enum = "EyeblossomType", json = "type")]
    eyeblossom_type: EyeblossomType,
}

impl EyeblossomBlock {
    /// Creates a new eyeblossom behavior.
    #[must_use]
    pub const fn new(block: BlockRef, eyeblossom_type: EyeblossomType) -> Self {
        Self {
            block,
            eyeblossom_type,
        }
    }

    /// Vanilla `EyeblossomBlock.tryChangingState`.
    fn try_changing_state(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        rng: &mut dyn Rng,
    ) -> bool {
        let should_be_open = bool_attribute(
            world,
            EYEBLOSSOM_OPEN_ATTRIBUTE,
            self.eyeblossom_type.is_open(),
        );
        if should_be_open == self.eyeblossom_type.is_open() {
            return false;
        }

        let new_type = self.eyeblossom_type.transform();
        world.set_block(pos, new_type.state(), UpdateFlags::UPDATE_ALL);
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(None, Some(state)),
        );
        new_type.spawn_transform_particle(world, pos, rng);

        for nearby_pos in
            BlockPos::between_closed(pos.offset(-3, -2, -3), pos.offset(3, 2, 3))
        {
            if world.get_block_state(nearby_pos) != state {
                continue;
            }

            let dx = f64::from(nearby_pos.x() - pos.x());
            let dy = f64::from(nearby_pos.y() - pos.y());
            let dz = f64::from(nearby_pos.z() - pos.z());
            let distance = dx.mul_add(dx, dy.mul_add(dy, dz * dz)).sqrt();
            let delay = rng.random_range((distance * 5.0) as i32..=(distance * 10.0) as i32);
            world.schedule_block_tick_default(nearby_pos, state.get_block(), delay);
        }
        true
    }
}

impl BlockBehavior for EyeblossomBlock {
    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        survives_on_tag(world, pos, &BlockTag::SUPPORTS_VEGETATION)
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        survival_update_shape(self, state, world, pos)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        default_surviving_state(self.block, self, context)
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let mut rng = rand::rng();
        if self.try_changing_state(state, world, pos, &mut rng) {
            world.play_block_sound(
                self.eyeblossom_type.transform().long_switch_sound(),
                pos,
                1.0,
                1.0,
                None,
            );
        }
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let mut rng = rand::rng();
        if self.try_changing_state(state, world, pos, &mut rng) {
            world.play_block_sound(
                self.eyeblossom_type.transform().short_switch_sound(),
                pos,
                1.0,
                1.0,
                None,
            );
        }
    }

    fn entity_inside(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        _pos: BlockPos,
        entity: &dyn Entity,
        _effect_collector: &mut InsideBlockEffectCollector,
        _is_precise: bool,
    ) {
        if world.difficulty() == Difficulty::Peaceful
            || entity.entity_type() != &vanilla_entities::BEE
            || !attracts_bees(state)
        {
            return;
        }

        let Some(bee) = entity.as_living_entity() else {
            return;
        };
        if bee.has_mob_effect(&vanilla_mob_effects::POISON) {
            return;
        }

        // Vanilla `EyeblossomBlock.getBeeInteractionEffect` is always poison,
        // regardless of the open/closed type's own effect.
        bee.add_mob_effect(MobEffectInstance::with_duration(
            &vanilla_mob_effects::POISON,
            25,
            0,
        ));
    }
}

impl Vegetation for EyeblossomBlock {}

/// Vanilla `Bee.attractsBees`: the block must be `#minecraft:bee_attractive`, not
/// waterlogged, and a sunflower only when it is the upper half.
fn attracts_bees(state: BlockStateId) -> bool {
    if !state.get_block().has_tag(&BlockTag::BEE_ATTRACTIVE) {
        return false;
    }
    if state
        .try_get_value(&BlockStateProperties::WATERLOGGED)
        .unwrap_or(false)
    {
        return false;
    }
    if state.get_block() == &vanilla_blocks::SUNFLOWER {
        return state.try_get_value(&BlockStateProperties::HALF)
            == Some(steel_registry::blocks::properties::Half::Top);
    }
    true
}

/// Samples a vanilla boolean environment attribute from the dimension's timelines.
///
/// Deviation: placeholder for a public environment-attribute getter on `World`
/// (`world::environment` currently exposes only sky light, sun angle and
/// monsters-burn, owner: `world/environment`).
#[must_use]
pub(crate) fn bool_attribute(world: &World, attribute: &str, default: bool) -> bool {
    let mut value = default;
    for_each_timeline(world, |timeline| {
        let Some(track) = timeline
            .tracks
            .iter()
            .find(|track| track.name == attribute)
        else {
            return;
        };
        let Some(total_ticks) = world.clock_total_ticks(timeline.clock) else {
            return;
        };
        let Some(sample) =
            sample_bool_track(track, timeline.period_ticks.map(i64::from), total_ticks)
        else {
            return;
        };
        value = match track.modifier {
            Some("or") => value || sample,
            Some("and") => value && sample,
            None => sample,
            _ => value,
        };
    });
    value
}

/// Samples a vanilla float environment attribute from the dimension's timelines.
///
/// Deviation: placeholder for a public environment-attribute getter on `World`
/// (owner: `world/environment`); unknown named easings are treated as stepped.
#[must_use]
pub(crate) fn float_attribute(world: &World, attribute: &str, default: f32) -> f32 {
    let mut value = default;
    for_each_timeline(world, |timeline| {
        let Some(track) = timeline
            .tracks
            .iter()
            .find(|track| track.name == attribute)
        else {
            return;
        };
        let Some(total_ticks) = world.clock_total_ticks(timeline.clock) else {
            return;
        };
        let Some(sample) =
            sample_float_track(track, timeline.period_ticks.map(i64::from), total_ticks)
        else {
            return;
        };
        value = match track.modifier {
            Some("maximum") => value.max(sample),
            Some("multiply") => value * sample,
            None => sample,
            _ => value,
        };
    });
    value
}

/// Visits every timeline attached to the world's dimension, mirroring
/// `world::environment::for_each_timeline`.
fn for_each_timeline(world: &World, mut visit: impl FnMut(TimelineRef)) {
    let Some(timelines) = world.dimension_type.timelines else {
        return;
    };
    if let Some(tag) = timelines.strip_prefix('#') {
        let Ok(tag) = Identifier::from_str(tag) else {
            return;
        };
        for timeline in REGISTRY.timelines.iter_tag(&tag) {
            visit(timeline);
        }
        return;
    }

    let Ok(key) = Identifier::from_str(timelines) else {
        return;
    };
    if let Some(timeline) = REGISTRY.timelines.by_key(&key) {
        visit(timeline);
    }
}

/// Boolean tracks hold the segment start until the next keyframe, ignoring easing.
fn sample_bool_track(track: &Track, period_ticks: Option<i64>, ticks: i64) -> Option<bool> {
    let keyframes = track.keyframes;
    match keyframes.len() {
        0 => return None,
        1 => return keyframe_bool_value(&keyframes[0].value),
        _ => {}
    }

    let sample_ticks = period_ticks.map_or(ticks, |period| ticks.rem_euclid(period));
    let first = &keyframes[0];
    let last = &keyframes[keyframes.len() - 1];

    if let Some(period) = period_ticks
        && sample_ticks < first.ticks
    {
        return step_bool_segment(
            last.ticks - period,
            &last.value,
            first.ticks,
            &first.value,
            sample_ticks,
        );
    }

    for segment in keyframes.windows(2) {
        let from = &segment[0];
        let to = &segment[1];
        if sample_ticks < to.ticks {
            return step_bool_segment(from.ticks, &from.value, to.ticks, &to.value, sample_ticks);
        }
    }

    if let Some(period) = period_ticks {
        return step_bool_segment(
            last.ticks,
            &last.value,
            first.ticks + period,
            &first.value,
            sample_ticks,
        );
    }

    keyframe_bool_value(&last.value)
}

fn step_bool_segment(
    from_ticks: i64,
    from_value: &KeyframeValue,
    to_ticks: i64,
    to_value: &KeyframeValue,
    sample_ticks: i64,
) -> Option<bool> {
    let from = keyframe_bool_value(from_value)?;
    let to = keyframe_bool_value(to_value)?;
    if sample_ticks <= from_ticks {
        return Some(from);
    }
    if sample_ticks >= to_ticks {
        return Some(to);
    }
    Some(from)
}

/// Float tracks interpolate with the track's easing; only `constant` and
/// `linear` (or absent) easing are honoured here, other named easings step.
fn sample_float_track(track: &Track, period_ticks: Option<i64>, ticks: i64) -> Option<f32> {
    let keyframes = track.keyframes;
    match keyframes.len() {
        0 => return None,
        1 => return keyframe_float_value(&keyframes[0].value),
        _ => {}
    }

    let sample_ticks = period_ticks.map_or(ticks, |period| ticks.rem_euclid(period));
    let first = &keyframes[0];
    let last = &keyframes[keyframes.len() - 1];

    if let Some(period) = period_ticks
        && sample_ticks < first.ticks
    {
        return interpolate_float_segment(
            track,
            last.ticks - period,
            &last.value,
            first.ticks,
            &first.value,
            sample_ticks,
        );
    }

    for segment in keyframes.windows(2) {
        let from = &segment[0];
        let to = &segment[1];
        if sample_ticks < to.ticks {
            return interpolate_float_segment(
                track,
                from.ticks,
                &from.value,
                to.ticks,
                &to.value,
                sample_ticks,
            );
        }
    }

    if let Some(period) = period_ticks {
        return interpolate_float_segment(
            track,
            last.ticks,
            &last.value,
            first.ticks + period,
            &first.value,
            sample_ticks,
        );
    }

    keyframe_float_value(&last.value)
}

fn interpolate_float_segment(
    track: &Track,
    from_ticks: i64,
    from_value: &KeyframeValue,
    to_ticks: i64,
    to_value: &KeyframeValue,
    sample_ticks: i64,
) -> Option<f32> {
    let from = keyframe_float_value(from_value)?;
    let to = keyframe_float_value(to_value)?;
    if sample_ticks <= from_ticks {
        return Some(from);
    }
    if sample_ticks >= to_ticks {
        return Some(to);
    }

    let alpha = (sample_ticks - from_ticks) as f32 / (to_ticks - from_ticks) as f32;
    // Deviation: the local bridge treats `constant` as a step and anything else
    // (including cubic-bezier) as linear; gameplay attributes use `constant`.
    let eased_alpha = match track.ease.as_ref() {
        Some(Ease::Named("constant")) => 0.0,
        _ => alpha,
    };
    Some(from + eased_alpha * (to - from))
}

const fn keyframe_float_value(value: &KeyframeValue) -> Option<f32> {
    match value {
        KeyframeValue::Float(value) => Some(*value),
        _ => None,
    }
}

const fn keyframe_bool_value(value: &KeyframeValue) -> Option<bool> {
    match value {
        KeyframeValue::Bool(value) => Some(*value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::BlockPos;

    use crate::test_support::TestLevel;

    use super::*;

    fn level_with_support(support: BlockRef) -> TestLevel {
        TestLevel::default().with_block(BlockPos::new(0, 63, 0), support.default_state())
    }

    #[test]
    fn eyeblossom_requires_vegetation_support() {
        init_vanilla_registry();
        let behavior =
            EyeblossomBlock::new(&vanilla_blocks::CLOSED_EYEBLOSSOM, EyeblossomType::Closed);
        let pos = BlockPos::new(0, 64, 0);
        let state = vanilla_blocks::CLOSED_EYEBLOSSOM.default_state();

        assert!(behavior.can_survive(state, &level_with_support(&vanilla_blocks::DIRT), pos));
        assert!(!behavior.can_survive(state, &level_with_support(&vanilla_blocks::AIR), pos));
    }

    #[test]
    fn eyeblossom_type_round_trips() {
        assert_eq!(EyeblossomType::from_boolean(true), EyeblossomType::Open);
        assert_eq!(EyeblossomType::Open.transform(), EyeblossomType::Closed);
        assert_eq!(EyeblossomType::Closed.transform(), EyeblossomType::Open);
        assert_eq!(EyeblossomType::Open.block(), &vanilla_blocks::OPEN_EYEBLOSSOM);
        assert_eq!(
            EyeblossomType::Closed.block(),
            &vanilla_blocks::CLOSED_EYEBLOSSOM
        );
    }
}
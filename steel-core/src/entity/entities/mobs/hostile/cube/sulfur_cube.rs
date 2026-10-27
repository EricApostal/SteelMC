//! Vanilla `SulfurCube` (`net.minecraft.world.entity.monster.cubemob.SulfurCube`).
//!
//! A sulfur cube is an [`AgeableMob`] (not a `Monster`) that swallows one block item into its
//! `BODY` equipment slot. The swallowed item selects one or more
//! [`SulfurCubeArchetype`](steel_registry::sulfur_cube_archetype::SulfurCubeArchetype)s from the
//! `sulfur_cube_archetypes` registry; the union of their attribute modifiers, contact damage,
//! explosion data, knockback modifiers and sound settings defines how the cube behaves until the
//! item is ejected (shears) or replaced.
//!
//! The fuse is a plain runtime field mirrored into the synced `max_fuse` value, exactly like
//! vanilla's `fuse` / `MAX_FUSE` pair.
//!
//! ## Deviations
//!
//! - Vanilla `SulfurCube.knockback` is reproduced from
//!   [`LivingEntity::apply_damage_knockback`], which does carry the `DamageSource`; the damage
//!   amount is captured in `before_actually_hurt`. Steel's hook supplies the pre-armor damage
//!   (armor/absorption are applied afterwards in `actually_hurt`), while vanilla hands the
//!   post-reduction amount to `knockback`, so the `sqrt(damage)` power scale differs for armored
//!   targets. GAP(sulfur-cube): exact `SulfurCube.knockback` damage scaling needs a post-armor
//!   damage amount at the knockback hook.
//! - GAP(sulfur-cube): vanilla `SulfurCube.isInvulnerableToPiercingWeapon` makes a primed cube
//!   pierceable; Steel has no piercing-weapon invulnerability hook.
//! - GAP(sulfur-cube): vanilla `SulfurCube.canBePickedFromInside` forbids picking a cube with a
//!   swallowed block out of an entity's inside; Steel has no such hook.
//! - GAP(sulfur-cube): vanilla `SulfurCube.omnidirectionalAirMover` (body item ⇒ air drag applies
//!   on every axis) has no equivalent in Steel's movement code.
//! - GAP(sulfur-cube): vanilla `SulfurCube.getLightLevelDependentMagicValue` returns a constant
//!   `1.0`; Steel resolves that brightness from the world (`LevelReader`), and there is no
//!   per-entity override hook. Sulfur cubes are not in `BURN_IN_DAYLIGHT`, so no server behavior
//!   depends on the constant today.
//! - GAP(sulfur-cube): vanilla `SulfurCube.getLeashOffset` (`bbHeight / 2`); Steel has no
//!   per-entity leash attachment offset hook.
//! - GAP(sulfur-cube): vanilla `Entity.triggerOnDeathMobEffects(level, KILLED)` runs before the
//!   fuse explosion discards the cube; Steel clears active effects through the shared death path
//!   only, and the explosion path removes the entity without running mob-effect removal hooks.
//! - GAP(sulfur-cube): vanilla passes `PrimedTnt.USED_PORTAL_DAMAGE_CALCULATOR` to the explosion
//!   when `getPortalCooldown() > 0`; Steel's [`World::explode_full`] accepts a damage calculator but
//!   has no nether-portal-exempt implementation yet.

use std::f32::consts::TAU;
use std::mem;
use std::sync::{Arc, Weak};

use glam::{DVec3, Vec2};
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::{DEGREE_90, RAD_TO_DEG};
use steel_protocol::packets::game::{CTakeItemEntity, SoundSource};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::components::{CustomData, SulfurCubeContent};
use steel_registry::data_components::vanilla_components::{
    BUCKET_ENTITY_DATA, CUSTOM_NAME, SULFUR_CUBE_CONTENT,
};
use steel_registry::entity_type::{EntityAttachment, EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::sulfur_cube_archetype::{
    SulfurContactDamage, SulfurCubeArchetype, SulfurExplosionData, SulfurKnockbackModifiers,
    SulfurSoundSettings,
};
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entity_data::SulfurCubeEntityData;
use steel_registry::vanilla_game_rules::{MOB_GRIEFING, TNT_EXPLODES};
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    ItemStackTemplate, REGISTRY, RegistryExt, sound_events, vanilla_attributes,
    vanilla_game_events, vanilla_items, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::translations::BLOCK_MINECRAFT_TNT_DISABLED;
use steel_utils::types::InteractionHand;
use steel_utils::{
    BlockPos, BlockStateId, ChunkPos, Downcast as _, DowncastType, DowncastTypeKey, Identifier,
};
use text_components::TextComponent;

use crate::behavior::advancement_hooks::{raise_filled_bucket, raise_player_sheared_equipment};
use crate::behavior::{BLOCK_BEHAVIORS, InteractionResult};
use crate::entity::ageable::AgeableMobBase;
use crate::entity::ai::control::rotate_towards;
use crate::entity::ai::goal::{Goal, GoalControls};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::attribute::AttributeModifier;
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::explosives::PrimedTntEntity;
use crate::entity::entities::objects::items::ItemEntity;
use crate::entity::leash::Leashable;
use crate::entity::{
    AgeableMob, ENTITIES, Entity, EntityBase, EntityBaseLoad, EntityEventSource as _, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, RemovalReason, SharedEntity, SpawnGroupData, next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::explosion::ExplosionInteraction;
use crate::world::{LevelReader, SignalGetter, World};

use super::abstract_cube_mob::{
    CubeMoveState, CubeSquish, clamp_cube_size, cube_jump_delay, cube_movement_speed,
    cube_sound_pitch, cube_sound_volume,
};

/// Vanilla `SulfurCube.SPLIT_COUNT`.
const SPLIT_COUNT: i32 = 2;
/// Vanilla `SulfurCube.MAX_SIZE`.
const MAX_SIZE: i32 = 2;
/// Vanilla `SulfurCube.MIN_SIZE`.
const MIN_SIZE: i32 = 1;
/// Vanilla `SulfurCube.PICKUP_TIMER_DURATION`.
const PICKUP_TIMER_DURATION: i32 = 100;
/// Vanilla `SulfurCube.PUSH_DISTANCE_THRESHOLD`.
const PUSH_DISTANCE_THRESHOLD: f64 = 1.3;
/// Vanilla `SulfurCube.MAX_PLAYER_PUSH_SPEED`.
const MAX_PLAYER_PUSH_SPEED: f64 = 0.5;
/// Vanilla `SulfurCube.PLAYER_PUSH_SPEED_SCALE_MULTIPLIER`.
const PLAYER_PUSH_SPEED_SCALE_MULTIPLIER: f32 = 0.3;
/// Vanilla `SulfurCube.VEHICLE_PUSH_SPEED_SCALE_MULTIPLIER`.
const VEHICLE_PUSH_SPEED_SCALE_MULTIPLIER: f32 = 0.16;
/// Vanilla `SulfurCube.VERTICAL_PUSH_MULTIPLIER`.
const VERTICAL_PUSH_MULTIPLIER: f32 = 0.3;
/// Vanilla `SulfurCube` explosion centre Y offset.
const EXPLOSION_Y_OFFSET: f64 = 0.0625;
/// Vanilla `SulfurCube` buoyancy vibration frequency.
const FLOAT_VIBE_FREQUENCY: f32 = 0.4;
/// Vanilla `SulfurCube` buoyancy vibration amplitude.
const FLOAT_VIBE_AMPLITUDE: f32 = 0.2;
/// Vanilla `SulfurCube` buoyancy push per tick.
const FLOAT_PUSH_PER_TICK: f64 = 0.04;
/// Vanilla `LivingEntity.DAMAGE_KNOCKBACK_POWER`.
const DAMAGE_KNOCKBACK_POWER: f64 = 0.4;
/// Vanilla `SulfurCubeArchetype.DEFAULT_KNOCKBACK_MODIFIERS` horizontal power.
const DEFAULT_KNOCKBACK_HORIZONTAL_POWER: f32 = 0.33;
/// Vanilla `SulfurCubeArchetype.DEFAULT_KNOCKBACK_MODIFIERS` vertical power.
const DEFAULT_KNOCKBACK_VERTICAL_POWER: f32 = 0.06;
/// Vanilla `SulfurCube.HORIZONTAL_HIT_ANGLE_SCALE`.
const HORIZONTAL_HIT_ANGLE_SCALE: f32 = 1.6;
/// Vanilla `SulfurCube.VERTICAL_HIT_ANGLE_SCALE`.
const VERTICAL_HIT_ANGLE_SCALE: f32 = 0.5;
/// Vanilla `SulfurCube.VERTICAL_POSITION_ANGLE_SCALE`.
const VERTICAL_POSITION_ANGLE_SCALE: f32 = 0.8;
/// Vanilla `SulfurCube.EXTRA_KNOCKBACK_DAMPENING`.
const EXTRA_KNOCKBACK_DAMPENING: f32 = 0.25;
/// Vanilla `SulfurCube.knockback` vertical impulse multiplier.
const VERTICAL_KNOCKBACK_MULTIPLIER: f64 = 1.2;
/// Vanilla `SulfurCube.knockback` horizontal power multiplier.
const HORIZONTAL_KNOCKBACK_MULTIPLIER: f32 = 0.4;
/// Vanilla `SulfurCube.knockback` power clamp.
const KNOCKBACK_POWER_LIMIT: f32 = 128.0;
/// Vanilla `SulfurCube.setCubeMobHealth` health per size.
const HEALTH_PER_SIZE: f64 = 4.0;
/// Vanilla `SulfurCubeTemptGoal` stop distance.
const TEMPT_STOP_DISTANCE: f64 = 1.0;
/// Vanilla `SulfurCubeSearchForItemsGoal` search inflation.
const ITEM_SEARCH_RANGE: f64 = 8.0;
/// Vanilla `Goal.reducedTickDelay(100)` for the tempt goal's calm-down timer.
const TEMPT_CALM_DOWN_TICKS: i32 = 50;
/// Vanilla `SulfurCubeArchetype.DEFAULT_SOUND_SETTINGS` push impulse threshold.
const DEFAULT_PUSH_SOUND_IMPULSE_THRESHOLD: f32 = 0.2;
/// Vanilla `SulfurCubeArchetype.DEFAULT_SOUND_SETTINGS` push sound cooldown, in seconds.
const DEFAULT_PUSH_SOUND_COOLDOWN: f32 = 0.5;
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `SulfurCube` runtime state that is not synchronized.
struct SulfurCubeState {
    pickup_timer: i32,
    push_sound_cooldown: i32,
    floats_in_liquids: bool,
    explosion_data: Option<&'static SulfurExplosionData>,
    contact_damages: Vec<&'static SulfurContactDamage>,
    knockback_modifiers: Option<&'static SulfurKnockbackModifiers>,
    sound_settings: Option<&'static SulfurSoundSettings>,
    /// Damage amount of the in-flight hit, captured in `before_actually_hurt` for
    /// vanilla `SulfurCube.knockback`'s `sqrt(damage)` power scale.
    last_damage: f32,
    fuse: i32,
    /// Last seen `BODY` item, used to detect equipment changes like vanilla's
    /// `collectEquipmentChanges` override.
    last_body_item: ItemStack,
}

impl Default for SulfurCubeState {
    fn default() -> Self {
        Self::new()
    }
}

impl SulfurCubeState {
    fn new() -> Self {
        Self {
            pickup_timer: 0,
            push_sound_cooldown: 0,
            floats_in_liquids: false,
            explosion_data: None,
            contact_damages: Vec::new(),
            knockback_modifiers: None,
            sound_settings: None,
            last_damage: 0.0,
            fuse: -1,
            last_body_item: ItemStack::empty(),
        }
    }

    /// Vanilla `SulfurCubeArchetype.DEFAULT_KNOCKBACK_MODIFIERS` fallback.
    fn knockback_modifiers(&self) -> SulfurKnockbackModifiers {
        self.knockback_modifiers.map_or(
            SulfurKnockbackModifiers {
                horizontal_power: DEFAULT_KNOCKBACK_HORIZONTAL_POWER,
                vertical_power: DEFAULT_KNOCKBACK_VERTICAL_POWER,
            },
            |modifiers| SulfurKnockbackModifiers {
                horizontal_power: modifiers.horizontal_power,
                vertical_power: modifiers.vertical_power,
            },
        )
    }

    /// Vanilla `SulfurCubeArchetype.SoundSettings.hitSound`.
    fn hit_sound(&self) -> SoundEventRef {
        self.sound_settings
            .map_or(&sound_events::ENTITY_SULFUR_CUBE_REGULAR_HIT, |settings| {
                resolve_sound(&settings.hit_sound)
            })
    }

    /// Vanilla `SulfurCubeArchetype.DEFAULT_SOUND_SETTINGS` fallback.
    fn push_sound_impulse_threshold(&self) -> f32 {
        self.sound_settings
            .map_or(DEFAULT_PUSH_SOUND_IMPULSE_THRESHOLD, |settings| {
                settings.push_sound_impulse_threshold
            })
    }

    /// Vanilla `SulfurCubeArchetype.DEFAULT_SOUND_SETTINGS` fallback.
    fn push_sound_cooldown_seconds(&self) -> f32 {
        self.sound_settings
            .map_or(DEFAULT_PUSH_SOUND_COOLDOWN, |settings| {
                settings.push_sound_cooldown
            })
    }

    /// Vanilla `SulfurCubeArchetype.SoundSettings.pushSound`.
    fn push_sound(&self) -> SoundEventRef {
        self.sound_settings
            .map_or(&sound_events::ENTITY_SULFUR_CUBE_REGULAR_PUSH, |settings| {
                resolve_sound(&settings.push_sound)
            })
    }
}

/// Resolves an archetype sound identifier through the sound-event registry.
///
/// Archetype data is loaded from the datapack, so an unknown key is a registry
/// integrity error; the vanilla regular hit sound is used so the cube stays audible.
fn resolve_sound(identifier: &Identifier) -> SoundEventRef {
    REGISTRY.sound_events.by_key(identifier).unwrap_or_else(|| {
        log::error!("sulfur cube archetype references unknown sound {identifier:?}");
        &sound_events::ENTITY_SULFUR_CUBE_REGULAR_HIT
    })
}

/// Vanilla `Mth.clampedMap`.
fn clamped_map(value: f64, min: f64, max: f64, new_min: f64, new_max: f64) -> f32 {
    let lerp = ((value - min) / (max - min)).clamp(0.0, 1.0);
    (new_min + lerp * (new_max - new_min)) as f32
}

/// Vanilla `Vec2.rotate` (radians).
fn rotate_vec2(value: Vec2, angle: f32) -> Vec2 {
    let (sin, cos) = angle.sin_cos();
    Vec2::new(value.x * cos - value.y * sin, value.x * sin + value.y * cos)
}

/// Vanilla `SulfurCube.applyHorizontalHitAngleScale`.
fn apply_horizontal_hit_angle_scale(
    horizontal_angle_scale: f32,
    original_angle: Vec2,
    attacker_position: DVec3,
    attacker_aim_direction: DVec3,
    target_center: DVec3,
) -> Vec2 {
    let attacker_to_target = (target_center - attacker_position).normalize_or_zero();
    let angle_diff = (attacker_aim_direction.x * attacker_to_target.z
        - attacker_aim_direction.z * attacker_to_target.x)
        .atan2(
            attacker_aim_direction.x * attacker_to_target.x
                + attacker_aim_direction.z * attacker_to_target.z,
        ) as f32;
    rotate_vec2(original_angle, angle_diff * horizontal_angle_scale)
}

/// Vanilla `SulfurCube.applyVerticalHitAnglePowerTransfer`.
fn apply_vertical_hit_angle_power_transfer(
    vertical_hit_angle_scale: f32,
    horizontal_power: f32,
    vertical_power: f32,
    attacker_position: DVec3,
    attacker_aim_direction: DVec3,
    target_centered_position: DVec3,
    target_height: f32,
) -> Vec2 {
    let target_half_height = 0.5 * f64::from(target_height);
    let target_top_position = target_centered_position + DVec3::new(0.0, target_half_height, 0.0);
    let target_bottom_position =
        target_centered_position - DVec3::new(0.0, target_half_height, 0.0);
    let attacker_to_target_top = (target_top_position - attacker_position).normalize_or_zero();
    let attacker_to_target_bottom =
        (target_bottom_position - attacker_position).normalize_or_zero();

    let vertical_hit_angle_factor = clamped_map(
        attacker_aim_direction.y,
        attacker_to_target_top.y,
        attacker_to_target_bottom.y,
        -1.0,
        1.0,
    );
    let mut transferred_power_ratio = (vertical_hit_angle_factor * vertical_hit_angle_scale).abs();
    if vertical_hit_angle_factor < 0.0 {
        transferred_power_ratio = -transferred_power_ratio;
    }

    Vec2::new(
        horizontal_power * (1.0 - transferred_power_ratio),
        vertical_power * (1.0 + transferred_power_ratio),
    )
}

/// Vanilla `SulfurCube.applyVerticalPositionAnglePowerRotation`.
fn apply_vertical_position_angle_power_rotation(
    vertical_position_angle_scale: f32,
    horizontal_power: f32,
    vertical_power: f32,
    original_horizontal_power: f32,
    original_vertical_power: f32,
    attacker_feet_position: DVec3,
    target_feet_position: DVec3,
) -> Vec2 {
    let attacker_feet_to_target_feet = target_feet_position - attacker_feet_position;
    let vertical_position_angle = (-attacker_feet_to_target_feet.y).atan2(
        attacker_feet_to_target_feet
            .x
            .hypot(attacker_feet_to_target_feet.z),
    ) as f32;
    let power_before_rotation = Vec2::new(horizontal_power, vertical_power);
    let mut rotated_power = rotate_vec2(
        power_before_rotation,
        -vertical_position_angle * vertical_position_angle_scale,
    );

    let horizontal_ratio = if original_horizontal_power > 0.0 {
        rotated_power.x.abs() / original_horizontal_power
    } else {
        0.0
    };
    let vertical_ratio = if original_vertical_power > 0.0 {
        rotated_power.y.abs() / original_vertical_power
    } else {
        0.0
    };
    let max_ratio = horizontal_ratio.max(vertical_ratio);
    if max_ratio > 1.0 {
        rotated_power /= max_ratio;
    }

    rotated_power
}

/// Vanilla `Mth.wrapDegrees90`.
fn wrap_degrees_90(angle: f32) -> f32 {
    let mut normalized = angle % 90.0;
    if normalized >= 45.0 {
        normalized -= 90.0;
    }
    if normalized < -45.0 {
        normalized += 90.0;
    }
    normalized
}

/// Vanilla `SulfurCube`.
#[entity_behavior(class = "SulfurCube")]
pub struct SulfurCubeEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    entity_data: SyncMutex<SulfurCubeEntityData>,
    squish: SyncMutex<CubeSquish>,
    was_on_ground: SyncMutex<bool>,
    move_state: SyncMutex<CubeMoveState>,
    state: SyncMutex<SulfurCubeState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SulfurCubeEntity`.
unsafe impl DowncastType for SulfurCubeEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/sulfur_cube");
}

impl SulfurCubeEntity {
    /// Creates a new sulfur cube entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved sulfur cube entity.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        let ageable_base = AgeableMobBase::new();
        let mut entity_data = SulfurCubeEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.abstract_cube_mob_mut().id_size.set(1);

        let cube = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            entity_data: SyncMutex::new(entity_data),
            squish: SyncMutex::new(CubeSquish::new()),
            was_on_ground: SyncMutex::new(false),
            move_state: SyncMutex::new(CubeMoveState::new()),
            state: SyncMutex::new(SulfurCubeState::new()),
        };
        cube.register_goals();
        cube
    }

    /// Vanilla `AbstractCubeMob.registerGoals` plus `SulfurCube.addBehaviourGoals`.
    ///
    /// Vanilla re-runs this after the swallowed item is ejected; the goal selector
    /// must therefore be rebuilt from scratch every time.
    fn register_goals(&self) {
        let mut goal_selector = self.mob_base.goal_selector().lock();
        goal_selector.add_goal(1, SulfurCubeFloatGoal::new());
        goal_selector.add_goal(2, SulfurCubeTemptGoal::new());
        goal_selector.add_goal(3, SulfurCubeSearchForItemsGoal::new());
        goal_selector.add_goal(4, SulfurCubeRandomDirectionGoal::new());
        goal_selector.add_goal(5, SulfurCubeKeepOnJumpingGoal::new());
        // Vanilla `SulfurCube.addTargetingGoals` is empty.
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }

        let display = self.living_base.mob_effect_display_state();

        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }

        self.entity_data.set_base_invisible_flag(display.invisible);
    }

    /// Vanilla `AbstractCubeMob.getSize`.
    #[must_use]
    pub fn size(&self) -> i32 {
        *self.entity_data.lock().abstract_cube_mob().id_size.get()
    }

    /// Vanilla `AbstractCubeMob.isTiny`.
    #[must_use]
    pub fn is_tiny(&self) -> bool {
        self.size() <= 1
    }

    /// Vanilla `SulfurCube.setSize`.
    pub fn set_size(&self, size: i32, update_health: bool) {
        let actual_size = clamp_cube_size(size);
        self.entity_data
            .lock()
            .abstract_cube_mob_mut()
            .id_size
            .set(actual_size);
        self.refresh_dimensions();

        {
            let mut attributes = self.attributes().lock();
            // Vanilla `SulfurCube.setCubeMobHealth` (`4 * actualSize`).
            attributes.set_base_value(
                vanilla_attributes::MAX_HEALTH,
                HEALTH_PER_SIZE * f64::from(actual_size),
            );
            attributes.set_base_value(
                vanilla_attributes::MOVEMENT_SPEED,
                cube_movement_speed(actual_size),
            );
        }

        if update_health {
            let max_health = self.get_max_health();
            self.set_health(max_health);
        }

        if update_health && actual_size == MIN_SIZE && !AgeableMob::is_baby(self) {
            AgeableMob::set_baby(self, true);
        }
    }

    /// Vanilla `AbstractCubeMob.getDefaultDimensions`.
    #[must_use]
    pub fn cube_dimensions(&self) -> EntityDimensions {
        self.entity_type.dimensions.scale(self.size() as f32)
    }

    /// Vanilla `SulfurCube.fromBucket`.
    #[must_use]
    #[expect(
        clippy::wrong_self_convention,
        reason = "vanilla `SulfurCube.fromBucket` getter; the name is shared with the other bucketable entities"
    )]
    pub fn from_bucket(&self) -> bool {
        *self.entity_data.lock().from_bucket.get()
    }

    /// Vanilla `SulfurCube.setFromBucket`.
    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.entity_data.lock().from_bucket.set(from_bucket);
    }

    /// Vanilla `SulfurCube.getFuse`.
    #[must_use]
    pub fn fuse(&self) -> i32 {
        self.state.lock().fuse
    }

    /// Vanilla `SulfurCube.setFuse`.
    fn set_fuse(&self, fuse: i32) {
        self.state.lock().fuse = fuse;
    }

    /// Vanilla `SulfurCube.isPrimed`.
    #[must_use]
    pub fn is_primed(&self) -> bool {
        self.fuse() >= 0
    }

    /// Vanilla `SulfurCube.hasBodyItem`.
    #[must_use]
    pub fn has_body_item(&self) -> bool {
        self.has_item_in_slot(EquipmentSlot::Body)
    }

    /// Vanilla `SulfurCube.canExplode`.
    #[must_use]
    pub fn can_explode(&self) -> bool {
        self.state.lock().explosion_data.is_some() && Entity::is_alive(self) && !self.is_primed()
    }

    /// Vanilla `SulfurCube.isSwallowableItem`.
    #[must_use]
    pub fn is_swallowable_item(item_stack: &ItemStack) -> bool {
        item_stack.item().has_tag(&ItemTag::SULFUR_CUBE_SWALLOWABLE)
    }

    /// Vanilla `SulfurCube.isFood`.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        item_stack.item().has_tag(&ItemTag::SULFUR_CUBE_FOOD)
    }

    /// Vanilla `SulfurCube.matchingArchetypes`.
    #[must_use]
    pub fn matching_archetypes(stack: &ItemStack) -> Vec<&'static SulfurCubeArchetype> {
        if stack.is_empty() {
            return Vec::new();
        }

        let item_key = &stack.item().key;
        REGISTRY
            .sulfur_cube_archetypes
            .iter()
            .filter(|(_, archetype)| archetype.items.iter().any(|id| id == item_key))
            .map(|(_, archetype)| archetype)
            .collect()
    }

    /// Vanilla `SulfurCube.collectEquipmentChanges`'s archetype half.
    ///
    /// Recomputes the goal set, attribute modifiers and archetype-derived state
    /// whenever the swallowed `BODY` item changes.
    fn sync_archetype_from_body(&self) {
        let current = self.equipment_in_slot(EquipmentSlot::Body);
        let previous = {
            let mut state = self.state.lock();
            if ItemStack::matches(&state.last_body_item, &current) {
                return;
            }
            mem::replace(&mut state.last_body_item, current.clone())
        };

        if current.is_empty() {
            self.register_goals();
        } else {
            PathfinderMob::remove_all_goals(self, |_| true);
            self.set_mob_speed(0.0);
        }

        for archetype in Self::matching_archetypes(&previous) {
            for entry in &archetype.attribute_modifiers {
                let Some(attribute) = REGISTRY.attributes.by_key(&entry.attribute) else {
                    continue;
                };
                self.attributes()
                    .lock()
                    .remove_modifier(attribute, &entry.id);
            }
        }

        let mut floats_in_liquids = false;
        let mut explosion_data = None;
        let mut contact_damages = Vec::new();
        let mut knockback_modifiers = None;
        let mut sound_settings = None;

        for archetype in Self::matching_archetypes(&current) {
            if archetype.buoyant {
                floats_in_liquids = true;
            }

            if archetype.explosion.is_some() {
                explosion_data = archetype.explosion.as_ref();
            }

            if let Some(contact_damage) = archetype.contact_damage.as_ref() {
                contact_damages.push(contact_damage);
            }

            knockback_modifiers = Some(&archetype.knockback_modifiers);
            sound_settings = Some(&archetype.sound_settings);

            for entry in &archetype.attribute_modifiers {
                let Some(attribute) = REGISTRY.attributes.by_key(&entry.attribute) else {
                    continue;
                };
                self.attributes().lock().add_modifier(
                    attribute,
                    AttributeModifier {
                        id: entry.id.clone(),
                        amount: entry.amount,
                        operation: entry.operation,
                    },
                    false,
                );
            }
        }

        let mut state = self.state.lock();
        state.floats_in_liquids = floats_in_liquids;
        state.explosion_data = explosion_data;
        state.contact_damages = contact_damages;
        state.knockback_modifiers = knockback_modifiers;
        state.sound_settings = sound_settings;
    }

    /// Vanilla `SulfurCube.tickFuse`.
    fn tick_fuse(self: &Arc<Self>) {
        {
            let mut state = self.state.lock();
            if state.fuse > 0 {
                state.fuse -= 1;
            }
            if state.explosion_data.is_none() || state.fuse != 0 {
                return;
            }
        }

        self.drop_leash();
        let Some(world) = self.level() else {
            self.set_removed(RemovalReason::Discarded);
            return;
        };

        if world.get_game_rule(&TNT_EXPLODES) {
            let interaction = if world.get_game_rule(&MOB_GRIEFING) {
                ExplosionInteraction::Tnt
            } else {
                ExplosionInteraction::None
            };
            let explosion = self.state.lock().explosion_data;
            if let Some(explosion) = explosion {
                let position = self.position();
                let source: SharedEntity = Arc::clone(self) as SharedEntity;
                world.explode_full(
                    Some(source),
                    None,
                    None,
                    position.x,
                    position.y + EXPLOSION_Y_OFFSET,
                    position.z,
                    explosion.power as f32,
                    explosion.causes_fire,
                    interaction,
                );
            }
        }

        self.set_removed(RemovalReason::Discarded);
    }

    /// Vanilla `SulfurCube.primeWhenOnPoweredPosition`.
    fn prime_when_on_powered_position(&self) {
        if !self.can_explode() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        if world.get_best_own_or_neighbour_signal(self.block_position()) != 0 {
            self.prime_time(false);
        }
    }

    /// Vanilla `SulfurCube.primeTime`.
    ///
    /// Returns whether the cube was primed.
    pub fn prime_time(&self, imminent: bool) -> bool {
        let Some(explosion) = self.state.lock().explosion_data else {
            return false;
        };
        if !Entity::is_alive(self) || self.is_primed() {
            return false;
        }
        let Some(world) = self.level() else {
            return false;
        };
        if !world.get_game_rule(&TNT_EXPLODES) {
            return false;
        }

        let fuse_time = if imminent {
            PrimedTntEntity::random_short_fuse(explosion.fuse)
        } else {
            explosion.fuse
        };

        self.set_invulnerable(true);
        self.set_fuse(fuse_time);
        self.entity_data.lock().max_fuse.set(fuse_time);
        self.play_sound(
            &sound_events::ENTITY_TNT_PRIMED,
            self.sound_volume(),
            self.voice_pitch(),
        );
        self.game_event(&vanilla_game_events::PRIME_FUSE);
        true
    }

    /// Vanilla `AbstractCubeMob.tick`: squish animation, landing particles and squish sound.
    fn tick_cube(&self) {
        self.squish.lock().tick();

        let on_ground = self.on_ground();
        let was_on_ground = *self.was_on_ground.lock();

        if on_ground && !was_on_ground {
            let width = self.cube_dimensions().width;
            if let Some(world) = self.level() {
                let size = width * 2.0;
                let radius = size / 2.0;
                let count = (size * 16.0) as i32;
                let position = self.position();
                for _ in 0..count {
                    let dir = rand::random::<f32>() * TAU;
                    let d = rand::random::<f32>() * 0.5 + 0.5;
                    let xd = f64::from(dir.sin() * radius * d);
                    let zd = f64::from(dir.cos() * radius * d);
                    world.send_particles(
                        ParticleData::simple(&vanilla_particle_types::SULFUR_CUBE_GOO),
                        DVec3::new(position.x + xd, position.y, position.z + zd),
                        1,
                        DVec3::ZERO,
                        0.0,
                    );
                }
            }

            // Vanilla plays the squish sound at `getSoundVolume()` with
            // `((random - random) * 0.2 + 1) / 0.8`.
            let pitch = ((rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0) / 0.8;
            self.play_sound(self.squish_sound(), cube_sound_volume(self.size()), pitch);
            self.squish.lock().target = -0.5;
        } else if !on_ground && was_on_ground {
            self.squish.lock().target = 1.0;
        }

        *self.was_on_ground.lock() = on_ground;
        self.squish.lock().decrease_squish();
    }

    /// Vanilla `AbstractCubeMob.CubeMobMoveControl.tick`, skipped while a block is swallowed.
    fn tick_cube_movement(&self) {
        if self.has_body_item() {
            return;
        }

        let (yaw, _pitch) = self.rotation();
        let mut state = *self.move_state.lock();
        let new_yaw = rotate_towards(yaw, state.y_rot, 90.0);
        self.set_rotation((new_yaw, 0.0));
        self.set_y_head_rot(new_yaw);
        self.set_y_body_rot(new_yaw);

        let Some(wanted_speed) = state.wanted_speed else {
            self.set_mob_speed(0.0);
            return;
        };

        let movement_speed = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::MOVEMENT_SPEED)
            .unwrap_or(0.0);

        if self.on_ground() {
            self.set_mob_speed((wanted_speed * movement_speed) as f32);
            if state.tick_jump_delay(cube_jump_delay()) {
                self.jump_control_jump();
                if self.size() > 0 {
                    self.play_sound(
                        self.jump_sound(),
                        cube_sound_volume(self.size()),
                        cube_sound_pitch(self.is_tiny()),
                    );
                }
            } else {
                self.set_mob_speed(0.0);
            }
        } else {
            self.set_mob_speed((wanted_speed * movement_speed) as f32);
        }

        *self.move_state.lock() = state;
    }

    /// Vanilla `SulfurCube.getSquishSound`.
    #[must_use]
    pub fn squish_sound(&self) -> SoundEventRef {
        if self.is_tiny() {
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_SQUISH
        } else if self.has_body_item() {
            &sound_events::ENTITY_SULFUR_CUBE_BOUNCE
        } else {
            &sound_events::ENTITY_SULFUR_CUBE_SQUISH
        }
    }

    /// Vanilla `SulfurCube.getJumpSound`.
    #[must_use]
    pub fn jump_sound(&self) -> SoundEventRef {
        if self.is_tiny() {
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_JUMP
        } else {
            &sound_events::ENTITY_SULFUR_CUBE_JUMP
        }
    }

    /// Vanilla `SulfurCube.getAbsorbSound`.
    #[must_use]
    pub fn absorb_sound() -> SoundEventRef {
        &sound_events::ENTITY_SULFUR_CUBE_ABSORB
    }

    /// Vanilla `SulfurCube.getEjectSound`.
    #[must_use]
    pub fn eject_sound() -> SoundEventRef {
        &sound_events::ENTITY_SULFUR_CUBE_EJECT
    }

    /// Vanilla `SulfurCube.getSplitCount`.
    fn split_count(&self) -> i32 {
        if self.is_primed() { 0 } else { SPLIT_COUNT }
    }

    /// Vanilla `AbstractCubeMob.remove` split-on-death with `SulfurCube.setUpSplitCube`.
    fn split_on_death(&self, world: &Arc<World>) {
        let size = self.size();
        if size <= 1 || !LivingEntity::is_dead_or_dying(self) {
            return;
        }

        let width = self.cube_dimensions().width;
        let xz_cube_spawn_offset = width / 2.0;
        let half_size = size / 2;
        let count = self.split_count();
        let entity_type = self.entity_type;
        let position = self.position();

        for i in 0..count {
            let xd = (i % 2) as f32 - 0.5;
            let zd = (i / 2) as f32 - 0.5;
            let child_position = DVec3::new(
                position.x + f64::from(xd * xz_cube_spawn_offset),
                position.y + 0.5,
                position.z + f64::from(zd * xz_cube_spawn_offset),
            );
            let Some(child) = ENTITIES.create(
                entity_type,
                next_entity_id(),
                child_position,
                Arc::downgrade(world),
            ) else {
                continue;
            };

            if let Some(cube) = child
                .as_entity_event_source()
                .downcast_ref::<SulfurCubeEntity>()
            {
                cube.set_size(half_size, true);
                AgeableMob::set_baby(cube, true);
            }
            child.snap_to(child_position, rand::random::<f32>() * 360.0, 0.0);
            let _ = world.try_add_entity(child);
        }
    }

    /// Vanilla `SulfurCube.applyContactDamage`.
    fn apply_contact_damage(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        let contact_damages = self.state.lock().contact_damages.clone();
        if contact_damages.is_empty() {
            return;
        }

        let this = world.get_entity_by_id(self.id());
        for damage in contact_damages {
            let Some(damage_type) = REGISTRY.damage_types.by_key(&damage.damage_type) else {
                log::error!(
                    "sulfur cube archetype references unknown damage type {:?}",
                    damage.damage_type
                );
                continue;
            };

            let attributes_to_source = damage.attribute_to_source || entity.as_player().is_some();
            let source = match (attributes_to_source, &this) {
                (true, Some(this)) => DamageSource::direct(damage_type, Arc::clone(this)),
                _ => DamageSource::environment(damage_type),
            };

            entity.hurt(world.as_ref(), &source, damage.amount);
        }
    }

    /// Vanilla `SulfurCube.playerPush`.
    fn player_push(&self, player: &Arc<Player>) {
        if !self.has_body_item() {
            return;
        }

        let player_entity: SharedEntity = Arc::<Player>::clone(player);
        let pusher = if player.is_passenger() {
            player
                .root_vehicle()
                .unwrap_or_else(|| Arc::clone(&player_entity))
        } else {
            Arc::clone(&player_entity)
        };

        let cube_to_pusher = self.position() - pusher.position();
        let pusher_feet_position = pusher.position().y;
        let sulfur_cube_bottom_position = self.position().y;
        let sulfur_cube_top_position = sulfur_cube_bottom_position + self.bounding_box().height();
        let pusher_top_position = pusher_feet_position + pusher.bounding_box().height();

        if cube_to_pusher.x.hypot(cube_to_pusher.z) >= PUSH_DISTANCE_THRESHOLD
            || pusher_feet_position > sulfur_cube_top_position
            || pusher_top_position <= sulfur_cube_bottom_position
        {
            return;
        }

        let knockback = (1.0 - self.knockback_resistance()).max(0.0);
        let horizontal = DVec3::new(cube_to_pusher.x, 0.0, cube_to_pusher.z).normalize_or_zero();
        let push_direction = horizontal * knockback;

        let push_speed_scale = if player.is_passenger() {
            f64::from(VEHICLE_PUSH_SPEED_SCALE_MULTIPLIER)
        } else {
            f64::from(PLAYER_PUSH_SPEED_SCALE_MULTIPLIER)
        };
        let player_speed = (player.known_speed().length() * 2.0 * push_speed_scale)
            .clamp(0.0, MAX_PLAYER_PUSH_SPEED);

        let vertical = if self.on_ground() {
            knockback * f64::from(VERTICAL_PUSH_MULTIPLIER)
        } else {
            0.0
        };
        let push_velocity = DVec3::new(push_direction.x, vertical, push_direction.z) * player_speed;

        self.mark_velocity_sync();

        let (threshold, cooldown_seconds) = {
            let state = self.state.lock();
            (
                state.push_sound_impulse_threshold(),
                state.push_sound_cooldown_seconds(),
            )
        };
        let mut play_push_sound = false;
        {
            let mut state = self.state.lock();
            if push_velocity.length_squared() > f64::from(threshold * threshold)
                && state.push_sound_cooldown <= 0
            {
                state.push_sound_cooldown = (cooldown_seconds * 20.0) as i32;
                play_push_sound = true;
            }
        }
        if play_push_sound {
            self.play_sound(self.state.lock().push_sound(), 1.0, 1.0);
        }

        self.set_velocity(self.velocity() + push_velocity);
        self.mark_velocity_sync();
        self.apply_contact_damage(&player_entity);
    }

    /// Vanilla `SulfurCube.knockback` archetype-aware branch.
    ///
    /// Returns `false` when vanilla would fall through to `LivingEntity.knockback`
    /// (no source entity, or no swallowed block).
    fn sulfur_knockback(
        &self,
        power: f64,
        xd: f64,
        zd: f64,
        source: &DamageSource,
        damage: f32,
        comes_from_effect: bool,
    ) -> bool {
        let Some(attacker) = source.causing_entity() else {
            return false;
        };
        if !self.has_body_item() {
            return false;
        }

        let (horizontal_power, vertical_power, hit_sound) = {
            let state = self.state.lock();
            let modifiers = state.knockback_modifiers();
            (
                modifiers.horizontal_power,
                modifiers.vertical_power,
                state.hit_sound(),
            )
        };
        let original_horizontal_power = horizontal_power;
        let original_vertical_power = vertical_power;

        let attacker_position = attacker.position();
        let attacker_eye_position =
            attacker_position + DVec3::new(0.0, attacker.get_eye_height(), 0.0);
        let attacker_aim_direction = attacker.look_angle().normalize_or_zero();
        let target_center = self.bounding_box().center();
        let target_height = self.bounding_box().height();

        let original_angle = Vec2::new(xd as f32, zd as f32);
        let new_angle = apply_horizontal_hit_angle_scale(
            HORIZONTAL_HIT_ANGLE_SCALE,
            original_angle,
            attacker_eye_position,
            attacker_aim_direction,
            target_center,
        );
        let mut new_power = apply_vertical_hit_angle_power_transfer(
            VERTICAL_HIT_ANGLE_SCALE,
            horizontal_power,
            vertical_power,
            attacker_eye_position,
            attacker_aim_direction,
            target_center,
            target_height as f32,
        );
        new_power = apply_vertical_position_angle_power_rotation(
            VERTICAL_POSITION_ANGLE_SCALE,
            new_power.x,
            new_power.y,
            original_horizontal_power,
            original_vertical_power,
            attacker_position,
            self.position(),
        );

        let xd = f64::from(new_angle.x);
        let zd = f64::from(new_angle.y);
        let mut horizontal_power = new_power.x;
        let mut vertical_power = new_power.y;

        let power_multiplier = damage.sqrt()
            * if comes_from_effect {
                power as f32 * EXTRA_KNOCKBACK_DAMPENING
            } else {
                1.0
            };
        horizontal_power *= power_multiplier;
        vertical_power *= power_multiplier;

        let knockback_resistance = (1.0 - self.knockback_resistance()) as f32;
        horizontal_power *= knockback_resistance;
        vertical_power *= knockback_resistance;

        self.mark_velocity_sync();
        let delta_movement = self.velocity();
        horizontal_power *= HORIZONTAL_KNOCKBACK_MULTIPLIER;
        horizontal_power = horizontal_power.clamp(-KNOCKBACK_POWER_LIMIT, KNOCKBACK_POWER_LIMIT);
        vertical_power = vertical_power.clamp(-KNOCKBACK_POWER_LIMIT, KNOCKBACK_POWER_LIMIT);

        let horizontal_knockback =
            DVec3::new(xd, 0.0, zd).normalize_or_zero() * f64::from(horizontal_power);
        self.set_velocity(DVec3::new(
            delta_movement.x - horizontal_knockback.x,
            delta_movement.y + f64::from(vertical_power) * VERTICAL_KNOCKBACK_MULTIPLIER,
            delta_movement.z - horizontal_knockback.z,
        ));
        self.play_sound(hit_sound, 1.0, 1.0);
        true
    }

    /// Vanilla `SulfurCube.equipItem`.
    pub fn equip_item(&self, held_item: &ItemStack) -> bool {
        if AgeableMob::is_baby(self) {
            return false;
        }

        if self.has_body_item() {
            let swallowed_item = self.equipment_in_slot(EquipmentSlot::Body);
            if held_item.is(swallowed_item.item()) {
                return false;
            }

            let offset = self
                .entity_type
                .dimensions
                .attachments
                .get_average(EntityAttachment::Passenger, self.cube_dimensions());
            let _ = self.spawn_at_location_with_offset(swallowed_item, offset);
        }

        self.set_body_item(held_item.copy_with_count(1));
        self.sync_archetype_from_body();
        self.play_sound(Self::absorb_sound(), 1.0, 1.0);
        true
    }

    /// Vanilla `LivingEntity.setItemSlotAndDropWhenKilled` for the `BODY` slot.
    fn set_body_item(&self, item_stack: ItemStack) {
        self.living_base
            .equipment()
            .lock()
            .set(EquipmentSlot::Body, item_stack);
        self.set_guaranteed_drop(EquipmentSlot::Body);
    }

    /// Vanilla `SulfurCube.readyForShearing`.
    #[must_use]
    pub fn ready_for_shearing(&self) -> bool {
        self.has_body_item()
    }

    /// Vanilla `SulfurCube.shear`.
    pub fn shear(&self) {
        let offset = self
            .entity_type
            .dimensions
            .attachments
            .get_average(EntityAttachment::Passenger, self.cube_dimensions());
        let item_stack_to_shear = self.equipment_in_slot(EquipmentSlot::Body);
        self.living_base
            .equipment()
            .lock()
            .set(EquipmentSlot::Body, ItemStack::empty());
        let _ = self.spawn_at_location_with_offset(item_stack_to_shear, offset);
        self.play_sound(Self::eject_sound(), 1.0, 1.0);
        self.state.lock().pickup_timer = PICKUP_TIMER_DURATION;
        self.sync_archetype_from_body();
    }

    /// Vanilla `SulfurCube.playEatingSound`.
    fn play_eating_sound(&self) {
        self.play_sound(
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_EAT,
            self.sound_volume(),
            self.voice_pitch(),
        );
    }

    /// Vanilla `SulfurCube.getBucketItemStack`.
    #[must_use]
    pub fn bucket_item_stack() -> ItemStack {
        ItemStack::new(&vanilla_items::SULFUR_CUBE_BUCKET)
    }

    /// Vanilla `SulfurCube.getPickupSound`.
    #[must_use]
    pub fn pickup_sound() -> SoundEventRef {
        &sound_events::ITEM_BUCKET_FILL_SULFUR_CUBE
    }

    /// Vanilla `SulfurCube.saveToBucketTag` (`Bucketable.saveDefaultDataToBucketTag` plus
    /// the sulfur-cube content and age payload).
    fn create_bucket_stack(&self) -> ItemStack {
        let mut stack = Self::bucket_item_stack();

        let mut tag = NbtCompound::new();
        if self.is_no_ai() {
            tag.insert("NoAI", 1i8);
        }
        if self.is_silent() {
            tag.insert("Silent", 1i8);
        }
        if self.is_no_gravity() {
            tag.insert("NoGravity", 1i8);
        }
        if self.has_glowing_tag() {
            tag.insert("Glowing", 1i8);
        }
        if self.is_invulnerable() {
            tag.insert("Invulnerable", 1i8);
        }
        if self.is_persistence_required() {
            tag.insert("PersistenceRequired", 1i8);
        }
        tag.insert("Health", self.get_health());
        tag.insert("age", self.get_age());
        tag.insert("age_locked", i8::from(self.is_age_locked()));

        if let Some(custom_data) = CustomData::try_from_compound(tag) {
            stack.set(BUCKET_ENTITY_DATA, custom_data);
        }
        if let Some(custom_name) = self.custom_name() {
            stack.set(CUSTOM_NAME, custom_name);
        }

        let body_item = self.equipment_in_slot(EquipmentSlot::Body);
        if !body_item.is_empty()
            && let Ok(template) = ItemStackTemplate::from_stack(&body_item)
        {
            stack.set(SULFUR_CUBE_CONTENT, SulfurCubeContent::new(template));
        }

        stack
    }

    /// Vanilla `SulfurCube.loadFromBucketTag`.
    pub fn load_from_bucket_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        if let Some(no_ai) = nbt.byte("NoAI") {
            self.set_no_ai(no_ai != 0);
        }
        if let Some(silent) = nbt.byte("Silent") {
            self.set_silent(silent != 0);
        }
        if let Some(no_gravity) = nbt.byte("NoGravity") {
            self.set_no_gravity(no_gravity != 0);
        }
        if let Some(glowing) = nbt.byte("Glowing") {
            self.set_glowing_tag(glowing != 0);
        }
        if let Some(invulnerable) = nbt.byte("Invulnerable") {
            self.set_invulnerable(invulnerable != 0);
        }
        if nbt
            .byte("PersistenceRequired")
            .is_some_and(|value| value != 0)
        {
            self.set_persistence_required();
        }
        if let Some(health) = nbt.float("Health") {
            self.set_health(health);
        }
        self.set_age(nbt.int("age").unwrap_or(0));
        self.set_age_locked(nbt.byte("age_locked").is_some_and(|value| value != 0));
    }

    /// Vanilla `Bucketable.bucketMobPickup` for the sulfur cube's plain bucket.
    fn try_bucket_pickup(&self, player: &Player, hand: InteractionHand) -> bool {
        let holds_bucket = {
            let inventory = player.inventory.lock();
            inventory.get_item_in_hand(hand).is(&vanilla_items::BUCKET)
        };
        if !holds_bucket || !Entity::is_alive(self) {
            return false;
        }

        self.play_sound(Self::pickup_sound(), 1.0, 1.0);
        let bucket = self.create_bucket_stack();
        let filled = bucket.clone();
        let overflow = {
            let mut inventory = player.inventory.lock();
            inventory.apply_filled_result(hand, bucket, player.has_infinite_materials(), false)
        };
        if !overflow.is_empty() {
            let _ = player.drop_item(overflow, false, Prediction::Predicted);
        }

        // Vanilla `Bucketable.bucketMobPickup` raises `filled_bucket` right after
        // `ItemUtils.createFilledResult`, before `Leashable.dropLeash`.
        if let Some(world) = Entity::level(self) {
            raise_filled_bucket(player, &world, &filled);
        }

        self.drop_leash();
        self.set_removed(RemovalReason::Discarded);
        true
    }

    /// Vanilla `SulfurCube.setSpawnSize`.
    fn set_spawn_size(&self) {
        if AgeableMob::is_baby(self) {
            self.set_size(MIN_SIZE, true);
        } else {
            self.set_size(MAX_SIZE, true);
        }
    }
}

/// Vanilla `AbstractCubeMob.CubeMobFloatGoal`.
struct SulfurCubeFloatGoal;

impl SulfurCubeFloatGoal {
    const fn new() -> Self {
        Self
    }
}

impl Goal for SulfurCubeFloatGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::JUMP | GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        sulfur_cube_of(mob).is_some() && (mob.is_in_water() || mob.is_in_lava())
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(cube) = sulfur_cube_of(mob) else {
            return;
        };
        if rand::random::<f32>() < 0.8 {
            mob.jump_control_jump();
        }
        cube.move_state.lock().set_wanted_movement(1.2);
    }
}

/// Vanilla `AbstractCubeMob.CubeMobKeepOnJumpingGoal`.
struct SulfurCubeKeepOnJumpingGoal;

impl SulfurCubeKeepOnJumpingGoal {
    const fn new() -> Self {
        Self
    }
}

impl Goal for SulfurCubeKeepOnJumpingGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::JUMP | GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !mob.is_passenger()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(cube) = sulfur_cube_of(mob) else {
            return;
        };
        cube.move_state.lock().set_wanted_movement(1.0);
    }
}

/// Vanilla `AbstractCubeMob.CubeMobRandomDirectionGoal`.
struct SulfurCubeRandomDirectionGoal {
    chosen_degrees: f32,
    next_randomize_time: i32,
}

impl SulfurCubeRandomDirectionGoal {
    const fn new() -> Self {
        Self {
            chosen_degrees: 0.0,
            next_randomize_time: 0,
        }
    }
}

impl Goal for SulfurCubeRandomDirectionGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target().is_none()
            && (mob.on_ground()
                || mob.is_in_water()
                || mob.is_in_lava()
                || mob.has_mob_effect(vanilla_mob_effects::LEVITATION))
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        self.next_randomize_time -= 1;
        if self.next_randomize_time <= 0 {
            // Vanilla `adjustedTickDelay(40 + random.nextInt(60))`.
            self.next_randomize_time = 20 + rand::random_range(0..30);
            self.chosen_degrees = rand::random_range(0..360) as f32;
        }
        if let Some(cube) = sulfur_cube_of(mob) {
            cube.move_state
                .lock()
                .set_direction(self.chosen_degrees, false);
        }
    }
}

/// Vanilla `SulfurCube.SulfurCubeSearchForItemsGoal`.
struct SulfurCubeSearchForItemsGoal {
    target_item: Option<SharedEntity>,
}

impl SulfurCubeSearchForItemsGoal {
    const fn new() -> Self {
        Self { target_item: None }
    }
}

impl Goal for SulfurCubeSearchForItemsGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(cube) = sulfur_cube_of(mob) else {
            return false;
        };
        if AgeableMob::is_baby(cube) || cube.state.lock().pickup_timer > 0 {
            return false;
        }

        self.target_item = nearest_allowed_item(cube);
        self.target_item.is_some()
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {}

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.target_item = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(item) = &self.target_item else {
            return;
        };
        let Some(cube) = sulfur_cube_of(mob) else {
            return;
        };

        mob.mob_base()
            .controls()
            .lock()
            .look_control
            .set_look_at(item.position(), 10.0, 10.0);

        let (yaw, _pitch) = mob.rotation();
        cube.move_state.lock().set_direction(yaw, true);
    }
}

/// Finds the nearest pickup-ready item entity inside vanilla's inflated search box.
fn nearest_allowed_item(cube: &SulfurCubeEntity) -> Option<SharedEntity> {
    let world = cube.level()?;
    let area =
        cube.bounding_box()
            .inflate_xyz(ITEM_SEARCH_RANGE, ITEM_SEARCH_RANGE, ITEM_SEARCH_RANGE);

    let candidates = world.get_entities_in_aabb_matching(&area, |entity| {
        entity.downcast_ref::<ItemEntity>().is_some_and(|item| {
            !item.has_pickup_delay()
                && item.is_alive()
                && SulfurCubeEntity::is_swallowable_item(&item.get_item())
        })
    });

    let position = cube.position();
    candidates.into_iter().min_by(|a, b| {
        let a_distance = a.position().distance_squared(position);
        let b_distance = b.position().distance_squared(position);
        a_distance.total_cmp(&b_distance)
    })
}

/// Vanilla `SulfurCube.SulfurCubeTemptGoal` (`TemptGoal.ForNonPathfinders`).
///
/// Steel's shared [`TemptGoal`](crate::entity::ai::goal::TemptGoal) drives path
/// navigation, so the non-pathfinder variant is expressed directly on the cube
/// move control here.
struct SulfurCubeTemptGoal {
    player: Option<Arc<Player>>,
    calm_down: i32,
}

impl SulfurCubeTemptGoal {
    const fn new() -> Self {
        Self {
            player: None,
            calm_down: 0,
        }
    }

    /// Vanilla `SulfurCubeTemptGoal` item predicate: food for babies, swallowable otherwise.
    fn matches_item(cube: &SulfurCubeEntity, item_stack: &ItemStack) -> bool {
        if AgeableMob::is_baby(cube) {
            SulfurCubeEntity::is_food(item_stack)
        } else {
            SulfurCubeEntity::is_swallowable_item(item_stack)
        }
    }
}

impl Goal for SulfurCubeTemptGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.calm_down > 0 {
            self.calm_down -= 1;
            return false;
        }

        let Some(cube) = sulfur_cube_of(mob) else {
            return false;
        };
        let Some(world) = mob.level() else {
            return false;
        };

        let range = mob
            .attributes()
            .lock()
            .required_value(vanilla_attributes::TEMPT_RANGE);
        let targeting_conditions = TargetingConditions::for_non_combat()
            .ignore_line_of_sight()
            .range(range);

        self.player = world.nearest_player(mob.position(), range, |player| {
            targeting_conditions.test(world.as_ref(), Some(mob), player)
                && player.is_holding(&mut |item_stack| {
                    SulfurCubeTemptGoal::matches_item(cube, item_stack)
                })
        });
        self.player.is_some()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_use(mob)
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {}

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.player = None;
        if let Some(cube) = sulfur_cube_of(mob) {
            // Vanilla `TemptGoal.ForNonPathfinders.stopNavigation` sets the move control to wait.
            cube.move_state.lock().set_wanted_movement(0.0);
        }
        self.calm_down = TEMPT_CALM_DOWN_TICKS;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(player) = &self.player else {
            return;
        };
        let Some(cube) = sulfur_cube_of(mob) else {
            return;
        };

        let player_position = player.position();
        mob.mob_base().controls().lock().look_control.set_look_at(
            DVec3::new(player_position.x, player.get_eye_y(), player_position.z),
            10.0,
            10.0,
        );

        if mob.position().distance_squared(player_position)
            < TEMPT_STOP_DISTANCE * TEMPT_STOP_DISTANCE
        {
            cube.move_state.lock().set_wanted_movement(0.0);
        } else {
            let (yaw, _pitch) = mob.rotation();
            cube.move_state.lock().set_direction(yaw, true);
        }
    }
}

/// Extracts the concrete sulfur cube behind a goal context.
fn sulfur_cube_of(mob: &dyn PathfinderMob) -> Option<&SulfurCubeEntity> {
    mob.as_entity_event_source()
        .downcast_ref::<SulfurCubeEntity>()
}

impl Entity for SulfurCubeEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        // Vanilla `SulfurCube.tick`: fuse first, then the powered-position check, then
        // `super.tick()` (the shared cube/living tick).
        self.sync_archetype_from_body();
        self.tick_fuse();
        self.prime_when_on_powered_position();
        self.tick_cube();
        self.tick_cube_movement();
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        self.cube_dimensions()
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    /// Vanilla `SulfurCube.maxUpStep`: a cube carrying a block cannot step up.
    fn max_up_step(&self) -> f32 {
        if self.has_body_item() {
            return 0.0;
        }
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(f64::from(DEFAULT_STEP_HEIGHT)) as f32
    }

    /// Vanilla `SulfurCube.getSoundSource`.
    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    /// Vanilla `SulfurCube.playStepSound` only plays without a swallowed block.
    fn play_step_sound(&self, _pos: BlockPos, block_state: BlockStateId) {
        if !self.has_body_item() {
            self.play_block_step_sound(block_state);
        }
    }

    /// Vanilla `SulfurCube.canFreeze`: a cube carrying a block never freezes.
    fn can_freeze(&self) -> bool {
        if self.has_body_item() {
            return false;
        }
        LivingEntity::default_living_can_freeze(self)
    }

    /// Vanilla `SulfurCube.getFluidJumpThreshold`.
    fn get_fluid_jump_threshold(&self) -> f64 {
        self.bounding_box().height() * 0.2
    }

    /// Vanilla `SulfurCube.hurtServer` fuse priming and block-immunity branch.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.has_body_item() {
            if self.can_explode() && !self.is_primed() {
                let burning_arrow = source.direct_entity().is_some_and(|direct| {
                    direct.entity_type().is_abstract_arrow && direct.is_on_fire()
                });
                if source.is(&DamageTypeTag::IS_FIRE) || burning_arrow {
                    self.prime_time(false);
                } else if source.is(&DamageTypeTag::IS_EXPLOSION) {
                    self.prime_time(true);
                }
            }

            if source.is(&DamageTypeTag::SULFUR_CUBE_WITH_BLOCK_IMMUNE_TO) {
                if !source.is(&DamageTypeTag::NO_KNOCKBACK) {
                    let (xd, zd) = self.damage_knockback_direction(source);
                    if !self.sulfur_knockback(DAMAGE_KNOCKBACK_POWER, xd, zd, source, amount, false)
                    {
                        self.knockback(DAMAGE_KNOCKBACK_POWER, xd, zd);
                    }
                }
                return true;
            }
        }

        LivingEntity::hurt_server(self, world, source, amount)
    }

    /// Vanilla `SulfurCube.applyImplicitComponent` for `SULFUR_CUBE_CONTENT`.
    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        let Some(content) = item_stack.get(SULFUR_CUBE_CONTENT) else {
            return;
        };
        self.set_body_item(content.absorbed_block_item_stack().create());
        self.sync_archetype_from_body();
    }

    /// Vanilla `AbstractCubeMob.addAdditionalSaveData` plus `SulfurCube.addAdditionalSaveData`.
    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_ageable_mob(nbt);
        self.save_mob(nbt);
        nbt.insert("Size", self.size() - 1);
        nbt.insert("wasOnGround", *self.was_on_ground.lock());
        nbt.insert("pickup_timer", self.state.lock().pickup_timer);
        nbt.insert("from_bucket", self.from_bucket());
        nbt.insert("fuse", self.fuse());
    }

    /// Vanilla `AbstractCubeMob.readAdditionalSaveData` plus `SulfurCube.readAdditionalSaveData`.
    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.state.lock().pickup_timer = nbt.int("pickup_timer").unwrap_or(0);
        self.set_from_bucket(nbt.byte("from_bucket").is_some_and(|value| value != 0));
        self.set_fuse(nbt.int("fuse").unwrap_or(-1));
        self.entity_data.lock().max_fuse.set(self.fuse());
        self.set_size(nbt.int("Size").unwrap_or(0) + 1, false);
        self.load_mob(nbt);
        *self.was_on_ground.lock() = nbt.byte("wasOnGround").is_some_and(|value| value != 0);
        self.load_ageable_mob(nbt);
    }

    /// Vanilla `SulfurCube.loadFromBucketTag`, restored by `MobBucketItem.spawn`
    /// on release.
    fn load_from_bucket_tag(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_from_bucket_data(nbt);
    }

    /// Vanilla `Bucketable.setFromBucket`, flipped for every released mob.
    fn set_from_bucket(&self, from_bucket: bool) {
        SulfurCubeEntity::set_from_bucket(self, from_bucket);
    }

    /// Vanilla `SulfurCube.playerTouch`.
    fn player_touch(self: Arc<Self>, player: &Arc<Player>) {
        self.player_push(player);
    }
}

impl LivingEntity for SulfurCubeEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    /// Vanilla `AbstractCubeMob.getSoundVolume`.
    fn sound_volume(&self) -> f32 {
        cube_sound_volume(self.size())
    }

    /// Vanilla `SulfurCube.getHurtSound`.
    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(if self.is_tiny() {
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_HURT
        } else {
            &sound_events::ENTITY_SULFUR_CUBE_HURT
        })
    }

    /// Vanilla `SulfurCube.getDeathSound`.
    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_tiny() {
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_DEATH
        } else {
            &sound_events::ENTITY_SULFUR_CUBE_DEATH
        })
    }

    /// Vanilla `SulfurCube.isEquippableInSlot`.
    fn is_equippable_in_slot(&self, item_stack: &ItemStack, slot: EquipmentSlot) -> bool {
        if slot == EquipmentSlot::Body {
            return Self::is_swallowable_item(item_stack);
        }

        let Some(equippable) = item_stack.get_equippable() else {
            return slot == EquipmentSlot::MainHand && self.can_use_slot(EquipmentSlot::MainHand);
        };

        slot == equippable.slot
            && self.can_use_slot(equippable.slot)
            && equippable.can_be_equipped_by(self.entity_type())
    }

    /// Vanilla `SulfurCube.canUseSlot`.
    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        if slot == EquipmentSlot::Body {
            return Entity::is_alive(self) && !AgeableMob::is_baby(self);
        }
        true
    }

    /// Vanilla `SulfurCube.canDispenserEquipIntoSlot`.
    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        slot == EquipmentSlot::Body
    }

    /// Vanilla `SulfurCube.canBreatheUnderwater`.
    fn can_breathe_underwater(&self) -> bool {
        self.has_body_item() || self.entity_type().flags.can_breathe_underwater
    }

    /// Vanilla `SulfurCube.travelInFluid` buoyancy.
    ///
    /// The shared fluid dispatch is repeated because the trait default body cannot be
    /// called separately from an override.
    fn travel_in_fluid(&self, entity: &SharedEntity, input: DVec3) -> Option<MoveResult> {
        let is_falling = self.velocity().y <= 0.0;
        let old_y = self.position().y;
        let base_gravity = self.get_effective_gravity();
        let result = if self.is_in_water() {
            let result = self.travel_in_water(entity, input, base_gravity, is_falling, old_y);
            self.float_in_water_while_ridden();
            result
        } else {
            self.travel_in_lava(entity, input, base_gravity, is_falling, old_y)
        };

        let floats_in_liquids = self.state.lock().floats_in_liquids;
        if self.has_body_item() && floats_in_liquids {
            let vibe_amount =
                FLOAT_VIBE_AMPLITUDE * (self.tick_count() as f32 * FLOAT_VIBE_FREQUENCY).sin();
            let fluid_height = if self.is_in_water() {
                self.fluid_contact().water_height()
            } else {
                self.fluid_contact().lava_height()
            };
            let immersion = fluid_height - self.get_fluid_jump_threshold() + f64::from(vibe_amount);
            if immersion > 0.0 {
                let velocity = self.velocity();
                self.set_velocity(
                    velocity + DVec3::new(0.0, immersion.min(1.0) * FLOAT_PUSH_PER_TICK, 0.0),
                );
            }
        }

        result
    }

    /// Vanilla `LivingEntity.doPush` plus `SulfurCube.doPush` contact damage.
    fn do_push(&self, entity: &SharedEntity) {
        entity.push_entity(self.as_entity_event_source());
        self.apply_contact_damage(entity);
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        result
    }

    /// Vanilla `SulfurCube.knockback` entry for the damage path.
    fn apply_damage_knockback(&self, source: &DamageSource, blocked: bool) {
        if source.is(&DamageTypeTag::NO_KNOCKBACK) {
            return;
        }

        let damage = self.state.lock().last_damage;
        let (xd, zd) = self.damage_knockback_direction(source);
        if self.sulfur_knockback(DAMAGE_KNOCKBACK_POWER, xd, zd, source, damage, false) {
            if !blocked {
                self.indicate_damage(xd, zd);
            }
            return;
        }

        // Shared `LivingEntity.apply_damage_knockback` body, inlined because a UFCS
        // call would dispatch back into this override.
        self.knockback(DAMAGE_KNOCKBACK_POWER, xd, zd);
        if !blocked {
            self.indicate_damage(xd, zd);
        }
    }

    /// Captures the damage amount vanilla `SulfurCube.knockback` scales by.
    fn before_actually_hurt(&self, _source: &DamageSource, amount: f32) {
        self.state.lock().last_damage = amount;
        if let Some(animal) = self.as_animal() {
            animal.reset_love();
        }
    }

    /// Vanilla `AbstractCubeMob.remove` splits a dying sulfur cube before removal.
    fn on_death(&self, _source: &DamageSource) {
        if let Some(world) = self.level() {
            self.split_on_death(&world);
        }
    }
}

impl Mob for SulfurCubeEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        None
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `AbstractCubeMob.getMaxHeadXRot`.
    fn max_head_x_rot(&self) -> f32 {
        0.0
    }

    /// Vanilla `SulfurCube.customServerAiStep` pickup and push-sound timers.
    fn custom_server_ai_step(&self) {
        let mut state = self.state.lock();
        if state.pickup_timer > 0 {
            state.pickup_timer -= 1;
        }
        if state.push_sound_cooldown > 0 {
            state.push_sound_cooldown -= 1;
        }
    }

    /// Vanilla `SulfurCube.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let data = AgeableMob::finalize_spawn_ageable_mob(self, world, spawn_reason, group_data);
        self.set_spawn_size();
        data
    }

    /// Vanilla `SulfurCube.getBaseExperienceReward`.
    fn base_experience_reward_mob(&self) -> i32 {
        if AgeableMob::is_baby(self) {
            0
        } else {
            1 + rand::random_range(0..2)
        }
    }

    /// Vanilla `SulfurCube.requiresCustomPersistence`.
    fn requires_custom_persistence(&self) -> bool {
        self.is_passenger() || self.is_leashed() || self.has_body_item() || self.from_bucket()
    }

    /// Vanilla `SulfurCube.canBeLeashed`.
    fn mob_can_be_leashed(&self) -> bool {
        self.has_body_item()
    }

    /// Vanilla `SulfurCube.canPickUpLoot`.
    fn can_pick_up_loot(&self) -> bool {
        !self.has_body_item()
    }

    /// Vanilla `SulfurCube.canHoldItem`.
    fn can_hold_item(&self, item_stack: &ItemStack) -> bool {
        !self.has_body_item() && Self::is_swallowable_item(item_stack) && !AgeableMob::is_baby(self)
    }

    /// Vanilla `SulfurCube.getEquipmentSlotForItem`.
    fn get_equipment_slot_for_item(&self, item_stack: &ItemStack) -> EquipmentSlot {
        if Self::is_swallowable_item(item_stack) {
            return EquipmentSlot::Body;
        }

        match item_stack.get_equippable() {
            Some(equippable) if self.can_use_slot(equippable.slot) => equippable.slot,
            _ => EquipmentSlot::MainHand,
        }
    }

    /// Vanilla `SulfurCube.pickUpItem` with the swallow pickup timer.
    fn pick_up_item(&self, world: &Arc<World>, item_entity: &ItemEntity) {
        let item_stack = item_entity.get_item();
        if !self.can_hold_item(&item_stack) || self.state.lock().pickup_timer > 0 {
            return;
        }

        self.set_body_item(item_stack.copy_with_count(1));
        self.play_sound(Self::absorb_sound(), 1.0, 1.0);
        self.sync_archetype_from_body();

        let chunk_pos = ChunkPos::from_entity_pos(item_entity.position());
        world.broadcast_to_nearby(
            chunk_pos,
            CTakeItemEntity::new(item_entity.id(), self.id(), 1),
            None,
        );

        let mut remaining = item_entity.get_item();
        remaining.shrink(1);
        if remaining.is_empty() {
            item_entity.set_removed(RemovalReason::Discarded);
        } else {
            item_entity.set_item(remaining);
        }
    }

    /// Vanilla `SulfurCube.mobInteract`.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let held_item = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if AgeableMob::is_baby(self) {
            if Self::is_food(&held_item) && AgeableMob::can_age_up(self) {
                let age = AgeableMob::get_age(self);
                Mob::use_player_item(self, player, hand);
                AgeableMob::age_up(
                    self,
                    AgeableMobBase::get_speed_up_seconds_when_feeding(-age),
                    true,
                );
                self.play_eating_sound();
                return InteractionResult::Success;
            }

            return AgeableMob::mob_interact_ageable(self, player, hand);
        }

        if self.is_primed() {
            return InteractionResult::Pass;
        }

        let is_igniter = held_item.is(&vanilla_items::FLINT_AND_STEEL)
            || held_item.is(&vanilla_items::FIRE_CHARGE);
        if !self.can_explode() || !is_igniter {
            if held_item.is(&vanilla_items::SHEARS) && self.ready_for_shearing() {
                self.shear();
                self.game_event_with_source_entity(
                    &vanilla_game_events::SHEAR,
                    Some(player as &dyn Entity),
                );
                {
                    let mut inventory = player.inventory.lock();
                    inventory
                        .get_item_in_hand_mut(hand)
                        .hurt_and_break(1, player.has_infinite_materials());
                }
                // Vanilla `SulfurCube.mobInteract` raises
                // `player_sheared_equipment` with the shears and this cube, after
                // the shears took their durability damage.
                raise_player_sheared_equipment(player, self, &held_item);
                return InteractionResult::Success;
            }

            if Self::is_swallowable_item(&held_item) {
                let worked = self.equip_item(&held_item);
                if worked {
                    {
                        let mut inventory = player.inventory.lock();
                        inventory
                            .get_item_in_hand_mut(hand)
                            .consume(1, player.has_infinite_materials());
                    }
                    self.game_event(&vanilla_game_events::ENTITY_INTERACT);
                }
                return if worked {
                    InteractionResult::SuccessServer
                } else {
                    InteractionResult::Pass
                };
            }

            if self.try_bucket_pickup(player, hand) {
                return InteractionResult::Success;
            }
            return AgeableMob::mob_interact_ageable(self, player, hand);
        }

        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };
        if !world.get_game_rule(&TNT_EXPLODES) {
            player.send_overlay_message(&TextComponent::translated(
                BLOCK_MINECRAFT_TNT_DISABLED.msg(),
            ));
            return InteractionResult::Pass;
        }

        self.prime_time(false);
        {
            let mut inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand_mut(hand);
            if item_stack.is(&vanilla_items::FLINT_AND_STEEL) {
                item_stack.hurt_and_break(1, player.has_infinite_materials());
            } else {
                item_stack.consume(1, player.has_infinite_materials());
            }
        }

        player.award_stat(&vanilla_stat_types::ITEM_USED, held_item.item());
        InteractionResult::SuccessServer
    }

    /// Vanilla `SulfurCube.checkSulfurCubeSpawnRules`.
    ///
    /// Vanilla registers the cube with `SpawnPlacementTypes.ON_GROUND` and a predicate
    /// that always returns `true`; Steel folds the placement block check into this
    /// shared rule, so spawners bypass it exactly like vanilla and every other
    /// spawn reason still requires a valid ground position.
    fn check_mob_spawn_rules(
        entity_type: EntityTypeRef,
        level: &dyn LevelReader,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        if spawn_reason.is_spawner() {
            return true;
        }

        // Shared `Mob.check_mob_spawn_rules` body, inlined because a UFCS call would
        // dispatch back into this override.
        let below = pos.below();
        let state = level.get_block_state(below);
        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .is_valid_spawn(state, level, below, entity_type)
    }

    /// Vanilla `SulfurCube` look control: with a swallowed block the body yaw snaps to the
    /// nearest 90° axis and the head follows it.
    fn tick_look_control(&self) {
        if self.has_body_item() {
            let (yaw, pitch) = self.rotation();
            let close_angle = wrap_degrees_90(yaw);
            let new_yaw = yaw - close_angle;
            self.set_rotation((new_yaw, pitch));
            self.set_y_head_rot(new_yaw);
            return;
        }

        // Shared `Mob.tick_look_control` body, inlined because a UFCS call would
        // dispatch back into this override.
        let look_control = {
            let mut controls = self.mob_base().controls().lock();
            let look_control = controls.look_control;
            controls.look_control.tick_cooldown();
            look_control
        };

        let mut rotation = self.rotation();
        rotation.1 = 0.0;
        if look_control.is_looking_at_target() {
            let position = self.position();
            let wanted_position = look_control.wanted_position();
            let xd = wanted_position.x - position.x;
            let yd = wanted_position.y - self.get_eye_y();
            let zd = wanted_position.z - position.z;
            let horizontal = xd.hypot(zd);
            if horizontal.abs() > 1.0e-5 || yd.abs() > 1.0e-5 {
                let target_pitch = -(yd.atan2(horizontal)) as f32 * RAD_TO_DEG;
                rotation.1 =
                    rotate_towards(rotation.1, target_pitch, look_control.x_max_rot_angle());
            }
            if zd.abs() > 1.0e-5 || xd.abs() > 1.0e-5 {
                let target_yaw = zd.atan2(xd) as f32 * RAD_TO_DEG - DEGREE_90;
                self.set_y_head_rot(rotate_towards(
                    self.y_head_rot(),
                    target_yaw,
                    look_control.y_max_rot_speed(),
                ));
            }
        } else {
            self.set_y_head_rot(rotate_towards(self.y_head_rot(), self.y_body_rot(), 10.0));
        }

        self.set_rotation(rotation);
        self.clamp_head_rotation_to_body_when_pathing();
    }
}

impl AgeableMob for SulfurCubeEntity {
    fn ageable_base(&self) -> &AgeableMobBase {
        &self.ageable_base
    }

    fn is_age_locked(&self) -> bool {
        *self.entity_data.lock().ageable_mob().age_locked.get()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.entity_data
            .lock()
            .ageable_mob_mut()
            .age_locked
            .set(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.entity_data.lock().ageable_mob_mut().baby.set(baby);
    }

    /// Vanilla `SulfurCube.ageBoundaryReached`: adults grow back to size 2.
    fn age_boundary_changed(&self, baby: bool) {
        self.refresh_dimensions();
        if !baby {
            self.set_size(MAX_SIZE, true);
        }
    }

    /// Vanilla `SulfurCube.getBreedOffspring` produces a size-1 cube.
    fn initialize_breed_offspring(&self, _partner: &dyn AgeableMob, offspring: &dyn AgeableMob) {
        if let Some(cube) = offspring
            .as_entity_event_source()
            .downcast_ref::<SulfurCubeEntity>()
        {
            cube.set_size(MIN_SIZE, true);
        }
    }
}

impl PathfinderMob for SulfurCubeEntity {}

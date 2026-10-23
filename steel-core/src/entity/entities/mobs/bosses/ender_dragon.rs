//! Vanilla `EnderDragon` boss entity.
//!
//! Mirrors `net.minecraft.world.entity.boss.enderdragon.EnderDragon`: the
//! phase-driven flight model ([`super::phases`]), the 24-node flight graph, the
//! body/tail flight history, crystal healing, per-part damage scaling, block
//! destruction and the 200-tick death animation.
//!
//! Server-only paths are ported. Vanilla's client-only presentation (local flap
//! and growl sounds, `getViewVector` interpolation on the client, phase
//! `doClientTick` particles) is not ported because Steel has no client.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::{fast_floor, wrap_degrees};
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::level_events;
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::EnderDragonEntityData;
use steel_registry::vanilla_game_rules::{MOB_DROPS, MOB_GRIEFING};
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_damage_type_tags, vanilla_damage_types,
    vanilla_game_events, vanilla_particle_types,
};
use steel_utils::Downcast as _;
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::GameType;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, WorldAabb};

use crate::chunk::heightmap::HeightmapType;
use crate::entity::boss::dragon_fight;
use crate::entity::damage::DamageSource;
use crate::entity::entities::ExperienceOrbEntity;
use crate::entity::entities::objects::explosives::EndCrystalEntity;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource as _, EntityPose, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob,
    RemovalReason, SharedEntity,
};
use crate::physics::{MoveResult, MoverType};
use crate::world::{LevelReader, World};

use super::ender_dragon_part::{EnderDragonPartEntity, EnderDragonPartKind};
use super::phases::{EnderDragonPhase, EnderDragonPhaseManager};

/// Vanilla `GROWL_INTERVAL_MIN`.
const GROWL_INTERVAL_MIN: i32 = 200;
/// Vanilla `GROWL_INTERVAL_MAX`.
const GROWL_INTERVAL_MAX: i32 = 400;
/// Vanilla `SITTING_ALLOWED_DAMAGE_PERCENTAGE`.
const SITTING_ALLOWED_DAMAGE_PERCENTAGE: f32 = 0.25;
/// Vanilla `DRAGON_DEATH_TIME_KEY`.
const DRAGON_DEATH_TIME_KEY: &str = "DragonDeathTime";
/// Vanilla `DRAGON_PHASE_KEY`.
const DRAGON_PHASE_KEY: &str = "DragonPhase";
/// Vanilla `SITTING_DAMAGE_RECEIVED_KEY`.
const SITTING_DAMAGE_RECEIVED_KEY: &str = "sitting_damage_received";
/// Vanilla `DEFAULT_DEATH_TIME`.
const DEFAULT_DEATH_TIME: i32 = 0;
/// Vanilla `EnderDragon.DATA_PHASE` default.
const DEFAULT_PHASE: EnderDragonPhase = EnderDragonPhase::Hovering;

/// Number of flight-graph nodes in vanilla's `EnderDragon`.
pub(crate) const NODE_COUNT: usize = 24;
/// Vanilla `EnderDragon.nodeAdjacency`.
const NODE_ADJACENCY: [u32; NODE_COUNT] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131_392, 131_712, 263_424, 526_848, 525_313,
    1_581_057, 3_166_214, 2_138_120, 6_373_424, 4_358_208, 12_910_976, 9_044_480, 9_706_496,
    15_216_640, 13_688_832, 11_763_712, 8_257_536,
];
/// Vanilla `DragonFlightHistory.LENGTH`.
const FLIGHT_HISTORY_LENGTH: usize = 64;
/// Vanilla `DragonFlightHistory.MASK`.
const FLIGHT_HISTORY_MASK: i32 = 63;
/// Vanilla `CRYSTAL_DESTROY_TARGETING` range.
const CRYSTAL_DESTROY_TARGETING_RANGE: f64 = 64.0;
/// Crystal healing radius (`getBoundingBox().inflate(32.0)`).
const CRYSTAL_SCAN_RANGE: f64 = 32.0;
/// Crystal healing cadence (`tickCount % 10 == 0`).
const CRYSTAL_HEAL_INTERVAL: i32 = 10;
/// Vanilla `EntitySelector.NO_CREATIVE_OR_SPECTATOR` uses this box inflation.
const WING_PUSH_INFLATION: f64 = 4.0;
/// Vanilla `EnderDragon.knockBack` contact damage.
const WING_CONTACT_DAMAGE: f32 = 5.0;
/// Vanilla `EnderDragon.hurt(ServerLevel, List)` head/neck contact damage.
const HEAD_CONTACT_DAMAGE: f32 = 10.0;
/// Vanilla `EnderDragon.onCrystalDestroyed` head explosion damage.
const CRYSTAL_EXPLOSION_DAMAGE: f32 = 10.0;
/// Vanilla `EnderDragon.tickDeath` first-kill experience.
const FIRST_KILL_EXPERIENCE: i32 = 12000;
/// Vanilla `EnderDragon.tickDeath` repeat-kill experience.
const REPEAT_KILL_EXPERIENCE: i32 = 500;
/// Vanilla `EnderDragon.tickDeath` removal tick.
const DEATH_ANIMATION_TICKS: i32 = 200;
/// Vanilla `EnderDragon.getSoundVolume`.
const DRAGON_SOUND_VOLUME: f32 = 5.0;
/// Vanilla `EnderDragon` initial `growlTime`.
const INITIAL_GROWL_TIME: i32 = 100;
/// Vanilla `EnderDragon.getHeadYOffset` sitting offset.
const SITTING_HEAD_Y_OFFSET: f32 = -1.0;
/// Vanilla `EnderDragon.aiStep` death movement per tick.
const DEATH_MOVE: DVec3 = DVec3::new(0.0, 0.1, 0.0);
/// `Vec3.normalize` treats squared lengths below this as zero.
const VANILLA_NORMALIZE_EPSILON_SQR: f64 = 1.0e-4;

/// Vanilla `DragonFlightHistory.Sample`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DragonFlightSample {
    /// Sampled `y` coordinate.
    pub(crate) y: f64,
    /// Sampled yaw.
    pub(crate) y_rot: f32,
}

/// Vanilla `DragonFlightHistory`: a 64-sample ring buffer of flight state.
///
/// The body, wings, head, neck and tail positions are all derived from delayed
/// samples of this history, which is what produces the dragon's snake-like body.
#[derive(Debug, Clone)]
pub(crate) struct DragonFlightHistory {
    samples: [DragonFlightSample; FLIGHT_HISTORY_LENGTH],
    head: i32,
}

impl DragonFlightHistory {
    /// Creates a history filled with vanilla's zero samples.
    pub(crate) fn new() -> Self {
        Self {
            samples: [DragonFlightSample { y: 0.0, y_rot: 0.0 }; FLIGHT_HISTORY_LENGTH],
            head: -1,
        }
    }

    /// Vanilla `DragonFlightHistory.record`.
    pub(crate) fn record(&mut self, y: f64, y_rot: f32) {
        let sample = DragonFlightSample { y, y_rot };
        if self.head < 0 {
            self.samples = [sample; FLIGHT_HISTORY_LENGTH];
        }
        self.head += 1;
        if self.head == FLIGHT_HISTORY_LENGTH as i32 {
            self.head = 0;
        }
        self.samples[self.head as usize] = sample;
    }

    /// Vanilla `DragonFlightHistory.get(int delay)`.
    pub(crate) fn get(&self, delay: i32) -> DragonFlightSample {
        self.samples[((self.head - delay) & FLIGHT_HISTORY_MASK) as usize]
    }
}

/// A node of vanilla's fixed 24-node ender-dragon flight graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DragonNode {
    /// Node x.
    pub(crate) x: i32,
    /// Node y.
    pub(crate) y: i32,
    /// Node z.
    pub(crate) z: i32,
}

impl DragonNode {
    /// Creates a graph node.
    #[must_use]
    pub(crate) const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Vanilla `Node.distanceTo`.
    fn distance_to(&self, other: &Self) -> f32 {
        let xd = (other.x - self.x) as f32;
        let yd = (other.y - self.y) as f32;
        let zd = (other.z - self.z) as f32;
        xd.mul_add(xd, yd.mul_add(yd, zd * zd)).sqrt()
    }

    /// Vanilla `Node.distanceToSqr`.
    fn distance_to_sqr(&self, other: &Self) -> f32 {
        let xd = (other.x - self.x) as f32;
        let yd = (other.y - self.y) as f32;
        let zd = (other.z - self.z) as f32;
        xd.mul_add(xd, yd.mul_add(yd, zd * zd))
    }

    const fn as_block_pos(&self) -> BlockPos {
        BlockPos::new(self.x, self.y, self.z)
    }
}

/// Vanilla `Path` restricted to the dragon's fixed 24-node flight graph.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DragonPath {
    nodes: Vec<DragonNode>,
    next_node_index: usize,
}

impl DragonPath {
    fn new(nodes: Vec<DragonNode>) -> Self {
        Self {
            nodes,
            next_node_index: 0,
        }
    }

    /// Vanilla `Path.isDone`.
    pub(crate) const fn is_done(&self) -> bool {
        self.next_node_index >= self.nodes.len()
    }

    /// Vanilla `Path.advance`.
    pub(crate) const fn advance(&mut self) {
        self.next_node_index += 1;
    }

    /// Vanilla `Path.getNextNodePos`.
    pub(crate) fn next_node_pos(&self) -> Option<BlockPos> {
        self.nodes
            .get(self.next_node_index)
            .map(DragonNode::as_block_pos)
    }
}

/// Search bookkeeping for one A* run over the dragon's flight graph.
struct DragonSearchNode {
    node: DragonNode,
    came_from: Option<usize>,
    g: f32,
    h: f32,
    f: f32,
    closed: bool,
    in_open_set: bool,
}

impl DragonSearchNode {
    fn new(node: DragonNode) -> Self {
        Self {
            node,
            came_from: None,
            g: 0.0,
            h: 0.0,
            f: 0.0,
            closed: false,
            in_open_set: false,
        }
    }
}

/// Mutable dragon state that is not part of the vanilla synced entity data.
struct DragonState {
    flight_history: DragonFlightHistory,
    /// Vanilla `oFlapTime`.
    o_flap_time: f32,
    /// Vanilla `flapTime`.
    flap_time: f32,
    /// Vanilla `inWall`.
    in_wall: bool,
    /// Vanilla `dragonDeathTime`.
    dragon_death_time: i32,
    /// Vanilla `yRotA`.
    y_rot_a: f32,
    /// Vanilla `nearestCrystal`, kept by UUID so a removed crystal is observable.
    nearest_crystal: Option<uuid::Uuid>,
    /// Vanilla `fightOrigin`.
    fight_origin: BlockPos,
    /// Vanilla `dragonFight != null`.
    dragon_fight_present: bool,
    /// Vanilla `growlTime`.
    growl_time: i32,
    /// Vanilla `sittingDamageReceived`.
    sitting_damage_received: f32,
    /// Vanilla `nodes`, built on first use from the world heightmap.
    nodes: Option<[DragonNode; NODE_COUNT]>,
    /// Vanilla `EnderDragonFight.aliveCrystals`, mirrored on the dragon.
    alive_crystals: i32,
    /// Vanilla `random`.
    random: LegacyRandom,
}

impl DragonState {
    fn new() -> Self {
        Self {
            flight_history: DragonFlightHistory::new(),
            o_flap_time: 0.0,
            flap_time: 0.0,
            in_wall: false,
            dragon_death_time: DEFAULT_DEATH_TIME,
            y_rot_a: 0.0,
            nearest_crystal: None,
            fight_origin: BlockPos::ZERO,
            dragon_fight_present: false,
            growl_time: INITIAL_GROWL_TIME,
            sitting_damage_received: 0.0,
            nodes: None,
            alive_crystals: 0,
            random: LegacyRandom::from_seed(rand::random()),
        }
    }
}

/// The eight vanilla `EnderDragonPart` hitboxes owned by a dragon.
pub(crate) struct EnderDragonParts {
    /// Vanilla `head`.
    pub(crate) head: EnderDragonPartEntity,
    /// Vanilla `neck`.
    pub(crate) neck: EnderDragonPartEntity,
    /// Vanilla `body`.
    pub(crate) body: EnderDragonPartEntity,
    /// Vanilla `tail1`.
    pub(crate) tail1: EnderDragonPartEntity,
    /// Vanilla `tail2`.
    pub(crate) tail2: EnderDragonPartEntity,
    /// Vanilla `tail3`.
    pub(crate) tail3: EnderDragonPartEntity,
    /// Vanilla `wing1`.
    pub(crate) wing1: EnderDragonPartEntity,
    /// Vanilla `wing2`.
    pub(crate) wing2: EnderDragonPartEntity,
}

impl EnderDragonParts {
    fn new(world: Weak<World>) -> Self {
        let make = |kind: EnderDragonPartKind| EnderDragonPartEntity::new(kind, world.clone());
        Self {
            head: make(EnderDragonPartKind::Head),
            neck: make(EnderDragonPartKind::Neck),
            body: make(EnderDragonPartKind::Body),
            tail1: make(EnderDragonPartKind::Tail1),
            tail2: make(EnderDragonPartKind::Tail2),
            tail3: make(EnderDragonPartKind::Tail3),
            wing1: make(EnderDragonPartKind::Wing1),
            wing2: make(EnderDragonPartKind::Wing2),
        }
    }

    /// Returns the parts in vanilla `subEntities` order.
    pub(crate) fn iter(&self) -> [&EnderDragonPartEntity; 8] {
        [
            &self.head,
            &self.neck,
            &self.body,
            &self.tail1,
            &self.tail2,
            &self.tail3,
            &self.wing1,
            &self.wing2,
        ]
    }

    /// Binds every part to the owning dragon once it has a shared handle.
    fn bind_parent(&self, parent: &SharedEntity) {
        for part in self.iter() {
            part.bind_parent(parent);
        }
    }
}

/// The ender dragon boss.
#[entity_behavior(class = "EnderDragon")]
pub struct EnderDragonEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<EnderDragonEntityData>,
    state: SyncMutex<DragonState>,
    parts: EnderDragonParts,
    phases: EnderDragonPhaseManager,
}

// SAFETY: The owner-scoped type key uniquely identifies EnderDragonEntity.
unsafe impl DowncastType for EnderDragonEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/ender_dragon");
}

impl EnderDragonEntity {
    /// Creates a new ender dragon.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world.clone()),
            entity_type,
            world,
        )
    }

    /// Loads a saved ender dragon.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        let world = load.world.clone();
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            world,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef, world: Weak<World>) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        let mut entity_data = EnderDragonEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.phase.set(DEFAULT_PHASE.id());

        // Vanilla `EnderDragon` constructor: `noPhysics = true`.
        base.set_no_physics(true);

        let dragon = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(DragonState::new()),
            parts: EnderDragonParts::new(world),
            phases: EnderDragonPhaseManager::new(),
        };
        // Vanilla `EnderDragonPhaseManager` constructor: `setPhase(HOVERING)`.
        dragon.phases.set_phase(&dragon, EnderDragonPhase::Hovering);
        dragon
    }

    /// Vanilla `setFightOrigin`.
    pub fn set_fight_origin(&self, origin: BlockPos) {
        self.state.lock().fight_origin = origin;
    }

    /// Vanilla `getFightOrigin`.
    #[must_use]
    pub fn fight_origin(&self) -> BlockPos {
        self.state.lock().fight_origin
    }

    /// Mirrors vanilla `dragonFight != null`.
    pub fn set_dragon_fight_present(&self, present: bool) {
        self.state.lock().dragon_fight_present = present;
    }

    /// Returns whether a dragon fight owns this dragon.
    #[must_use]
    pub fn dragon_fight_present(&self) -> bool {
        self.state.lock().dragon_fight_present
    }

    /// Vanilla `EnderDragonFight.aliveCrystals` as mirrored on the dragon.
    #[must_use]
    pub fn alive_crystals(&self) -> i32 {
        self.state.lock().alive_crystals
    }

    /// Stores the fight's alive crystal count.
    pub fn set_alive_crystals(&self, count: i32) {
        self.state.lock().alive_crystals = count;
    }

    /// Vanilla `Entity.setInvulnerable` for the dragon's permanent immunity.
    pub fn set_permanently_invulnerable(&self, value: bool) {
        self.set_invulnerable(value);
    }

    /// Returns whether the dragon is permanently invulnerable.
    #[must_use]
    pub fn permanently_invulnerable(&self) -> bool {
        self.is_invulnerable()
    }

    /// Vanilla `phaseManager.setPhase`.
    pub fn set_phase(&self, phase: EnderDragonPhase) {
        self.phases.set_phase(self, phase);
    }

    /// Vanilla `getPhaseManager`.
    #[must_use]
    pub(crate) const fn phases(&self) -> &EnderDragonPhaseManager {
        &self.phases
    }

    /// Writes vanilla `DATA_PHASE`.
    pub(crate) fn set_synced_phase(&self, phase: EnderDragonPhase) {
        self.entity_data.lock().phase.set(phase.id());
    }

    /// Returns this dragon's shared handle while it is being ticked.
    ///
    /// The handle comes from the parts, which are bound to the dragon at the
    /// start of each tick; phase code needs it to own spawned projectiles and
    /// damage sources.
    #[must_use]
    pub(crate) fn shared_handle(&self) -> Option<SharedEntity> {
        self.parts.head.parent()
    }

    /// Vanilla `DATA_PHASE`.
    #[must_use]
    pub fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::from_id(*self.entity_data.lock().phase.get())
    }

    /// Vanilla `dragonDeathTime >= 200`.
    #[must_use]
    pub fn is_dragon_dead(&self) -> bool {
        self.state.lock().dragon_death_time >= DEATH_ANIMATION_TICKS
    }

    /// Vanilla `EnderDragonFight.updateDragon` entry point.
    ///
    /// Feeds the live health and custom name into the fight's boss bar.
    pub fn update_dragon_fight_bar(&self) {
        if !self.dragon_fight_present() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let name = self.custom_name();
        dragon_fight::update_dragon_bar(
            &world,
            self.get_health(),
            self.get_max_health(),
            name.as_ref(),
        );
    }

    /// Vanilla `EnderDragonFight.hasPreviouslyKilledDragon`.
    fn dragon_fight_previously_killed(&self) -> bool {
        self.level()
            .and_then(|world| {
                world
                    .dragon_fight()
                    .map(|fight| fight.lock().previously_killed())
            })
            .unwrap_or(false)
    }

    /// Vanilla `onCrystalDestroyed`.
    pub fn on_crystal_destroyed(
        &self,
        world: &Arc<World>,
        crystal: &SharedEntity,
        source: &DamageSource,
    ) {
        // Vanilla `CRYSTAL_DESTROY_TARGETING`: combat targeting without a
        // targeter, so only visible, non-invulnerable players qualify.
        let player: Option<SharedEntity> = source
            .causing_entity()
            .filter(|entity| entity.as_player().is_some())
            .map(Arc::clone)
            .or_else(|| {
                let pos = crystal.position();
                world
                    .nearest_player(pos, CRYSTAL_DESTROY_TARGETING_RANGE, |player| {
                        !player.is_spectator() && player.can_be_seen_as_enemy()
                    })
                    .map(|player| Arc::clone(&player) as SharedEntity)
            });

        if self.nearest_crystal_is(crystal) {
            let mut explosion =
                DamageSource::direct(&vanilla_damage_types::EXPLOSION, Arc::clone(crystal));
            if let Some(player) = player.clone() {
                explosion = explosion.with_causing_entity(player);
            }
            self.hurt_dragon(
                EnderDragonPartKind::Head,
                world.as_ref(),
                &explosion,
                CRYSTAL_EXPLOSION_DAMAGE,
            );
        }

        self.phases.on_crystal_destroyed(
            self,
            crystal,
            crystal.block_position(),
            source,
            player.as_deref().and_then(|entity| entity.as_player()),
        );
    }

    /// Vanilla `EnderDragon.hurt(ServerLevel, EnderDragonPart, DamageSource, float)`.
    ///
    /// `part` selects the vanilla head/neck damage scaling. Returns whether the
    /// dragon accepted the hit.
    pub(crate) fn hurt_dragon(
        &self,
        part: EnderDragonPartKind,
        world: &World,
        source: &DamageSource,
        damage: f32,
    ) -> bool {
        if self.is_invulnerable_to_base(source) {
            return false;
        }
        if self.phase() == EnderDragonPhase::Dying {
            return false;
        }

        let mut damage = self.phases.on_hurt(self, source, damage);
        if !matches!(part, EnderDragonPartKind::Head | EnderDragonPartKind::Neck) {
            damage = damage / 4.0 + damage.min(1.0);
        }

        if damage < 0.01 {
            return false;
        }

        let player_damage = source
            .causing_entity()
            .is_some_and(|entity| entity.as_player().is_some())
            || source.is(&vanilla_damage_type_tags::DamageTypeTag::ALWAYS_HURTS_ENDER_DRAGONS);
        if player_damage {
            let health_before = self.get_health();
            LivingEntity::hurt_server(self, world, source, damage);
            if self.is_phase_sitting() {
                let received = {
                    let mut state = self.state.lock();
                    state.sitting_damage_received += health_before - self.get_health();
                    state.sitting_damage_received
                };
                if received > SITTING_ALLOWED_DAMAGE_PERCENTAGE * self.get_max_health() {
                    self.state.lock().sitting_damage_received = 0.0;
                    self.set_phase(EnderDragonPhase::Takeoff);
                }
            }
        }

        true
    }

    /// Returns whether the current phase reports `isSitting`.
    pub(crate) fn is_phase_sitting(&self) -> bool {
        self.phase().is_sitting()
    }

    /// Vanilla `getHeadYOffset`.
    fn head_y_offset(&self) -> f32 {
        if self.is_phase_sitting() {
            return SITTING_HEAD_Y_OFFSET;
        }
        let history = &self.state.lock().flight_history;
        (history.get(5).y - history.get(0).y) as f32
    }

    /// Vanilla `getHeadLookVector(1.0F)`.
    pub(crate) fn head_look_vector(&self) -> DVec3 {
        let (yaw, _) = self.rotation();
        match self.phase() {
            EnderDragonPhase::Landing | EnderDragonPhase::Takeoff => {
                let Some(world) = self.level() else {
                    return self.look_angle();
                };
                let egg = self.podium_heightmap_pos(&world, HeightmapType::MotionBlockingNoLeaves);
                let dist = (block_center_distance_sqr(egg, self.position()).sqrt() / 4.0).max(1.0);
                let y_offset = 6.0 / dist;
                self.calculate_view_vector((-y_offset * 1.5 * 5.0) as f32, yaw)
            }
            _ if self.is_phase_sitting() => self.calculate_view_vector(-45.0, yaw),
            _ => self.look_angle(),
        }
    }

    /// Vanilla `EnderDragonFight.getPodiumLocation` resolved through a heightmap.
    ///
    /// Vanilla's `END_PODIUM_LOCATION` is the zero block, so the podium is the
    /// fight origin itself.
    pub(crate) fn podium_heightmap_pos(&self, world: &World, heightmap: HeightmapType) -> BlockPos {
        let origin = self.fight_origin();
        BlockPos::new(
            origin.x(),
            LevelReader::height_at(world, heightmap, origin.x(), origin.z()),
            origin.z(),
        )
    }

    /// Vanilla `EnderDragon.getYRot` assignment helper.
    pub(crate) fn set_y_rot(&self, yaw: f32) {
        self.set_rotation((yaw, self.rotation().1));
    }

    /// Vanilla `yRotA`.
    pub(crate) fn y_rot_a(&self) -> f32 {
        self.state.lock().y_rot_a
    }

    /// Sets vanilla `yRotA`.
    pub(crate) fn set_y_rot_a(&self, y_rot_a: f32) {
        self.state.lock().y_rot_a = y_rot_a;
    }

    /// Returns the head part position.
    pub(crate) fn head_position(&self) -> DVec3 {
        self.parts.head.position()
    }

    /// Returns the head part bounding box.
    pub(crate) fn head_box(&self) -> WorldAabb {
        self.parts.head.bounding_box()
    }

    /// Returns the neck part bounding box.
    pub(crate) fn neck_box(&self) -> WorldAabb {
        self.parts.neck.bounding_box()
    }

    /// Returns the body part bounding box.
    pub(crate) fn body_box(&self) -> WorldAabb {
        self.parts.body.bounding_box()
    }

    /// Returns the vanilla `head` part.
    pub(crate) const fn head_part(&self) -> &EnderDragonPartEntity {
        &self.parts.head
    }

    /// Vanilla `getRandom().nextInt(bound)`.
    pub(crate) fn random_i32_bounded(&self, bound: i32) -> i32 {
        self.state.lock().random.next_i32_bounded(bound)
    }

    /// Vanilla `getRandom().nextInt()`.
    pub(crate) fn random_i32(&self) -> i32 {
        self.state.lock().random.next_i32()
    }

    /// Vanilla `getRandom().nextFloat()`.
    pub(crate) fn random_f32(&self) -> f32 {
        self.state.lock().random.next_f32()
    }

    /// Vanilla `EnderDragonFight.aliveCrystals()` with the `dragonFight == null` fallback.
    pub(crate) fn alive_crystals_or_zero(&self) -> i32 {
        if self.dragon_fight_present() {
            self.alive_crystals()
        } else {
            0
        }
    }

    /// Returns the cached 24-node flight graph, building it on first use.
    fn dragon_nodes(&self) -> Option<[DragonNode; NODE_COUNT]> {
        if let Some(nodes) = self.state.lock().nodes {
            return Some(nodes);
        }

        let world = self.level()?;
        let mut nodes = [DragonNode::new(0, 0, 0); NODE_COUNT];
        for (index, node) in nodes.iter_mut().enumerate() {
            let mut y_adjustment = 5;
            let mut multiplier = index as i32;
            let node_x;
            let node_z;
            if index < 12 {
                let angle =
                    2.0_f32 * (-f32::consts::PI + (f32::consts::PI / 12.0) * multiplier as f32);
                node_x = (60.0_f32 * angle.cos()).floor() as i32;
                node_z = (60.0_f32 * angle.sin()).floor() as i32;
            } else if index < 20 {
                multiplier -= 12;
                let angle =
                    2.0_f32 * (-f32::consts::PI + (f32::consts::PI / 8.0) * multiplier as f32);
                node_x = (40.0_f32 * angle.cos()).floor() as i32;
                node_z = (40.0_f32 * angle.sin()).floor() as i32;
                y_adjustment += 10;
            } else {
                multiplier -= 20;
                let angle =
                    2.0_f32 * (-f32::consts::PI + (f32::consts::PI / 4.0) * multiplier as f32);
                node_x = (20.0_f32 * angle.cos()).floor() as i32;
                node_z = (20.0_f32 * angle.sin()).floor() as i32;
            }

            let height = world.height_at(HeightmapType::MotionBlockingNoLeaves, node_x, node_z);
            // Vanilla: `Math.max(73, heightmapY + yAdjustment)`.
            *node = DragonNode::new(node_x, height.saturating_add(y_adjustment).max(73), node_z);
        }

        self.state.lock().nodes = Some(nodes);
        Some(nodes)
    }

    /// Vanilla `findClosestNode()`.
    pub(crate) fn find_closest_node(&self) -> usize {
        let position = self.position();
        self.find_closest_node_at(position.x, position.y, position.z)
    }

    /// Vanilla `findClosestNode(double, double, double)`.
    pub(crate) fn find_closest_node_at(&self, t_x: f64, t_y: f64, t_z: f64) -> usize {
        let Some(nodes) = self.dragon_nodes() else {
            return 0;
        };

        let current = DragonNode::new(fast_floor(t_x), fast_floor(t_y), fast_floor(t_z));
        let start_index = if self.alive_crystals_or_zero() == 0 {
            12
        } else {
            0
        };

        let mut closest_dist = 10000.0_f32;
        let mut closest_index = 0;
        for (index, node) in nodes.iter().enumerate().skip(start_index) {
            let dist = node.distance_to_sqr(&current);
            if dist < closest_dist {
                closest_dist = dist;
                closest_index = index;
            }
        }
        closest_index
    }

    /// Vanilla `findPath(int, int, Node)`.
    pub(crate) fn find_path(
        &self,
        start_index: usize,
        end_index: usize,
        final_node: Option<DragonNode>,
    ) -> Option<DragonPath> {
        let nodes = self.dragon_nodes()?;
        let mut search: Vec<DragonSearchNode> =
            nodes.iter().copied().map(DragonSearchNode::new).collect();
        if let Some(final_node) = final_node {
            search.push(DragonSearchNode::new(final_node));
        }

        let from = start_index.min(NODE_COUNT - 1);
        let to = end_index.min(NODE_COUNT - 1);
        let to_node = search[to].node;

        search[from].g = 0.0;
        search[from].h = search[from].node.distance_to(&to_node);
        search[from].f = search[from].h;
        search[from].in_open_set = true;
        let mut open_set = vec![from];

        let mut closest = from;
        let minimum_node_index = if self.alive_crystals_or_zero() == 0 {
            12
        } else {
            0
        };

        while !open_set.is_empty() {
            let mut best = 0;
            for index in 1..open_set.len() {
                if search[open_set[index]].f < search[open_set[best]].f {
                    best = index;
                }
            }
            let current = open_set.swap_remove(best);
            search[current].in_open_set = false;

            if current == to {
                let target = if final_node.is_some() {
                    let final_index = search.len() - 1;
                    search[final_index].came_from = Some(to);
                    final_index
                } else {
                    to
                };
                return Some(Self::reconstruct_path(&search, target));
            }

            if search[current].node.distance_to(&to_node)
                < search[closest].node.distance_to(&to_node)
            {
                closest = current;
            }

            search[current].closed = true;
            let adjacency = NODE_ADJACENCY[current];
            for index in minimum_node_index..NODE_COUNT {
                if adjacency & (1 << index) == 0 || search[index].closed {
                    continue;
                }

                let tentative_g =
                    search[current].g + search[current].node.distance_to(&search[index].node);
                if search[index].in_open_set && tentative_g >= search[index].g {
                    continue;
                }

                search[index].came_from = Some(current);
                search[index].g = tentative_g;
                search[index].h = search[index].node.distance_to(&to_node);
                search[index].f = search[index].g + search[index].h;
                if !search[index].in_open_set {
                    search[index].in_open_set = true;
                    open_set.push(index);
                }
            }
        }

        if closest == from {
            return None;
        }

        let target = if final_node.is_some() {
            let final_index = search.len() - 1;
            search[final_index].came_from = Some(closest);
            final_index
        } else {
            closest
        };
        Some(Self::reconstruct_path(&search, target))
    }

    /// Vanilla `reconstructPath`.
    ///
    /// The `from` node starts the search with no `cameFrom`, so walking the
    /// chain back from `to` stops there and includes it.
    fn reconstruct_path(search: &[DragonSearchNode], to: usize) -> DragonPath {
        let mut nodes = vec![search[to].node];
        let mut current = to;
        while let Some(previous) = search[current].came_from {
            current = previous;
            nodes.insert(0, search[current].node);
        }
        DragonPath::new(nodes)
    }

    /// Vanilla `EnderDragon.checkCrystals`.
    fn check_crystals(&self) {
        let Some(world) = self.level() else {
            return;
        };

        let nearest_uuid = {
            let mut state = self.state.lock();
            match state.nearest_crystal {
                Some(uuid) => match world.get_entity_by_uuid(&uuid) {
                    Some(crystal) if !crystal.is_removed() => {
                        if self.tick_count() % CRYSTAL_HEAL_INTERVAL == 0
                            && self.get_health() < self.get_max_health()
                        {
                            self.set_health(self.get_health() + 1.0);
                        }
                        Some(uuid)
                    }
                    _ => {
                        state.nearest_crystal = None;
                        None
                    }
                },
                None => None,
            }
        };

        if nearest_uuid.is_some() || self.random_i32_bounded(10) != 0 {
            return;
        }

        let crystals = world.get_entities_in_aabb(&self.bounding_box().inflate(CRYSTAL_SCAN_RANGE));
        let mut nearest: Option<(uuid::Uuid, f64)> = None;
        for entity in crystals {
            if entity.as_ref().downcast_ref::<EndCrystalEntity>().is_none() {
                continue;
            }
            let dist = entity.position().distance_squared(self.position());
            if nearest.is_none_or(|(_, current)| dist < current) {
                nearest = Some((entity.uuid(), dist));
            }
        }
        self.state.lock().nearest_crystal = nearest.map(|(uuid, _)| uuid);
    }

    /// Returns whether `crystal` is the dragon's tracked nearest crystal.
    fn nearest_crystal_is(&self, crystal: &SharedEntity) -> bool {
        let uuid = self.state.lock().nearest_crystal;
        uuid.is_some_and(|uuid| uuid == crystal.uuid())
    }

    /// Vanilla `EnderDragon.tickPart`.
    fn tick_part(&self, part: &EnderDragonPartEntity, x: f64, y: f64, z: f64) {
        let position = self.position();
        let target = DVec3::new(position.x + x, position.y + y, position.z + z);
        if let Err(error) = part.try_set_position(target) {
            log::debug!("failed to move ender dragon part: {error}");
        }
    }

    /// Vanilla `EnderDragon.checkWalls`.
    fn check_walls(&self, world: &Arc<World>, bounds: WorldAabb) -> bool {
        let x0 = fast_floor(bounds.min_x());
        let y0 = fast_floor(bounds.min_y());
        let z0 = fast_floor(bounds.min_z());
        let x1 = fast_floor(bounds.max_x());
        let y1 = fast_floor(bounds.max_y());
        let z1 = fast_floor(bounds.max_z());
        let mut hit_wall = false;
        let mut destroyed_block = false;
        let mob_griefing = world.get_game_rule(&MOB_GRIEFING);

        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    let block_pos = BlockPos::new(x, y, z);
                    let state: BlockStateId = world.get_block_state(block_pos);
                    if state.is_air() || state.get_block().has_tag(&BlockTag::DRAGON_TRANSPARENT) {
                        continue;
                    }
                    if mob_griefing && !state.get_block().has_tag(&BlockTag::DRAGON_IMMUNE) {
                        destroyed_block = world.remove_block(block_pos, false) || destroyed_block;
                    } else {
                        hit_wall = true;
                    }
                }
            }
        }

        if destroyed_block {
            let random_pos = BlockPos::new(
                x0 + self.random_i32_bounded(x1 - x0 + 1),
                y0 + self.random_i32_bounded(y1 - y0 + 1),
                z0 + self.random_i32_bounded(z1 - z0 + 1),
            );
            world.level_event(
                level_events::PARTICLES_DRAGON_BLOCK_BREAK,
                random_pos,
                0,
                None,
            );
        }

        hit_wall
    }

    /// Vanilla `EnderDragon.knockBack`.
    fn knock_back(&self, world: &Arc<World>, self_entity: &SharedEntity, targets: &[SharedEntity]) {
        let body = self.body_box();
        let xm = (body.min_x() + body.max_x()) / 2.0;
        let zm = (body.min_z() + body.max_z()) / 2.0;

        for entity in targets {
            let Some(living) = entity.as_living_entity() else {
                continue;
            };
            let xd = entity.position().x - xm;
            let zd = entity.position().z - zm;
            let dd = (xd * xd + zd * zd).max(0.1);
            entity.push_impulse(DVec3::new(xd / dd * 4.0, 0.2, zd / dd * 4.0));
            if !self.is_phase_sitting()
                && living.last_hurt_by_mob_timestamp() < entity.tick_count() - 2
            {
                // Vanilla also gates on `doTeamsAllowDamage`; Steel has no
                // scoreboard teams on the entity damage path.
                let source = DamageSource::direct(
                    &vanilla_damage_types::MOB_ATTACK,
                    Arc::clone(self_entity),
                );
                entity.hurt(world, &source, WING_CONTACT_DAMAGE);
            }
        }
    }

    /// Vanilla `EnderDragon.hurt(ServerLevel, List<Entity>)` head/neck contact damage.
    fn hurt_contact(
        &self,
        world: &Arc<World>,
        self_entity: &SharedEntity,
        targets: &[SharedEntity],
    ) {
        for target in targets {
            if target.as_living_entity().is_none() {
                continue;
            }
            let source =
                DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(self_entity));
            target.hurt(world, &source, HEAD_CONTACT_DAMAGE);
        }
    }

    /// Returns the entities inside `box` that vanilla's
    /// `EntitySelector.NO_CREATIVE_OR_SPECTATOR` accepts.
    fn contact_targets(
        &self,
        world: &Arc<World>,
        self_entity: &SharedEntity,
        bounds: WorldAabb,
    ) -> Vec<SharedEntity> {
        world
            .get_entities_in_aabb(&bounds)
            .into_iter()
            .filter(|entity| {
                !Arc::ptr_eq(entity, self_entity)
                    && !entity.is_spectator()
                    && entity
                        .as_player()
                        .is_none_or(|player| player.game_mode() != GameType::Creative)
            })
            .collect()
    }

    /// Vanilla `EnderDragon.aiStep` movement and phase tick.
    fn dragon_ai_step(&self, entity: &SharedEntity) {
        self.process_flapping_movement();

        let o_flap_time = self.state.lock().flap_time;
        self.state.lock().o_flap_time = o_flap_time;

        if self.is_dead_or_dying() {
            let position = self.position();
            let xo = (self.random_f32() - 0.5) * 8.0;
            let yo = (self.random_f32() - 0.5) * 4.0;
            let zo = (self.random_f32() - 0.5) * 8.0;
            self.spawn_particle(
                ParticleData::simple(&vanilla_particle_types::EXPLOSION),
                DVec3::new(
                    position.x + f64::from(xo),
                    position.y + 2.0 + f64::from(yo),
                    position.z + f64::from(zo),
                ),
            );
            return;
        }

        self.check_crystals();

        let movement = self.velocity();
        let mut flap_speed =
            0.2 / ((movement.x * movement.x + movement.z * movement.z).sqrt() as f32 * 10.0 + 1.0);
        flap_speed *= 2.0_f32.powf(movement.y as f32);
        let flap_delta = if self.is_phase_sitting() {
            0.1
        } else if self.state.lock().in_wall {
            flap_speed * 0.5
        } else {
            flap_speed
        };
        self.state.lock().flap_time += flap_delta;

        let yaw = wrap_degrees(self.rotation().0);
        self.set_y_rot(yaw);

        if self.is_no_ai() {
            self.state.lock().flap_time = 0.5;
            return;
        }

        self.state
            .lock()
            .flight_history
            .record(self.position().y, yaw);

        let Some(world) = self.level() else {
            return;
        };

        let phase_before = self.phase();
        self.phases.run_server_tick(self, &world);
        if self.phase() != phase_before {
            self.phases.run_server_tick(self, &world);
        }

        if let Some(target) = self.phases.fly_target() {
            self.fly_towards(entity, target);
        }

        self.apply_effects_from_blocks();

        self.set_y_body_rot(yaw);
        self.update_parts();
        self.update_contact_damage(&world, entity);

        let in_wall = self.check_walls(&world, self.head_box())
            | self.check_walls(&world, self.neck_box())
            | self.check_walls(&world, self.body_box());
        self.state.lock().in_wall = in_wall;
        self.update_dragon_fight_bar();
    }

    /// Vanilla `EnderDragon.aiStep` flight steering towards `target`.
    fn fly_towards(&self, entity: &SharedEntity, target: DVec3) {
        let position = self.position();
        let xdd = target.x - position.x;
        let ydd = target.y - position.y;
        let zdd = target.z - position.z;
        let dist_to_target = xdd * xdd + ydd * ydd + zdd * zdd;
        let max = self.phases.fly_speed();
        let horizontal_dist = (xdd * xdd + zdd * zdd).sqrt();
        let mut ydd = ydd;
        if horizontal_dist > 0.0 {
            ydd = (ydd / horizontal_dist).clamp(-f64::from(max), f64::from(max));
        }

        self.set_velocity(self.velocity() + DVec3::new(0.0, ydd * 0.01, 0.0));
        let yaw = wrap_degrees(self.rotation().0);
        self.set_y_rot(yaw);

        let aim = normalize_vec(DVec3::new(
            target.x - position.x,
            target.y - position.y,
            target.z - position.z,
        ));
        let yaw_rad = f64::from(yaw) * f64::consts::PI / 180.0;
        let dir = normalize_vec(DVec3::new(yaw_rad.sin(), self.velocity().y, -yaw_rad.cos()));
        let dot = ((dir.dot(aim) as f32 + 0.5) / 1.5).max(0.0);

        if xdd.abs() > 1.0e-5 || zdd.abs() > 1.0e-5 {
            let y_rot_d =
                wrap_degrees(180.0 - (xdd.atan2(zdd) as f32) * 180.0 / f32::consts::PI - yaw)
                    .clamp(-50.0, 50.0);
            let mut y_rot_a = self.y_rot_a() * 0.8;
            y_rot_a += y_rot_d * self.phases.turn_speed(self);
            self.set_y_rot_a(y_rot_a);
            self.set_y_rot(self.rotation().0 + y_rot_a * 0.1);
        }

        let span = (2.0 / (dist_to_target + 1.0)) as f32;
        let speed = 0.06;
        self.move_relative(
            speed * (dot * span + (1.0 - span)),
            DVec3::new(0.0, 0.0, -1.0),
        );
        let delta = if self.state.lock().in_wall {
            self.velocity() * 0.8
        } else {
            self.velocity()
        };
        let _ = Arc::clone(entity).move_entity(MoverType::SelfMovement, delta);

        let actual = normalize_vec(self.velocity());
        let slide = 0.8 + 0.15 * (actual.dot(dir) + 1.0) / 2.0;
        let velocity = self.velocity();
        self.set_velocity(DVec3::new(
            velocity.x * slide,
            velocity.y * 0.91,
            velocity.z * slide,
        ));
    }

    /// Vanilla `EnderDragon.aiStep` sub-entity positioning.
    fn update_parts(&self) {
        let old_positions: Vec<DVec3> = self
            .parts
            .iter()
            .into_iter()
            .map(Entity::position)
            .collect();

        let (yaw, _) = self.rotation();
        let history_tilt = {
            let state = self.state.lock();
            (state.flight_history.get(5).y - state.flight_history.get(10).y) as f32
                * 10.0
                * f32::consts::PI
                / 180.0
        };
        let cc_tilt = history_tilt.cos();
        let ss_tilt = history_tilt.sin();
        let rot1 = yaw * f32::consts::PI / 180.0;
        let ss1 = rot1.sin();
        let cc1 = rot1.cos();
        self.tick_part(
            &self.parts.body,
            f64::from(ss1 * 0.5),
            0.0,
            f64::from(-cc1 * 0.5),
        );
        self.tick_part(
            &self.parts.wing1,
            f64::from(cc1 * 4.5),
            2.0,
            f64::from(ss1 * 4.5),
        );
        self.tick_part(
            &self.parts.wing2,
            f64::from(cc1 * -4.5),
            2.0,
            f64::from(ss1 * -4.5),
        );

        let y_rot_a = self.y_rot_a();
        let ss2 = (rot1 - y_rot_a * 0.01).sin();
        let cc2 = (rot1 - y_rot_a * 0.01).cos();
        let y_offset = self.head_y_offset();
        self.tick_part(
            &self.parts.head,
            f64::from(ss2 * 6.5 * cc_tilt),
            f64::from(y_offset + ss_tilt * 6.5),
            f64::from(-cc2 * 6.5 * cc_tilt),
        );
        self.tick_part(
            &self.parts.neck,
            f64::from(ss2 * 5.5 * cc_tilt),
            f64::from(y_offset + ss_tilt * 5.5),
            f64::from(-cc2 * 5.5 * cc_tilt),
        );

        let tails = [&self.parts.tail1, &self.parts.tail2, &self.parts.tail3];
        let (p1, samples) = {
            let state = self.state.lock();
            (
                state.flight_history.get(5),
                [
                    state.flight_history.get(12),
                    state.flight_history.get(14),
                    state.flight_history.get(16),
                ],
            )
        };
        for (index, part) in tails.into_iter().enumerate() {
            let p0 = samples[index];
            let rot = rot1 + wrap_degrees(p0.y_rot - p1.y_rot) * f32::consts::PI / 180.0;
            let ss = rot.sin();
            let cc = rot.cos();
            let dd = (index + 1) as f32 * 2.0;
            self.tick_part(
                part,
                f64::from(-(ss1 * 1.5 + ss * dd) * cc_tilt),
                p0.y - p1.y - f64::from((dd + 1.5) * ss_tilt) + 1.5,
                f64::from((cc1 * 1.5 + cc * dd) * cc_tilt),
            );
        }

        for (part, old_position) in self.parts.iter().into_iter().zip(old_positions) {
            part.set_old_position(old_position);
        }
    }

    /// Vanilla `EnderDragon.aiStep` wing knockback and head/neck contact damage.
    fn update_contact_damage(&self, world: &Arc<World>, entity: &SharedEntity) {
        if self.living_base.invulnerable_time() > 10 {
            return;
        }

        for wing in [
            self.parts.wing1.bounding_box(),
            self.parts.wing2.bounding_box(),
        ] {
            let box_ = wing
                .inflate_xyz(WING_PUSH_INFLATION, 2.0, WING_PUSH_INFLATION)
                .translate(DVec3::new(0.0, -2.0, 0.0));
            let targets = self.contact_targets(world, entity, box_);
            self.knock_back(world, entity, &targets);
        }

        let head_targets = self.contact_targets(world, entity, self.head_box().inflate(1.0));
        self.hurt_contact(world, entity, &head_targets);
        let neck_targets = self.contact_targets(world, entity, self.neck_box().inflate(1.0));
        self.hurt_contact(world, entity, &neck_targets);
    }

    /// Vanilla `Level.addParticle` for a single particle on the server.
    fn spawn_particle(&self, particle: ParticleData, position: DVec3) {
        if let Some(world) = self.level() {
            world.send_particles(particle, position, 1, DVec3::ZERO, 0.0);
        }
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
}

/// Vanilla `Vec3.normalize`: squared lengths below `1.0E-4` collapse to zero.
pub(crate) fn normalize_vec(vector: DVec3) -> DVec3 {
    if vector.length_squared() < VANILLA_NORMALIZE_EPSILON_SQR {
        DVec3::ZERO
    } else {
        vector.normalize()
    }
}

/// Vanilla `BlockPos.distToCenterSqr`.
fn block_center_distance_sqr(pos: BlockPos, point: DVec3) -> f64 {
    let center = DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    );
    center.distance_squared(point)
}

impl Entity for EnderDragonEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        // Vanilla dragon parts hold their parent; bind it now that the dragon has
        // a shared handle so part damage can forward to `hurt_dragon`.
        let shared: SharedEntity = Arc::clone(&self) as SharedEntity;
        self.parts.bind_parent(&shared);
        let entity = self.into_shared_entity();
        if let Some(living) = entity.as_living_entity() {
            living.tick_living_entity(&entity);
        }
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(0.6) as f32
    }

    fn sound_source(&self) -> SoundSource {
        Monster::sound_source_monster(self)
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn is_pickable(&self) -> bool {
        false
    }

    fn can_use_portal(&self, _ignore_passenger: bool) -> bool {
        false
    }

    /// Vanilla `EnderDragon.hurtServer` routes every non-part hit through the body
    /// part, which applies the phase hook and the quarter damage scaling.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.hurt_dragon(EnderDragonPartKind::Body, world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert(DRAGON_PHASE_KEY, self.phase().id());
        let state = self.state.lock();
        nbt.insert(DRAGON_DEATH_TIME_KEY, state.dragon_death_time);
        nbt.insert(SITTING_DAMAGE_RECEIVED_KEY, state.sitting_damage_received);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(phase) = nbt.int(DRAGON_PHASE_KEY) {
            self.set_phase(EnderDragonPhase::from_id(phase));
        }
        let mut state = self.state.lock();
        state.dragon_death_time = nbt.int(DRAGON_DEATH_TIME_KEY).unwrap_or(DEFAULT_DEATH_TIME);
        state.sitting_damage_received = nbt.float(SITTING_DAMAGE_RECEIVED_KEY).unwrap_or(0.0);
    }
}

impl LivingEntity for EnderDragonEntity {
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

    fn sound_volume(&self) -> f32 {
        DRAGON_SOUND_VOLUME
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ENDER_DRAGON_HURT)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    /// Vanilla `EnderDragon.addEffect` always refuses effects.
    fn add_mob_effect(&self, _effect: MobEffectInstance) -> bool {
        false
    }

    /// Vanilla `EnderDragon.canAttack` ignores the peaceful-difficulty check.
    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        target.can_be_seen_as_enemy()
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.dragon_ai_step(entity);
        None
    }

    /// Vanilla `EnderDragon.handleKillingBlow` plus the shared death bookkeeping.
    fn die(&self, source: &DamageSource) {
        if self.is_removed() || !self.living_base.mark_death_processed() {
            return;
        }

        if !self.is_phase_sitting() {
            self.set_health(1.0);
            self.set_phase(EnderDragonPhase::Dying);
        }

        if let Some(world) = self.level() {
            let Some(self_living) = self.as_living_entity() else {
                return;
            };
            let killed = source
                .causing_entity()
                .is_none_or(|entity| entity.killed_entity(&world, self_living, source));
            if killed {
                self.game_event(&vanilla_game_events::ENTITY_DIE);
                self.drop_all_death_loot(source);
            }
        }

        self.broadcast_entity_event(EntityStatus::Death);
        self.set_pose(EntityPose::Dying);
    }

    /// Vanilla `EnderDragon.tickDeath`.
    fn tick_death(&self) {
        if self.dragon_fight_present() {
            self.update_dragon_fight_bar();
        }

        let death_time = {
            let mut state = self.state.lock();
            state.dragon_death_time += 1;
            state.dragon_death_time
        };

        if (180..=DEATH_ANIMATION_TICKS).contains(&death_time) {
            let position = self.position();
            let xo = (self.random_f32() - 0.5) * 8.0;
            let yo = (self.random_f32() - 0.5) * 4.0;
            let zo = (self.random_f32() - 0.5) * 8.0;
            self.spawn_particle(
                ParticleData::simple(&vanilla_particle_types::EXPLOSION_EMITTER),
                DVec3::new(
                    position.x + f64::from(xo),
                    position.y + 2.0 + f64::from(yo),
                    position.z + f64::from(zo),
                ),
            );
        }

        let experience = if self.dragon_fight_present() && !self.dragon_fight_previously_killed() {
            FIRST_KILL_EXPERIENCE
        } else {
            REPEAT_KILL_EXPERIENCE
        };

        if let Some(world) = self.level() {
            if death_time > 150 && death_time % 5 == 0 && world.get_game_rule(&MOB_DROPS) {
                ExperienceOrbEntity::award(
                    &world,
                    self.position(),
                    fast_floor(f64::from(experience) * 0.08),
                );
            }
            if death_time == 1 && !self.is_silent() {
                world.global_level_event(
                    level_events::SOUND_DRAGON_DEATH,
                    self.block_position(),
                    0,
                );
            }
        }

        // The dragon is `noPhysics`, so vanilla's `move(SELF, ...)` is a position add.
        let target = self.position() + DEATH_MOVE;
        if let Err(error) = self.try_set_position(target) {
            log::debug!("failed to advance dying ender dragon: {error}");
        }
        for part in self.parts.iter() {
            part.set_old_position(part.position());
            part.base().set_old_rotation(part.rotation());
            let target = part.position() + DEATH_MOVE;
            if let Err(error) = part.try_set_position(target) {
                log::debug!("failed to advance dying ender dragon part: {error}");
            }
        }

        if death_time >= DEATH_ANIMATION_TICKS
            && let Some(world) = self.level()
        {
            if world.get_game_rule(&MOB_DROPS) {
                ExperienceOrbEntity::award(
                    &world,
                    self.position(),
                    fast_floor(f64::from(experience) * 0.2),
                );
            }
            if self.dragon_fight_present() {
                dragon_fight::notify_dragon_killed(&world, self.uuid());
            }
            self.set_removed(RemovalReason::Killed);
            self.game_event(&vanilla_game_events::ENTITY_DIE);
        }
    }

    /// Vanilla `EnderDragon.knockback` ignores knockback while sitting.
    fn knockback(&self, power: f64, xd: f64, zd: f64) {
        if self.is_phase_sitting() {
            return;
        }

        // Mirrors `LivingEntity::knockback`; Steel has no hook to call the
        // default body from an override.
        let power = power * (1.0 - self.knockback_resistance());
        if power <= 0.0 {
            return;
        }
        let mut xd = xd;
        let mut zd = zd;
        while xd * xd + zd * zd < f64::from(1.0e-5_f32) {
            xd = (rand::random::<f64>() - rand::random::<f64>()) * 0.01;
            zd = (rand::random::<f64>() - rand::random::<f64>()) * 0.01;
        }
        let old_velocity = self.velocity();
        let delta = DVec3::new(xd, 0.0, zd).normalize() * power;
        self.set_velocity(DVec3::new(
            old_velocity.x / 2.0 - delta.x,
            if self.on_ground() {
                0.4_f64.min(old_velocity.y / 2.0 + power)
            } else {
                old_velocity.y
            },
            old_velocity.z / 2.0 - delta.z,
        ));
        self.mark_velocity_sync();
    }
}

impl Mob for EnderDragonEntity {
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
        Some(&sound_events::ENTITY_ENDER_DRAGON_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl Monster for EnderDragonEntity {}

impl PathfinderMob for EnderDragonEntity {}

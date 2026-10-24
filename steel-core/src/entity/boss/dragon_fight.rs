//! Vanilla `EnderDragonFight` and its `DragonRespawnStage` saved state.
//!
//! Steel keeps the vanilla saved-data shape: `needs_state_scanning`,
//! `dragon_killed`, `previously_killed`, `respawn_stage`, `respawn_time`,
//! `dragon_uuid`, `exit_portal_location`, `gateways`, and the respawn-crystal
//! references. The fight ticks from the level tick when the loaded dimension
//! type opts in with `has_ender_dragon_fight`, exactly like `ServerLevel`.
//!
//! Two foundations are reported rather than stubbed:
//! - Vanilla adds `TicketType.DRAGON` (radius 9) while a player listens. Steel's
//!   ticket store is private to `chunk` and has no runtime
//!   `addTicketWithRadius`, so the fight does not hold arena tickets.
//! - Vanilla `Level.explode` has no Steel counterpart yet; the respawn stage's
//!   pillar clear therefore destroys the 21³ volume directly (block effect
//!   only, no explosion damage/particles).

use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::sync::{Arc, LazyLock};

use glam::DVec3;
use serde::{Deserialize, Serialize};
use steel_registry::REGISTRY;
use steel_registry::level_events::{ANIMATION_DRAGON_SUMMON_ROAR, ANIMATION_END_GATEWAY_SPAWN};
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::translations;
use steel_utils::{BlockPos, BlockStateId, Direction, WorldAabb};
use text_components::TextComponent;
use uuid::Uuid;

use steel_protocol::packets::game::{BossBarColor, BossBarOverlay, CBossEvent};

use super::event::ServerBossEvent;
use crate::chunk::heightmap::HeightmapType;
use crate::entity::entities::EndCrystalEntity;
use crate::entity::spawn::create_entity_instance;
use crate::entity::{Entity, RemovalReason, SharedEntity};
use crate::world::explosion::ExplosionInteraction;
use crate::world::{LevelReader, World};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_utils::Downcast as _;
use steel_utils::types::UpdateFlags;

/// Vanilla `EnderDragonFight.MAX_TICKS_BEFORE_DRAGON_RESPAWN`.
pub const MAX_TICKS_BEFORE_DRAGON_RESPAWN: i32 = 1200;
/// Vanilla `EnderDragonFight.TIME_BETWEEN_CRYSTAL_SCANS`.
pub const TIME_BETWEEN_CRYSTAL_SCANS: i32 = 100;
/// Vanilla `EnderDragonFight.TIME_BETWEEN_PLAYER_SCANS`.
pub const TIME_BETWEEN_PLAYER_SCANS: i32 = 20;
/// Vanilla `EnderDragonFight.ARENA_SIZE_CHUNKS`.
pub const ARENA_SIZE_CHUNKS: i32 = 8;
/// Vanilla `EnderDragonFight.GATEWAY_COUNT`.
pub const GATEWAY_COUNT: usize = 20;
/// Vanilla `EnderDragonFight.GATEWAY_DISTANCE`.
pub const GATEWAY_DISTANCE: f64 = 96.0;
/// Vanilla `EnderDragonFight.DRAGON_SPAWN_Y`.
pub const DRAGON_SPAWN_Y: i32 = 128;
/// Vanilla `EnderDragonFight.spawnNewGateway` ring height.
pub const GATEWAY_Y: i32 = 75;
/// Vanilla `EnderDragonFight.validPlayer` listen radius (192 blocks).
pub const DRAGON_LISTEN_RADIUS: f64 = 192.0;
/// Vanilla `EndPodiumFeature.PODIUM_RADIUS`.
pub const PODIUM_RADIUS: i32 = 4;
/// Vanilla `EndPodiumFeature.PODIUM_PILLAR_HEIGHT`.
pub const PODIUM_PILLAR_HEIGHT: i32 = 4;

/// Vanilla `DragonRespawnStage.SUMMONING_PILLARS` interval.
const SUMMONING_PILLARS_INTERVAL: i32 = 40;

fn dragon_display_name() -> TextComponent {
    TextComponent::translated(translations::ENTITY_MINECRAFT_ENDER_DRAGON.msg())
}

/// Vanilla `DragonRespawnStage` (`world.level.dimension.end`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DragonRespawnStage {
    /// Beam every respawn crystal at the pool.
    #[default]
    Start,
    /// Roar anticipation before the pillars rebuild.
    PreparingToSummonPillars,
    /// Rebuild the pillars one beam at a time.
    SummoningPillars,
    /// Beam the crystals together and summon the dragon.
    SummoningDragon,
    /// Terminal state; `set_respawn_stage(END)` clears the stage.
    End,
}

impl DragonRespawnStage {
    /// Vanilla declaration order.
    pub const VALUES: [Self; 5] = [
        Self::Start,
        Self::PreparingToSummonPillars,
        Self::SummoningPillars,
        Self::SummoningDragon,
        Self::End,
    ];

    /// Vanilla `StringRepresentable.getSerializedName`.
    #[must_use]
    pub const fn serialized_name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::PreparingToSummonPillars => "preparing_to_summon_pillars",
            Self::SummoningPillars => "summoning_pillars",
            Self::SummoningDragon => "summoning_dragon",
            Self::End => "end",
        }
    }

    /// Parses a serialized stage name.
    #[must_use]
    pub fn from_serialized_name(name: &str) -> Option<Self> {
        Self::VALUES
            .into_iter()
            .find(|stage| stage.serialized_name() == name)
    }
}

/// Vanilla `EnderDragonFight`: saved state plus the live boss bar.
#[derive(Clone)]
pub struct EnderDragonFight {
    dirty: bool,
    needs_state_scanning: bool,
    dragon_killed: bool,
    previously_killed: bool,
    respawn_stage: Option<DragonRespawnStage>,
    respawn_time: i32,
    dragon_uuid: Option<Uuid>,
    exit_portal_location: Option<BlockPos>,
    gateways: Vec<i32>,
    respawn_crystals: Vec<Uuid>,
    dragon_event: ServerBossEvent,
    ticks_since_last_player_scan: i32,
    ticks_since_dragon_seen: i32,
    ticks_since_crystals_scanned: i32,
    alive_crystals: i32,
    origin: BlockPos,
    initialized: bool,
    players: BTreeSet<Uuid>,
}

impl EnderDragonFight {
    /// Creates a fresh (never-scanned) fight at `origin` (vanilla `BlockPos.ZERO`).
    #[must_use]
    pub fn new(origin: BlockPos) -> Self {
        Self {
            dirty: true,
            needs_state_scanning: true,
            dragon_killed: false,
            previously_killed: false,
            respawn_stage: None,
            respawn_time: 0,
            dragon_uuid: None,
            exit_portal_location: None,
            gateways: Vec::new(),
            respawn_crystals: Vec::new(),
            dragon_event: Self::make_bar(),
            ticks_since_last_player_scan: TIME_BETWEEN_PLAYER_SCANS + 1,
            ticks_since_dragon_seen: 0,
            ticks_since_crystals_scanned: 0,
            alive_crystals: 0,
            origin,
            initialized: false,
            players: BTreeSet::new(),
        }
    }

    fn make_bar() -> ServerBossEvent {
        let mut bar = ServerBossEvent::new(
            Uuid::new_v4(),
            dragon_display_name(),
            BossBarColor::Pink,
            BossBarOverlay::Progress,
        );
        bar.set_play_boss_music(true);
        bar.set_create_world_fog(true);
        bar
    }

    /// Vanilla `EnderDragonFight.init`: shuffle the gateway order from the
    /// level seed and cache the origin.
    pub fn init(&mut self, world_seed: i64, origin: BlockPos) {
        self.origin = origin;
        self.initialized = true;
        if self.gateways.is_empty() {
            let mut order: Vec<i32> = (0..GATEWAY_COUNT as i32).collect();
            let mut random = LegacyRandom::from_seed(world_seed as u64);
            for bound in (2..=order.len()).rev() {
                let swap_to = random.next_i32_bounded(bound as i32) as usize;
                order.swap(bound - 1, swap_to);
            }
            self.gateways = order;
        }
        self.set_dirty();
    }

    /// Whether [`Self::init`] ran for this world.
    #[must_use]
    pub const fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Vanilla `SavedData.setDirty`.
    pub const fn set_dirty(&mut self) {
        self.dirty = true;
        self.dragon_event.set_dirty();
    }

    /// Whether the saved state changed since the last write.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Vanilla `SavedData.save` completion.
    pub const fn mark_saved(&mut self) {
        self.dirty = false;
        self.dragon_event.mark_saved();
    }

    /// The live boss bar (`ServerBossEvent`).
    #[must_use]
    pub const fn dragon_event(&self) -> &ServerBossEvent {
        &self.dragon_event
    }

    /// The live boss bar.
    pub const fn dragon_event_mut(&mut self) -> &mut ServerBossEvent {
        &mut self.dragon_event
    }

    /// Whether the dragon is currently dead.
    #[must_use]
    pub const fn dragon_killed(&self) -> bool {
        self.dragon_killed
    }

    /// Whether the dragon was ever killed on this world.
    #[must_use]
    pub const fn previously_killed(&self) -> bool {
        self.previously_killed
    }

    /// The tracked dragon's uuid.
    #[must_use]
    pub const fn dragon_uuid(&self) -> Option<Uuid> {
        self.dragon_uuid
    }

    /// The cached exit-portal location.
    #[must_use]
    pub const fn exit_portal_location(&self) -> Option<BlockPos> {
        self.exit_portal_location
    }

    /// The active respawn stage, if any.
    #[must_use]
    pub const fn respawn_stage(&self) -> Option<DragonRespawnStage> {
        self.respawn_stage
    }

    /// The number of live pillar crystals from the last scan.
    #[must_use]
    pub const fn alive_crystals(&self) -> i32 {
        self.alive_crystals
    }

    /// Drains queued boss-bar wire packets.
    pub fn drain_boss_bar_packets(&mut self) -> Vec<CBossEvent> {
        self.dragon_event.drain_packets()
    }

    /// Vanilla `EnderDragonFight.updateDragon`: feed the live dragon's
    /// health/name into the boss bar.
    pub fn update_dragon(
        &mut self,
        health: f32,
        max_health: f32,
        custom_name: Option<&TextComponent>,
    ) {
        if max_health > 0.0 {
            self.dragon_event
                .set_progress((health / max_health).clamp(0.0, 1.0));
        }
        if let Some(name) = custom_name {
            self.dragon_event.set_name(name.clone());
        }
        self.ticks_since_dragon_seen = 0;
        self.set_dirty();
    }

    /// Vanilla `EnderDragonFight.tick`.
    pub fn tick(&mut self, world: &Arc<World>) {
        self.dragon_event.set_visible(!self.dragon_killed);

        self.ticks_since_last_player_scan += 1;
        if self.ticks_since_last_player_scan >= TIME_BETWEEN_PLAYER_SCANS {
            self.update_players(world);
            self.ticks_since_last_player_scan = 0;
        }

        if self.players.is_empty() || !self.is_arena_loaded(world) {
            return;
        }

        if self.needs_state_scanning {
            self.scan_state(world);
        }

        if self.respawn_stage.is_some() {
            self.tick_respawn_stage(world);
            return;
        }

        if !self.dragon_killed {
            self.ticks_since_dragon_seen += 1;
            if self.dragon_uuid.is_none()
                || self.ticks_since_dragon_seen >= MAX_TICKS_BEFORE_DRAGON_RESPAWN
            {
                self.find_or_create_dragon(world);
                self.ticks_since_dragon_seen = 0;
            }

            self.ticks_since_crystals_scanned += 1;
            if self.ticks_since_crystals_scanned >= TIME_BETWEEN_CRYSTAL_SCANS {
                self.update_crystal_count(world);
                self.ticks_since_crystals_scanned = 0;
            }
        }
    }

    /// Vanilla `EnderDragonFight.updatePlayers`.
    fn update_players(&mut self, world: &Arc<World>) {
        let origin = self.pool_center();
        let mut inside = BTreeSet::new();
        world.players.iter_players(|uuid, player| {
            if !player.is_alive() {
                return true;
            }
            if origin.distance_squared(player.position())
                <= DRAGON_LISTEN_RADIUS * DRAGON_LISTEN_RADIUS
            {
                inside.insert(*uuid);
                if !self.players.contains(uuid) {
                    self.dragon_event.add_player(Arc::clone(player));
                }
            }
            true
        });

        let removed: Vec<Uuid> = self.players.difference(&inside).copied().collect();
        for uuid in removed {
            self.dragon_event.remove_player(&uuid);
        }
        self.players = inside;
        self.set_dirty();
    }

    /// Vanilla `EnderDragonFight.isArenaLoaded`: the 8-chunk arena around the
    /// origin must be fully loaded before the fight advances.
    fn is_arena_loaded(&self, world: &Arc<World>) -> bool {
        let radius = ARENA_SIZE_CHUNKS * 16;
        world.has_chunks_at(
            self.origin.x() - radius,
            self.origin.z() - radius,
            self.origin.x() + radius,
            self.origin.z() + radius,
        )
    }

    fn pool_center(&self) -> DVec3 {
        DVec3::new(
            f64::from(self.origin.x()),
            f64::from(DRAGON_SPAWN_Y + self.origin.y()),
            f64::from(self.origin.z()),
        )
    }

    const fn pool_block(&self) -> BlockPos {
        BlockPos::new(
            self.origin.x(),
            DRAGON_SPAWN_Y + self.origin.y(),
            self.origin.z(),
        )
    }

    /// Vanilla `EnderDragonFight.scanState`: reconcile a legacy world state.
    fn scan_state(&mut self, world: &Arc<World>) {
        self.needs_state_scanning = false;
        let active_portal_exists = self.has_active_exit_portal(world);
        if active_portal_exists {
            self.previously_killed = true;
            self.dragon_killed = true;
        } else {
            self.previously_killed = false;
            if self.find_exit_portal(world).is_none() {
                self.spawn_exit_portal(world, false);
            }
        }

        let dragons = self.dragons_in_arena(world);
        if dragons.is_empty() {
            if self.dragon_uuid.is_some() {
                self.dragon_killed = true;
            }
        } else if let Some(dragon) = dragons.first() {
            self.dragon_uuid = Some(dragon.uuid());
            self.dragon_killed = false;
            if active_portal_exists {
                Self::bind_dragon(dragon, self.origin);
            } else {
                dragon.set_removed(RemovalReason::Discarded);
                self.dragon_uuid = None;
            }
        }

        if !self.previously_killed && self.dragon_killed {
            self.dragon_killed = false;
        }
        self.set_dirty();
    }

    fn dragons_in_arena(&self, world: &Arc<World>) -> Vec<SharedEntity> {
        let center = self.pool_center();
        let box_ = WorldAabb::of_size(
            center,
            DRAGON_LISTEN_RADIUS * 2.0,
            DRAGON_LISTEN_RADIUS * 2.0,
            DRAGON_LISTEN_RADIUS * 2.0,
        );
        world.get_entities_in_aabb_matching(&box_, |entity| {
            entity.entity_type() == &vanilla_entities::ENDER_DRAGON
        })
    }

    /// Vanilla `EnderDragonFight.hasActiveExitPortal`: any end-portal block
    /// inside the podium footprint.
    fn has_active_exit_portal(&self, world: &Arc<World>) -> bool {
        let Some(ring_y) = self.find_exit_portal_ring_y(world) else {
            return false;
        };
        let end_portal = PODIUM_STATES.end_portal;
        for dx in -PODIUM_RADIUS..=PODIUM_RADIUS {
            for dz in -PODIUM_RADIUS..=PODIUM_RADIUS {
                let pos = BlockPos::new(self.origin.x() + dx, ring_y, self.origin.z() + dz);
                if world.get_block_state(pos) == end_portal {
                    return true;
                }
            }
        }
        false
    }

    /// The podium ring Y: the top of the four-block bedrock pillar at the
    /// origin column, minus three (vanilla finds it through the bedrock
    /// `BlockPattern`; Steel uses the known `EndPodiumFeature` geometry).
    fn find_exit_portal_ring_y(&self, world: &Arc<World>) -> Option<i32> {
        let podium = Self::podium_location(self.origin);
        let x = podium.x();
        let z = podium.z();
        let top = LevelReader::height_at(world, HeightmapType::MotionBlockingNoLeaves, x, z);
        let bedrock = PODIUM_STATES.bedrock;
        for y in (world.get_min_y() + 1..=top).rev() {
            if world.get_block_state(BlockPos::new(x, y, z)) == bedrock {
                return Some((y - 3).max(world.get_min_y() + 1));
            }
        }
        None
    }

    /// Vanilla `EnderDragonFight.findExitPortal`.
    fn find_exit_portal(&mut self, world: &Arc<World>) -> Option<BlockPos> {
        if let Some(location) = self.exit_portal_location {
            return Some(location);
        }
        let ring_y = self.find_exit_portal_ring_y(world)?;
        let location = BlockPos::new(self.origin.x(), ring_y, self.origin.z());
        self.exit_portal_location = Some(location);
        self.set_dirty();
        Some(location)
    }

    /// Vanilla `EnderDragonFight.findOrCreateDragon`.
    fn find_or_create_dragon(&mut self, world: &Arc<World>) {
        let dragons = self.dragons_in_arena(world);
        if let Some(dragon) = dragons.first() {
            self.dragon_uuid = Some(dragon.uuid());
            Self::bind_dragon(dragon, self.origin);
            self.set_dirty();
            return;
        }
        self.create_new_dragon(world);
    }

    /// Vanilla `dragon.setDragonFight(this)` + `dragon.setFightOrigin(origin)`.
    ///
    /// Steel passes the fight's observable state instead of a fight handle, so a
    /// dragon adopted by [`Self::scan_state`] is bound the same way as a freshly
    /// created one.
    fn bind_dragon(dragon: &SharedEntity, origin: BlockPos) {
        use crate::entity::entities::mobs::bosses::EnderDragonEntity;
        if let Some(dragon) = dragon.downcast_ref::<EnderDragonEntity>() {
            dragon.set_fight_origin(origin);
            dragon.set_dragon_fight_present(true);
        }
    }

    /// Vanilla `EnderDragonFight.createNewDragon`.
    fn create_new_dragon(&mut self, world: &Arc<World>) -> Option<SharedEntity> {
        let spawn = self.pool_center();
        let dragon = create_entity_instance(world, &vanilla_entities::ENDER_DRAGON, spawn).ok()?;
        // Vanilla `dragon.setDragonFight(this)` + `dragon.setFightOrigin(origin)`.
        Self::bind_dragon(&dragon, self.origin);
        dragon.snap_to(spawn, rand::random::<f32>() * 360.0, 0.0);
        if world.try_add_entity(Arc::clone(&dragon)).is_ok() {
            self.dragon_uuid = Some(dragon.uuid());
            self.set_dirty();
            Some(dragon)
        } else {
            None
        }
    }

    /// Vanilla `EnderDragonFight.setRespawnStage`. `END` clears the stage,
    /// resets `dragon_killed`, and spawns the new dragon.
    pub fn set_respawn_stage(&mut self, world: &Arc<World>, stage: DragonRespawnStage) {
        self.respawn_time = 0;
        if stage == DragonRespawnStage::End {
            self.respawn_stage = None;
            self.dragon_killed = false;
            self.create_new_dragon(world);
        } else {
            self.respawn_stage = Some(stage);
        }
        self.set_dirty();
    }

    /// Vanilla per-stage ticking driven by `respawnTime`.
    fn tick_respawn_stage(&mut self, world: &Arc<World>) {
        let Some(stage) = self.respawn_stage else {
            return;
        };
        let time = self.respawn_time;
        match stage {
            DragonRespawnStage::Start => {
                let target = self.pool_block();
                for crystal in self.live_respawn_crystals(world) {
                    set_crystal_beam(&crystal, Some(target));
                }
                self.set_respawn_stage(world, DragonRespawnStage::PreparingToSummonPillars);
            }
            DragonRespawnStage::PreparingToSummonPillars => {
                if time == 0 || time == 50 || time == 51 || time == 52 || time >= 95 {
                    self.summon_roar(world);
                }
                if time >= 100 {
                    self.set_respawn_stage(world, DragonRespawnStage::SummoningPillars);
                } else {
                    self.respawn_time += 1;
                }
            }
            DragonRespawnStage::SummoningPillars => {
                let start_of_beam = time % SUMMONING_PILLARS_INTERVAL == 0;
                let end_of_beam =
                    time % SUMMONING_PILLARS_INTERVAL == SUMMONING_PILLARS_INTERVAL - 1;
                if !start_of_beam && !end_of_beam {
                    self.respawn_time += 1;
                } else {
                    let spikes = end_spikes_for_seed(world.seed());
                    let index = (time / SUMMONING_PILLARS_INTERVAL) as usize;
                    let Some(spike) = spikes.get(index).copied() else {
                        self.set_respawn_stage(world, DragonRespawnStage::SummoningDragon);
                        return;
                    };
                    if start_of_beam {
                        let target =
                            BlockPos::new(spike.center_x, spike.height + 1, spike.center_z);
                        for crystal in self.live_respawn_crystals(world) {
                            set_crystal_beam(&crystal, Some(target));
                        }
                    } else {
                        rebuild_spike(world, spike);
                    }
                    self.respawn_time += 1;
                }
            }
            DragonRespawnStage::SummoningDragon => {
                if time >= 100 {
                    self.set_respawn_stage(world, DragonRespawnStage::End);
                    Self::reset_spike_crystals(world);
                    for crystal in self.live_respawn_crystals(world) {
                        set_crystal_beam(&crystal, None);
                        // Vanilla `level.explode(crystal, x, y, z, 6, NONE)`:
                        // breaks nothing, damages nothing out of range.
                        let position = crystal.position();
                        world.explode(
                            Some(Arc::clone(&crystal)),
                            position.x,
                            position.y,
                            position.z,
                            6.0,
                            ExplosionInteraction::None,
                        );
                        crystal.set_removed(RemovalReason::Discarded);
                    }
                } else {
                    if time == 0 {
                        let target = self.pool_block();
                        for crystal in self.live_respawn_crystals(world) {
                            set_crystal_beam(&crystal, Some(target));
                        }
                    }
                    if !(5..80).contains(&time) {
                        self.summon_roar(world);
                    }
                    self.respawn_time += 1;
                }
            }
            DragonRespawnStage::End => {}
        }
        self.set_dirty();
    }

    fn live_respawn_crystals(&self, world: &Arc<World>) -> Vec<SharedEntity> {
        self.respawn_crystals
            .iter()
            .filter_map(|uuid| world.get_entity_by_uuid(uuid))
            .filter(|entity| {
                !entity.is_removed() && entity.entity_type() == &vanilla_entities::END_CRYSTAL
            })
            .collect()
    }

    fn summon_roar(&self, world: &Arc<World>) {
        world.level_event(ANIMATION_DRAGON_SUMMON_ROAR, self.pool_block(), 0, None);
    }

    /// Vanilla `EnderDragonFight.setDragonKilled`: hide the bar, open the exit
    /// portal, spawn the next gateway, and place the dragon egg on a first kill.
    pub fn set_dragon_killed(&mut self, world: &Arc<World>, dragon_uuid: Uuid) {
        if self.dragon_uuid != Some(dragon_uuid) {
            return;
        }
        self.dragon_event.set_progress(0.0);
        self.dragon_event.set_visible(false);
        self.players.clear();
        self.spawn_exit_portal(world, true);
        self.spawn_new_gateway(world);
        if !self.previously_killed {
            // Vanilla places the egg on the `MOTION_BLOCKING` heightmap of the
            // podium column, i.e. on top of the four-block bedrock pillar,
            // rather than on the portal ring the fight cached.
            let podium = Self::podium_location(self.origin);
            let egg_pos = BlockPos::new(
                podium.x(),
                LevelReader::height_at(
                    world,
                    HeightmapType::MotionBlocking,
                    podium.x(),
                    podium.z(),
                ),
                podium.z(),
            );
            world.set_block(egg_pos, PODIUM_STATES.dragon_egg, UpdateFlags::UPDATE_ALL);
        }
        self.previously_killed = true;
        self.dragon_killed = true;
        self.set_dirty();
    }

    /// Vanilla `EnderDragonFight.tryRespawn`: four crystals on the portal edges
    /// start the respawn sequence.
    pub fn try_respawn(&mut self, world: &Arc<World>) {
        if !self.dragon_killed || self.respawn_stage.is_some() {
            return;
        }
        let location = match self.exit_portal_location {
            Some(location) => location,
            None => {
                if self.find_exit_portal(world).is_none() {
                    self.spawn_exit_portal(world, true);
                }
                let Some(location) = self.exit_portal_location else {
                    return;
                };
                location
            }
        };
        let center = location.above_n(1);
        let mut crystals = Vec::new();
        for (dx, dz) in [(3, 0), (-3, 0), (0, 3), (0, -3)] {
            let edge = BlockPos::new(center.x() + dx, center.y(), center.z() + dz);
            let box_ = WorldAabb::of_size(
                DVec3::new(
                    f64::from(edge.x()) + 0.5,
                    f64::from(edge.y()) + 0.5,
                    f64::from(edge.z()) + 0.5,
                ),
                1.0,
                1.0,
                1.0,
            );
            let found = world.get_entities_in_aabb_matching(&box_, |entity| {
                entity.entity_type() == &vanilla_entities::END_CRYSTAL
            });
            if found.is_empty() {
                return;
            }
            crystals.extend(found);
        }
        self.respawn_dragon(world, crystals);
    }

    /// Vanilla `EnderDragonFight.respawnDragon`: replace the fountain with end
    /// stone and start the `START` stage.
    fn respawn_dragon(&mut self, world: &Arc<World>, crystals: Vec<SharedEntity>) {
        if !self.dragon_killed || self.respawn_stage.is_some() {
            return;
        }
        if let Some(location) = self.find_exit_portal(world) {
            for dx in -PODIUM_RADIUS..=PODIUM_RADIUS {
                for dz in -PODIUM_RADIUS..=PODIUM_RADIUS {
                    for dy in 0..=1 {
                        let pos =
                            BlockPos::new(location.x() + dx, location.y() + dy, location.z() + dz);
                        let state = world.get_block_state(pos);
                        if state.get_block() == &vanilla_blocks::BEDROCK
                            || state.get_block() == &vanilla_blocks::END_PORTAL
                        {
                            world.set_block(pos, PODIUM_STATES.end_stone, UpdateFlags::UPDATE_ALL);
                        }
                    }
                }
            }
        }
        self.respawn_stage = Some(DragonRespawnStage::Start);
        self.respawn_time = 0;
        self.spawn_exit_portal(world, false);
        self.respawn_crystals = crystals.iter().map(|entity| entity.uuid()).collect();
        self.set_dirty();
    }

    /// Vanilla `EnderDragonFight.onCrystalDestroyed`.
    pub fn on_crystal_destroyed(&mut self, world: &Arc<World>, crystal_uuid: Uuid) {
        if self.respawn_stage.is_some() && self.respawn_crystals.contains(&crystal_uuid) {
            self.abort_respawn_sequence(world);
        } else {
            self.update_crystal_count(world);
        }
    }

    /// Vanilla `EnderDragonFight.abortRespawnSequence`.
    fn abort_respawn_sequence(&mut self, world: &Arc<World>) {
        self.respawn_stage = None;
        self.respawn_time = 0;
        Self::reset_spike_crystals(world);
        self.spawn_exit_portal(world, true);
        self.set_dirty();
    }

    /// Vanilla `EnderDragonFight.resetSpikeCrystals`.
    pub fn reset_spike_crystals(world: &Arc<World>) {
        for crystal in Self::spike_crystals(world) {
            if let Some(crystal) = crystal.downcast_ref::<EndCrystalEntity>() {
                crystal.set_invulnerable(false);
                crystal.set_beam_target(None);
            }
        }
    }

    /// Vanilla `EnderDragonFight.updateCrystalCount`.
    fn update_crystal_count(&mut self, world: &Arc<World>) {
        self.ticks_since_crystals_scanned = 0;
        self.alive_crystals = Self::spike_crystals(world).len() as i32;
        self.set_dirty();
    }

    /// Live crystals standing on the arena spikes.
    fn spike_crystals(world: &Arc<World>) -> Vec<SharedEntity> {
        let mut crystals = Vec::new();
        for spike in end_spikes_for_seed(world.seed()) {
            let box_ = WorldAabb::from_min_max(
                DVec3::new(
                    f64::from(spike.center_x - spike.radius),
                    f64::from(world.get_min_y()),
                    f64::from(spike.center_z - spike.radius),
                ),
                DVec3::new(
                    f64::from(spike.center_x + spike.radius) + 1.0,
                    f64::from(world.get_max_y()),
                    f64::from(spike.center_z + spike.radius) + 1.0,
                ),
            );
            crystals.extend(world.get_entities_in_aabb_matching(&box_, |entity| {
                entity.entity_type() == &vanilla_entities::END_CRYSTAL && !entity.is_removed()
            }));
        }
        crystals
    }

    /// Vanilla `EnderDragonFight.getPodiumLocation`.
    ///
    /// Vanilla's `END_PODIUM_LOCATION` is `BlockPos.ZERO`, so the podium column
    /// is the fight origin itself. The perch the dragon lands on is the
    /// `MOTION_BLOCKING_NO_LEAVES` first-available block of this column, which
    /// `EnderDragonEntity::podium_heightmap_pos` resolves for the phases.
    #[must_use]
    pub const fn podium_location(origin: BlockPos) -> BlockPos {
        // `END_PODIUM_LOCATION.offset(origin)`: `BlockPos.ZERO` is the identity
        // offset, so vanilla's constant collapses to the origin.
        origin
    }

    /// Vanilla `EnderDragonFight.spawnExitPortal`: build the fountain at the
    /// cached exit-portal location, deriving one when the fight never resolved
    /// it (fresh or legacy worlds).
    fn spawn_exit_portal(&mut self, world: &Arc<World>, activated: bool) {
        let location = match self.exit_portal_location {
            Some(location) => location,
            None => {
                let location = self.derive_exit_portal_location(world);
                self.exit_portal_location = Some(location);
                location
            }
        };
        place_end_podium(world, location, activated);
        self.set_dirty();
    }

    /// Vanilla `EnderDragonFight.spawnExitPortal` location derivation.
    ///
    /// Starts one below the podium column's `MOTION_BLOCKING_NO_LEAVES`
    /// heightmap (the island surface on a fresh world) and walks down over any
    /// existing bedrock blob, so an already-built fountain is re-centred on its
    /// lowest bedrock block. The result is clamped to `minY + 1`.
    fn derive_exit_portal_location(&self, world: &Arc<World>) -> BlockPos {
        /// Vanilla's bedrock walk-down floor (`while (... && y > 63)`).
        const BEDROCK_WALK_FLOOR: i32 = 63;

        let podium = Self::podium_location(self.origin);
        let mut location = BlockPos::new(
            podium.x(),
            LevelReader::height_at(
                world,
                HeightmapType::MotionBlockingNoLeaves,
                podium.x(),
                podium.z(),
            ),
            podium.z(),
        )
        .below();
        while world.get_block_state(location).get_block() == &vanilla_blocks::BEDROCK
            && location.y() > BEDROCK_WALK_FLOOR
        {
            location = location.below();
        }
        location.at_y((world.get_min_y() + 1).max(location.y()))
    }

    /// Vanilla `EnderDragonFight.spawnNewGateway`.
    fn spawn_new_gateway(&mut self, world: &Arc<World>) {
        let Some(gateway) = self.gateways.pop() else {
            self.set_dirty();
            return;
        };
        let pos = Self::gateway_position(self.origin, gateway);
        place_end_gateway(world, pos);
        world.level_event(ANIMATION_END_GATEWAY_SPAWN, pos, 0, None);
        self.set_dirty();
    }

    /// Vanilla `spawnNewGateway` ring position for a shuffled gateway index:
    /// `Mth.floor(96.0 * cos/sin(2.0 * (-PI + PI / 20 * index)))` at y = 75.
    ///
    /// The doubling is vanilla's: twenty gateways sit 18° apart on the ring.
    #[must_use]
    fn gateway_position(origin: BlockPos, index: i32) -> BlockPos {
        let angle = 2.0 * (-PI + PI / 20.0 * f64::from(index));
        BlockPos::new(
            (GATEWAY_DISTANCE * angle.cos()).floor() as i32 + origin.x(),
            GATEWAY_Y,
            (GATEWAY_DISTANCE * angle.sin()).floor() as i32 + origin.z(),
        )
    }

    /// Serializes the fight to saved data.
    #[must_use]
    pub fn to_saved(&self) -> EnderDragonFightData {
        EnderDragonFightData {
            needs_state_scanning: self.needs_state_scanning,
            dragon_killed: self.dragon_killed,
            previously_killed: self.previously_killed,
            respawn_stage: self
                .respawn_stage
                .map(|stage| stage.serialized_name().to_owned()),
            respawn_time: self.respawn_time,
            dragon_uuid: self.dragon_uuid,
            exit_portal_location: self
                .exit_portal_location
                .map(|pos| [pos.x(), pos.y(), pos.z()]),
            gateways: self.gateways.clone(),
            respawn_crystals: self.respawn_crystals.clone(),
            origin: [self.origin.x(), self.origin.y(), self.origin.z()],
        }
    }

    /// Loads a fight from saved data.
    #[must_use]
    pub fn from_saved(data: EnderDragonFightData) -> Self {
        Self {
            dirty: false,
            needs_state_scanning: data.needs_state_scanning,
            dragon_killed: data.dragon_killed,
            previously_killed: data.previously_killed,
            respawn_stage: data
                .respawn_stage
                .as_deref()
                .and_then(DragonRespawnStage::from_serialized_name),
            respawn_time: data.respawn_time,
            dragon_uuid: data.dragon_uuid,
            exit_portal_location: data
                .exit_portal_location
                .map(|coords| BlockPos::new(coords[0], coords[1], coords[2])),
            gateways: data.gateways,
            respawn_crystals: data.respawn_crystals,
            dragon_event: Self::make_bar(),
            ticks_since_last_player_scan: TIME_BETWEEN_PLAYER_SCANS + 1,
            ticks_since_dragon_seen: 0,
            ticks_since_crystals_scanned: 0,
            alive_crystals: 0,
            origin: BlockPos::new(data.origin[0], data.origin[1], data.origin[2]),
            initialized: false,
            players: BTreeSet::new(),
        }
    }
}

/// Vanilla `SavedData` record for [`EnderDragonFight`].
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EnderDragonFightData {
    /// Whether the first-tick legacy scan still needs to run.
    #[serde(default = "default_true")]
    pub needs_state_scanning: bool,
    /// Whether the dragon is currently dead.
    #[serde(default)]
    pub dragon_killed: bool,
    /// Whether the dragon was ever killed on this world.
    #[serde(default)]
    pub previously_killed: bool,
    /// Active respawn stage name, if any.
    #[serde(default)]
    pub respawn_stage: Option<String>,
    /// Ticks spent in the current respawn stage.
    #[serde(default)]
    pub respawn_time: i32,
    /// Tracked dragon uuid.
    #[serde(default)]
    pub dragon_uuid: Option<Uuid>,
    /// Cached exit-portal location.
    #[serde(default)]
    pub exit_portal_location: Option<[i32; 3]>,
    /// Shuffled remaining gateway indices.
    #[serde(default)]
    pub gateways: Vec<i32>,
    /// Respawn-crystal entity uuids.
    #[serde(default)]
    pub respawn_crystals: Vec<Uuid>,
    /// Fight origin (vanilla `BlockPos.ZERO`), persisted so Steel can reload it.
    #[serde(default)]
    pub origin: [i32; 3],
}

const fn default_true() -> bool {
    true
}

/// Vanilla `EnderDragonFight.updateDragon` entry point used by the dragon
/// entity: feeds live health/name into the level's fight boss bar.
pub fn update_dragon_bar(
    world: &Arc<World>,
    health: f32,
    max_health: f32,
    custom_name: Option<&TextComponent>,
) {
    if let Some(fight) = world.dragon_fight() {
        fight.lock().update_dragon(health, max_health, custom_name);
    }
}

/// Vanilla `EnderDragonFight.setDragonKilled` entry point used by the dragon
/// entity's death animation.
pub fn notify_dragon_killed(world: &Arc<World>, dragon_uuid: Uuid) {
    if let Some(fight) = world.dragon_fight() {
        fight.lock().set_dragon_killed(world, dragon_uuid);
    }
}

/// Vanilla `EndSpikeFeature.EndSpike` runtime geometry.
#[derive(Debug, Clone, Copy)]
struct EndSpike {
    center_x: i32,
    center_z: i32,
    radius: i32,
    height: i32,
}

/// Vanilla `EndSpikeFeature.getSpikesForLevel`: the deterministic spike ring.
///
/// Mirrors `crate::worldgen::feature::features::end_spike::end_spikes_for_level`,
/// which is unreachable here because that module is private to `worldgen`.
/// Reported: make it `pub(crate)` and delete this copy.
fn end_spikes_for_seed(seed: i64) -> Vec<EndSpike> {
    const COUNT: usize = 10;
    const DISTANCE: f64 = 42.0;
    const ANGLE_STEP: f64 = PI / 10.0;
    let mut seed_random = LegacyRandom::from_seed(seed as u64);
    let cache_key = seed_random.next_i64() & 65_535;
    let mut random = LegacyRandom::from_seed(cache_key as u64);
    let mut sizes = [0_i32; COUNT];
    for (index, size) in sizes.iter_mut().enumerate() {
        *size = index as i32;
    }
    for bound in (2..=COUNT).rev() {
        let swap_to = random.next_i32_bounded(bound as i32) as usize;
        sizes.swap(bound - 1, swap_to);
    }
    sizes
        .iter()
        .enumerate()
        .map(|(index, size)| {
            let angle = 2.0 * (-PI + ANGLE_STEP * index as f64);
            EndSpike {
                center_x: (DISTANCE * angle.cos()).floor() as i32,
                center_z: (DISTANCE * angle.sin()).floor() as i32,
                radius: 2 + size / 3,
                height: 76 + size * 3,
            }
        })
        .collect()
}

fn set_crystal_beam(crystal: &SharedEntity, target: Option<BlockPos>) {
    if let Some(crystal) = crystal.downcast_ref::<EndCrystalEntity>() {
        crystal.set_beam_target(target);
    }
}

/// Vanilla `DragonRespawnStage.SUMMONING_PILLARS` pillar rebuild: clear the
/// 21³ volume with the vanilla `explode(power 5, BLOCK)` call, then restore
/// the obsidian pillar with its bedrock cap.
fn rebuild_spike(world: &Arc<World>, spike: EndSpike) {
    world.explode(
        None,
        f64::from(spike.center_x) + 0.5,
        f64::from(spike.height),
        f64::from(spike.center_z) + 0.5,
        5.0,
        ExplosionInteraction::Block,
    );

    for dx in -spike.radius..=spike.radius {
        for dz in -spike.radius..=spike.radius {
            if dx * dx + dz * dz > spike.radius * spike.radius + 1 {
                continue;
            }
            for y in world.get_min_y()..spike.height {
                world.set_block(
                    BlockPos::new(spike.center_x + dx, y, spike.center_z + dz),
                    PODIUM_STATES.obsidian,
                    UpdateFlags::UPDATE_NONE,
                );
            }
        }
    }

    world.set_block(
        BlockPos::new(spike.center_x, spike.height - 1, spike.center_z),
        PODIUM_STATES.bedrock,
        UpdateFlags::UPDATE_NONE,
    );
    world.set_block(
        BlockPos::new(spike.center_x, spike.height, spike.center_z),
        PODIUM_STATES.bedrock,
        UpdateFlags::UPDATE_NONE,
    );
}

/// Vanilla `EndPodiumFeature.place`.
fn place_end_podium(world: &Arc<World>, origin: BlockPos, activated: bool) {
    let center = DVec3::new(
        f64::from(origin.x()) + 0.5,
        f64::from(origin.y()) + 0.5,
        f64::from(origin.z()) + 0.5,
    );
    for dx in -PODIUM_RADIUS..=PODIUM_RADIUS {
        for dz in -PODIUM_RADIUS..=PODIUM_RADIUS {
            for dy in -1..=32 {
                let pos = BlockPos::new(origin.x() + dx, origin.y() + dy, origin.z() + dz);
                let point = DVec3::new(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()) + 0.5,
                    f64::from(pos.z()) + 0.5,
                );
                let distance_sqr = center.distance_squared(point);
                let inside_rim = distance_sqr < 2.5 * 2.5;
                if !(inside_rim || distance_sqr < 3.5 * 3.5) {
                    continue;
                }
                if dy < 0 {
                    if inside_rim {
                        world.set_block(pos, PODIUM_STATES.bedrock, UpdateFlags::UPDATE_ALL);
                    } else {
                        world.set_block(pos, PODIUM_STATES.end_stone, UpdateFlags::UPDATE_ALL);
                    }
                } else if dy > 0 {
                    if activated {
                        world.destroy_block(pos, true);
                    } else {
                        world.set_block(pos, PODIUM_STATES.air, UpdateFlags::UPDATE_ALL);
                    }
                } else if !inside_rim {
                    world.set_block(pos, PODIUM_STATES.bedrock, UpdateFlags::UPDATE_ALL);
                } else if activated {
                    world.set_block(pos, PODIUM_STATES.end_portal, UpdateFlags::UPDATE_ALL);
                } else {
                    world.set_block(pos, PODIUM_STATES.air, UpdateFlags::UPDATE_ALL);
                }
            }
        }
    }

    for y in 0..PODIUM_PILLAR_HEIGHT {
        world.set_block(
            origin.above_n(y),
            PODIUM_STATES.bedrock,
            UpdateFlags::UPDATE_ALL,
        );
    }

    for direction in [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ] {
        // Vanilla `WALL_TORCH.defaultBlockState().setValue(FACING, direction)`.
        let torch = PODIUM_STATES
            .wall_torch
            .set_value(&BlockStateProperties::HORIZONTAL_FACING, direction);
        world.set_block(
            origin.above_n(2).relative(direction),
            torch,
            UpdateFlags::UPDATE_ALL,
        );
    }
}

/// Vanilla `EndGatewayFeature.place`.
fn place_end_gateway(world: &Arc<World>, origin: BlockPos) {
    for dx in -1..=1 {
        for dy in -2..=2 {
            for dz in -1..=1 {
                let pos = BlockPos::new(origin.x() + dx, origin.y() + dy, origin.z() + dz);
                let same_x = dx == 0;
                let same_y = dy == 0;
                let same_z = dz == 0;
                let end = dy.abs() == 2;
                let state = if same_x && same_y && same_z {
                    PODIUM_STATES.end_gateway
                } else if same_y {
                    PODIUM_STATES.air
                } else if (end && same_x && same_z) || ((same_x || same_z) && !end) {
                    PODIUM_STATES.bedrock
                } else {
                    PODIUM_STATES.air
                };
                world.set_block(pos, state, UpdateFlags::UPDATE_ALL);
            }
        }
    }
}

struct PodiumStates {
    bedrock: BlockStateId,
    end_stone: BlockStateId,
    obsidian: BlockStateId,
    air: BlockStateId,
    end_portal: BlockStateId,
    end_gateway: BlockStateId,
    dragon_egg: BlockStateId,
    wall_torch: BlockStateId,
}

impl PodiumStates {
    fn load() -> Self {
        let blocks = &REGISTRY.blocks;
        Self {
            bedrock: blocks.get_default_state_id(&vanilla_blocks::BEDROCK),
            end_stone: blocks.get_default_state_id(&vanilla_blocks::END_STONE),
            obsidian: blocks.get_default_state_id(&vanilla_blocks::OBSIDIAN),
            air: blocks.get_default_state_id(&vanilla_blocks::AIR),
            end_portal: blocks.get_default_state_id(&vanilla_blocks::END_PORTAL),
            end_gateway: blocks.get_default_state_id(&vanilla_blocks::END_GATEWAY),
            dragon_egg: blocks.get_default_state_id(&vanilla_blocks::DRAGON_EGG),
            wall_torch: blocks.get_default_state_id(&vanilla_blocks::WALL_TORCH),
        }
    }
}

static PODIUM_STATES: LazyLock<PodiumStates> = LazyLock::new(PodiumStates::load);

#[cfg(test)]
mod tests {
    use steel_registry::blocks::block_state_ext::BlockStateExt;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_dimension_types};
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::{
        TestWorld, fresh_test_world_with_dimension_type, insert_ready_full_chunk,
    };

    /// Island surface used by the fixtures. Above vanilla `spawnExitPortal`'s
    /// bedrock walk-down floor (y = 63), so a fresh-world derivation must stop
    /// here instead of descending.
    const ISLAND_SURFACE_Y: i32 = 64;

    /// A fresh End world whose origin column is a single end-stone island block
    /// at [`ISLAND_SURFACE_Y`], like the central island before the fight builds
    /// the fountain.
    fn end_world_with_island(key: &'static str) -> TestWorld {
        init_vanilla_registry();
        init_behaviors();
        let fixture =
            fresh_test_world_with_dimension_type("steel", key, &vanilla_dimension_types::THE_END);
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert!(world.set_block(
            BlockPos::new(0, ISLAND_SURFACE_Y, 0),
            vanilla_blocks::END_STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        fixture
    }

    /// Vanilla `spawnExitPortal` derives a missing exit-portal location from the
    /// podium column's `MOTION_BLOCKING_NO_LEAVES` heightmap, so the fountain is
    /// built on the island surface on a fresh End world rather than at the fight
    /// origin's y (which is 0 and leaves the perch in the void).
    #[test]
    fn fresh_world_podium_lands_on_the_island_surface() {
        let fixture = end_world_with_island("end_fresh_podium");
        let world = &fixture.world;

        let mut fight = EnderDragonFight::new(BlockPos::ZERO);
        fight.spawn_exit_portal(world, false);

        let podium = BlockPos::new(0, ISLAND_SURFACE_Y, 0);
        assert_eq!(fight.exit_portal_location(), Some(podium));
        // The four-block bedrock pillar stands on the surface: its base is the
        // portal ring and its top is the perch the dragon lands on.
        let bedrock = PODIUM_STATES.bedrock;
        assert_eq!(world.get_block_state(podium), bedrock);
        assert_eq!(world.get_block_state(podium.above_n(3)), bedrock);
        // Vanilla lights the pillar with four faced wall torches.
        let east_torch = world.get_block_state(BlockPos::new(1, ISLAND_SURFACE_Y + 2, 0));
        assert_eq!(east_torch.get_block(), &vanilla_blocks::WALL_TORCH);
        assert_eq!(
            east_torch.get_value(&BlockStateProperties::HORIZONTAL_FACING),
            Direction::East
        );
    }

    /// Vanilla places the dragon egg on the `MOTION_BLOCKING` heightmap of the
    /// podium column, i.e. on top of the pillar, not on the portal ring.
    #[test]
    fn first_kill_places_the_dragon_egg_on_the_pillar_top() {
        let fixture = end_world_with_island("end_dragon_egg");
        let world = &fixture.world;

        let mut fight = EnderDragonFight::new(BlockPos::ZERO);
        let dragon_uuid = Uuid::new_v4();
        fight.dragon_uuid = Some(dragon_uuid);
        fight.set_dragon_killed(world, dragon_uuid);

        let egg = BlockPos::new(0, ISLAND_SURFACE_Y + 4, 0);
        assert_eq!(world.get_block_state(egg), PODIUM_STATES.dragon_egg);
        // The pillar is intact below the egg.
        assert_eq!(
            world.get_block_state(BlockPos::new(0, ISLAND_SURFACE_Y + 3, 0)),
            PODIUM_STATES.bedrock
        );
    }

    /// Vanilla spreads the twenty gateways 18° apart on a 96 block ring at
    /// y = 75: `floor(96.0 * cos(2.0 * (-PI + PI / 20 * index)))`.
    #[test]
    fn gateway_ring_positions_match_vanilla() {
        let first = EnderDragonFight::gateway_position(BlockPos::ZERO, 0);
        assert_eq!(first.x(), 96);
        assert_eq!(first.y(), GATEWAY_Y);
        assert_eq!(
            EnderDragonFight::gateway_position(BlockPos::ZERO, 19),
            BlockPos::new(91, GATEWAY_Y, -30)
        );
    }
}

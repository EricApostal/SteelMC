//! Vanilla ender-dragon phase instances.
//!
//! Mirrors `net.minecraft.world.entity.boss.enderdragon.phases`: the phase
//! registry ([`EnderDragonPhase`]), the phase manager
//! ([`EnderDragonPhaseManager`]) and one instance per phase. Each instance
//! reproduces the server-side behaviour of its vanilla counterpart; vanilla's
//! `doClientTick` presentation (breath particles, local growls) is not ported
//! because Steel has no client.

use std::sync::Arc;

use glam::DVec3;
use steel_math::wrap_degrees;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::level_events;
use steel_registry::particle_type::{ParticleData, PowerParticleOption};
use steel_registry::{
    MobEffectInstance, vanilla_entities, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::BlockPos;
use steel_utils::locks::SyncMutex;

use crate::chunk::heightmap::HeightmapType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::{AreaEffectCloudEntity, DragonFireballEntity};
use crate::entity::{Entity, LivingEntity, Projectile, SharedEntity, next_entity_id};
use crate::world::{LevelReader, World};

use super::ender_dragon::{DragonPath, EnderDragonEntity, normalize_vec};

/// Degrees-to-radians factor used throughout the vanilla dragon.
const DEG_TO_RAD: f32 = std::f32::consts::PI / 180.0;
/// Radians-to-degrees factor used throughout the vanilla dragon.
const RAD_TO_DEG: f32 = 180.0 / std::f32::consts::PI;
/// Vanilla `DragonChargePlayerPhase.CHARGE_RECOVERY_TIME`.
const CHARGE_RECOVERY_TIME: i32 = 10;
/// Vanilla `DragonStrafePlayerPhase.FIREBALL_CHARGE_AMOUNT`.
const FIREBALL_CHARGE_AMOUNT: i32 = 5;
/// Vanilla `DragonSittingScanningPhase.SITTING_SCANNING_IDLE_TICKS`.
const SITTING_SCANNING_IDLE_TICKS: i32 = 100;
/// Vanilla `DragonSittingScanningPhase.SITTING_ATTACK_Y_VIEW_RANGE`.
const SITTING_ATTACK_Y_VIEW_RANGE: f64 = 10.0;
/// Vanilla `DragonSittingScanningPhase.SITTING_ATTACK_VIEW_RANGE`.
const SITTING_ATTACK_VIEW_RANGE: f64 = 20.0;
/// Vanilla `DragonSittingScanningPhase.SITTING_CHARGE_VIEW_RANGE`.
const SITTING_CHARGE_VIEW_RANGE: f64 = 150.0;
/// Vanilla `DragonSittingFlamingPhase.FLAME_DURATION`.
const FLAME_DURATION: i32 = 200;
/// Vanilla `DragonSittingFlamingPhase.SITTING_FLAME_ATTACKS_COUNT`.
const SITTING_FLAME_ATTACKS_COUNT: i32 = 4;
/// Vanilla `DragonSittingFlamingPhase.WARMUP_TIME`.
const WARMUP_TIME: i32 = 10;
/// Vanilla `DragonSittingAttackingPhase.ROAR_DURATION`.
const ROAR_DURATION: i32 = 40;
/// Vanilla `DragonTakeoffPhase` podium proximity.
const TAKEOFF_PODIUM_RANGE: f64 = 10.0;
/// Vanilla `DragonLandingPhase` arrival distance.
const LANDING_ARRIVAL_DISTANCE_SQR: f64 = 1.0;
/// Vanilla `DragonHoldingPatternPhase`/`DragonLandingApproachPhase` retarget band.
const RETARGET_MIN_DISTANCE_SQR: f64 = 100.0;
/// Vanilla `DragonHoldingPatternPhase`/`DragonLandingApproachPhase` retarget band.
const RETARGET_MAX_DISTANCE_SQR: f64 = 22500.0;
/// Vanilla `DragonStrafePlayerPhase` fireball range.
const STRAFE_FIREBALL_RANGE_SQR: f64 = 4096.0;
/// Vanilla `DragonSittingFlamingPhase` breath cloud radius.
const FLAME_CLOUD_RADIUS: f32 = 5.0;
/// Vanilla `DragonSittingFlamingPhase` breath cloud duration.
const FLAME_CLOUD_DURATION: i32 = 200;
/// Vanilla `DragonSittingFlamingPhase` potion duration scale.
const FLAME_CLOUD_POTION_DURATION_SCALE: f32 = 0.25;

/// Vanilla `EnderDragonPhase`.
///
/// The discriminants are vanilla's registry ids, in `EnderDragonPhase`
/// declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum EnderDragonPhase {
    /// Vanilla `HOLDING_PATTERN`.
    HoldingPattern = 0,
    /// Vanilla `STRAFE_PLAYER`.
    StrafePlayer = 1,
    /// Vanilla `LANDING_APPROACH`.
    LandingApproach = 2,
    /// Vanilla `LANDING`.
    Landing = 3,
    /// Vanilla `TAKEOFF`.
    Takeoff = 4,
    /// Vanilla `SITTING_FLAMING`.
    SittingFlaming = 5,
    /// Vanilla `SITTING_SCANNING`.
    SittingScanning = 6,
    /// Vanilla `SITTING_ATTACKING`.
    SittingAttacking = 7,
    /// Vanilla `CHARGING_PLAYER`.
    ChargingPlayer = 8,
    /// Vanilla `DYING`.
    Dying = 9,
    /// Vanilla `HOVERING`.
    Hovering = 10,
}

impl EnderDragonPhase {
    /// Number of registered phases (`EnderDragonPhase.getCount`).
    pub const COUNT: usize = 11;

    /// All phases in vanilla registry order.
    pub const ALL: [Self; Self::COUNT] = [
        Self::HoldingPattern,
        Self::StrafePlayer,
        Self::LandingApproach,
        Self::Landing,
        Self::Takeoff,
        Self::SittingFlaming,
        Self::SittingScanning,
        Self::SittingAttacking,
        Self::ChargingPlayer,
        Self::Dying,
        Self::Hovering,
    ];

    /// Vanilla `EnderDragonPhase.getById`, falling back to `HOLDING_PATTERN`.
    #[must_use]
    pub fn from_id(id: i32) -> Self {
        Self::ALL
            .get(usize::try_from(id).unwrap_or(usize::MAX))
            .copied()
            .unwrap_or(Self::HoldingPattern)
    }

    /// Vanilla `EnderDragonPhase.getId`.
    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    /// Returns the phase as an index into the manager's instance table.
    const fn index(self) -> usize {
        self as usize
    }

    /// Vanilla `DragonPhaseInstance.isSitting`.
    ///
    /// `HOVERING` reports sitting as well; that is what makes the hovering
    /// dragon hover in place with its head lowered.
    #[must_use]
    pub const fn is_sitting(self) -> bool {
        matches!(
            self,
            Self::Hovering | Self::SittingFlaming | Self::SittingScanning | Self::SittingAttacking
        )
    }

    /// Vanilla `EnderDragonPhase.toString` name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::HoldingPattern => "HoldingPattern",
            Self::StrafePlayer => "StrafePlayer",
            Self::LandingApproach => "LandingApproach",
            Self::Landing => "Landing",
            Self::Takeoff => "Takeoff",
            Self::SittingFlaming => "SittingFlaming",
            Self::SittingScanning => "SittingScanning",
            Self::SittingAttacking => "SittingAttacking",
            Self::ChargingPlayer => "ChargingPlayer",
            Self::Dying => "Dying",
            Self::Hovering => "Hover",
        }
    }
}

impl std::fmt::Display for EnderDragonPhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} (#{})", self.name(), self.id())
    }
}

/// Vanilla `DragonPhaseInstance`.
pub trait DragonPhaseInstance: Send + Sync {
    /// Vanilla `getPhase`.
    fn phase(&self) -> EnderDragonPhase;

    /// Vanilla `isSitting`.
    fn is_sitting(&self) -> bool {
        false
    }

    /// Vanilla `doServerTick`.
    fn do_server_tick(&mut self, _dragon: &EnderDragonEntity, _world: &Arc<World>) {}

    /// Vanilla `onCrystalDestroyed`.
    fn on_crystal_destroyed(
        &mut self,
        _dragon: &EnderDragonEntity,
        _crystal: &SharedEntity,
        _pos: BlockPos,
        _source: &DamageSource,
        _player: Option<&crate::player::Player>,
    ) {
    }

    /// Vanilla `begin`.
    fn begin(&mut self, _dragon: &EnderDragonEntity) {}

    /// Vanilla `end`.
    fn end(&mut self, _dragon: &EnderDragonEntity) {}

    /// Vanilla `getFlySpeed`.
    fn fly_speed(&self) -> f32 {
        0.6
    }

    /// Vanilla `getTurnSpeed`.
    fn turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let velocity = dragon.velocity();
        let rot_speed = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt() as f32 + 1.0;
        let dist = rot_speed.min(40.0);
        0.7 / dist / rot_speed
    }

    /// Vanilla `getFlyTargetLocation`.
    fn fly_target(&self) -> Option<DVec3> {
        None
    }

    /// Vanilla `onHurt`.
    fn on_hurt(&mut self, _dragon: &EnderDragonEntity, _source: &DamageSource, damage: f32) -> f32 {
        damage
    }

    /// Sets the strafe target; only `DragonStrafePlayerPhase` honors it.
    fn set_strafe_target(&mut self, _dragon: &EnderDragonEntity, _target: &SharedEntity) {}

    /// Sets the charge target; only `DragonChargePlayerPhase` honors it.
    fn set_charge_target(&mut self, _target: DVec3) {}

    /// Resets the flame burst count; only `DragonSittingFlamingPhase` honors it.
    fn reset_flame_count(&mut self) {}
}

/// Vanilla `AbstractDragonSittingPhase`.
///
/// Shared by the three sitting phases: arrows and wind charges are deflected by
/// igniting them instead of damaging the dragon.
fn sitting_on_hurt(source: &DamageSource, damage: f32) -> f32 {
    let Some(direct) = source.direct_entity() else {
        return damage;
    };
    let is_arrow = [
        &vanilla_entities::ARROW,
        &vanilla_entities::SPECTRAL_ARROW,
        &vanilla_entities::TRIDENT,
    ]
    .contains(&direct.entity_type());
    let is_wind_charge = [
        &vanilla_entities::WIND_CHARGE,
        &vanilla_entities::BREEZE_WIND_CHARGE,
    ]
    .contains(&direct.entity_type());

    if !is_arrow && !is_wind_charge {
        return damage;
    }

    // Vanilla `igniteForSeconds(1.0F)`.
    direct.ignite_for_ticks(20);
    0.0
}

/// Vanilla `DragonHoldingPatternPhase.navigateToNextPathNode` and its siblings.
///
/// The vanilla `do/while` re-rolls while `yTarget < current.getY()`; `nextFloat`
/// is never negative, so a single roll always satisfies the loop.
fn navigate_to_next_path_node(
    dragon: &EnderDragonEntity,
    path: &mut Option<DragonPath>,
) -> Option<DVec3> {
    let path = path.as_mut()?;
    if path.is_done() {
        return None;
    }

    let current = path.next_node_pos()?;
    path.advance();
    let x_target = f64::from(current.x());
    let z_target = f64::from(current.z());
    let y_target = f64::from(current.y()) + f64::from(dragon.random_f32()) * 20.0;
    Some(DVec3::new(x_target, y_target, z_target))
}

/// Vanilla `DragonHoldingPatternPhase`.
pub struct DragonHoldingPatternPhase {
    current_path: Option<DragonPath>,
    target_location: Option<DVec3>,
    clockwise: bool,
}

impl DragonHoldingPatternPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current_path: None,
            target_location: None,
            clockwise: false,
        }
    }

    fn find_new_target(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        let path_done = self.current_path.as_ref().is_some_and(DragonPath::is_done);
        if path_done {
            let egg = dragon.podium_heightmap_pos(world, HeightmapType::MotionBlockingNoLeaves);
            let crystals = dragon.alive_crystals_or_zero();
            if dragon.random_i32_bounded(crystals + 3) == 0 {
                dragon.set_phase(EnderDragonPhase::LandingApproach);
                return;
            }

            // Vanilla `NEW_TARGET_TARGETING`: combat targeting that ignores
            // line of sight, with the dragon as the targeter.
            let egg_center = DVec3::new(
                f64::from(egg.x()) + 0.5,
                f64::from(egg.y()) + 0.5,
                f64::from(egg.z()) + 0.5,
            );
            let player = world.nearest_player(egg_center, -1.0, |player| {
                !player.is_spectator() && dragon.can_attack(player)
            });
            let dist_sqr = player.as_ref().map_or(64.0, |player| {
                block_center_distance_sqr(egg, player.position()) / 512.0
            });

            if let Some(player) = player
                && (dragon.random_i32_bounded(dist_sqr as i32 + 2) == 0
                    || dragon.random_i32_bounded(crystals + 2) == 0)
            {
                self.strafe_player(dragon, player);
                return;
            }
        }

        if self.current_path.as_ref().is_none_or(DragonPath::is_done) {
            let current_node_index = dragon.find_closest_node();
            let mut target_node_index = current_node_index;
            if dragon.random_i32_bounded(8) == 0 {
                self.clockwise = !self.clockwise;
                target_node_index += 6;
            }

            if self.clockwise {
                target_node_index += 1;
            } else {
                target_node_index -= 1;
            }

            if dragon.dragon_fight_present() && dragon.alive_crystals() >= 0 {
                target_node_index %= 12;
                if target_node_index < 0 {
                    target_node_index += 12;
                }
            } else {
                target_node_index -= 12;
                target_node_index &= 7;
                target_node_index += 12;
            }

            self.current_path =
                dragon.find_path(current_node_index, target_node_index.max(0) as usize, None);
            if let Some(path) = self.current_path.as_mut() {
                path.advance();
            }
        }

        if let Some(location) = navigate_to_next_path_node(dragon, &mut self.current_path) {
            self.target_location = Some(location);
        }
    }

    fn strafe_player(&self, dragon: &EnderDragonEntity, player: Arc<crate::player::Player>) {
        dragon.set_phase(EnderDragonPhase::StrafePlayer);
        let player: SharedEntity = player;
        dragon
            .phases()
            .with_instance(EnderDragonPhase::StrafePlayer, &mut |instance| {
                instance.set_strafe_target(dragon, &player);
            });
    }
}

impl Default for DragonHoldingPatternPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonHoldingPatternPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::HoldingPattern
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        let dist_to_target = self.target_location.map_or(0.0, |target| {
            let position = dragon.position();
            (target.x - position.x) * (target.x - position.x)
                + (target.y - position.y) * (target.y - position.y)
                + (target.z - position.z) * (target.z - position.z)
        });
        if dist_to_target < RETARGET_MIN_DISTANCE_SQR
            || dist_to_target > RETARGET_MAX_DISTANCE_SQR
            || dragon.horizontal_collision()
            || dragon.vertical_collision()
        {
            self.find_new_target(dragon, world);
        }
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.current_path = None;
        self.target_location = None;
    }

    fn fly_target(&self) -> Option<DVec3> {
        self.target_location
    }

    fn on_crystal_destroyed(
        &mut self,
        dragon: &EnderDragonEntity,
        _crystal: &SharedEntity,
        _pos: BlockPos,
        _source: &DamageSource,
        player: Option<&crate::player::Player>,
    ) {
        let Some(player) = player else {
            return;
        };
        if !dragon.can_attack(player) {
            return;
        }
        if let Some(shared) = dragon.shared_handle() {
            dragon.set_phase(EnderDragonPhase::StrafePlayer);
            dragon
                .phases()
                .with_instance(EnderDragonPhase::StrafePlayer, &mut |instance| {
                    instance.set_strafe_target(dragon, &shared);
                });
        }
    }
}

/// Vanilla `DragonStrafePlayerPhase`.
pub struct DragonStrafePlayerPhase {
    fireball_charge: i32,
    current_path: Option<DragonPath>,
    target_location: Option<DVec3>,
    attack_target: Option<SharedEntity>,
    holding_pattern_clockwise: bool,
}

impl DragonStrafePlayerPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fireball_charge: 0,
            current_path: None,
            target_location: None,
            attack_target: None,
            holding_pattern_clockwise: false,
        }
    }

    fn find_new_target(&mut self, dragon: &EnderDragonEntity) {
        if self.current_path.as_ref().is_none_or(DragonPath::is_done) {
            let current_node_index = dragon.find_closest_node();
            let mut target_node_index = current_node_index;
            if dragon.random_i32_bounded(8) == 0 {
                self.holding_pattern_clockwise = !self.holding_pattern_clockwise;
                target_node_index += 6;
            }

            if self.holding_pattern_clockwise {
                target_node_index += 1;
            } else {
                target_node_index -= 1;
            }

            if dragon.dragon_fight_present() && dragon.alive_crystals() > 0 {
                target_node_index %= 12;
                if target_node_index < 0 {
                    target_node_index += 12;
                }
            } else {
                target_node_index -= 12;
                target_node_index &= 7;
                target_node_index += 12;
            }

            self.current_path =
                dragon.find_path(current_node_index, target_node_index.max(0) as usize, None);
            if let Some(path) = self.current_path.as_mut() {
                path.advance();
            }
        }

        if let Some(location) = navigate_to_next_path_node(dragon, &mut self.current_path) {
            self.target_location = Some(location);
        }
    }

    fn spawn_fireball(
        &self,
        dragon: &EnderDragonEntity,
        world: &Arc<World>,
        target: &SharedEntity,
    ) {
        let Some(owner) = dragon.shared_handle() else {
            return;
        };
        let view = dragon.look_angle();
        let head = dragon.head_position();
        let start = DVec3::new(head.x - view.x, head.y + 0.5 + 0.5, head.z - view.z);
        let target_position = target.position();
        let direction = normalize_vec(DVec3::new(
            target_position.x - start.x,
            target_position.y + 0.5 - start.y,
            target_position.z - start.z,
        ));

        if !dragon.is_silent() {
            world.level_event(
                level_events::SOUND_DRAGON_FIREBALL,
                dragon.block_position(),
                0,
                None,
            );
        }

        let fireball = DragonFireballEntity::with_direction(
            &vanilla_entities::DRAGON_FIREBALL,
            next_entity_id(),
            start,
            direction,
            Arc::downgrade(world),
        );
        fireball.set_owner_entity(Some(&owner));
        let fireball: SharedEntity = Arc::new(fireball);
        if let Err(error) = world.try_add_entity(fireball) {
            log::debug!("failed to spawn ender dragon fireball: {error}");
        }
    }
}

impl Default for DragonStrafePlayerPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonStrafePlayerPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::StrafePlayer
    }

    fn set_strafe_target(&mut self, dragon: &EnderDragonEntity, target: &SharedEntity) {
        self.set_target(dragon, target);
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        let Some(attack_target) = self.attack_target.clone() else {
            log::debug!("skipping ender dragon strafe phase: no player was found");
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
            return;
        };

        if self.current_path.as_ref().is_some_and(DragonPath::is_done) {
            let target_position = attack_target.position();
            let x_target = target_position.x;
            let z_target = target_position.z;
            let position = dragon.position();
            let x_target_dist = x_target - position.x;
            let z_target_dist = z_target - position.z;
            let dist = (x_target_dist * x_target_dist + z_target_dist * z_target_dist).sqrt();
            let height_offset = (0.4 + dist / 80.0 - 1.0).min(10.0);
            self.target_location = Some(DVec3::new(
                x_target,
                target_position.y + height_offset,
                z_target,
            ));
        }

        let dist_to_target = self.target_location.map_or(0.0, |target| {
            let position = dragon.position();
            (target.x - position.x) * (target.x - position.x)
                + (target.y - position.y) * (target.y - position.y)
                + (target.z - position.z) * (target.z - position.z)
        });
        if dist_to_target < RETARGET_MIN_DISTANCE_SQR || dist_to_target > RETARGET_MAX_DISTANCE_SQR
        {
            self.find_new_target(dragon);
        }

        if attack_target.position().distance_squared(dragon.position()) < STRAFE_FIREBALL_RANGE_SQR
        {
            if dragon.has_line_of_sight(attack_target.as_ref()) {
                self.fireball_charge += 1;
                let position = dragon.position();
                let target_position = attack_target.position();
                let aim = normalize_vec(DVec3::new(
                    target_position.x - position.x,
                    0.0,
                    target_position.z - position.z,
                ));
                let yaw = dragon.rotation().0 * DEG_TO_RAD;
                let dir = normalize_vec(DVec3::new(yaw.sin() as f64, 0.0, -(yaw.cos() as f64)));
                let dot = dir.dot(aim) as f32;
                let mut angle_degrees = dot.acos() * RAD_TO_DEG;
                angle_degrees += 0.5;

                if self.fireball_charge >= FIREBALL_CHARGE_AMOUNT
                    && (0.0..10.0).contains(&angle_degrees)
                {
                    self.spawn_fireball(dragon, world, &attack_target);
                    self.fireball_charge = 0;
                    if let Some(path) = self.current_path.as_mut() {
                        while !path.is_done() {
                            path.advance();
                        }
                    }
                    dragon.set_phase(EnderDragonPhase::HoldingPattern);
                }
            } else if self.fireball_charge > 0 {
                self.fireball_charge -= 1;
            }
        } else if self.fireball_charge > 0 {
            self.fireball_charge -= 1;
        }
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.fireball_charge = 0;
        self.target_location = None;
        self.current_path = None;
        self.attack_target = None;
    }

    fn fly_target(&self) -> Option<DVec3> {
        self.target_location
    }

    fn on_crystal_destroyed(
        &mut self,
        _dragon: &EnderDragonEntity,
        _crystal: &SharedEntity,
        _pos: BlockPos,
        _source: &DamageSource,
        _player: Option<&crate::player::Player>,
    ) {
    }
}

impl DragonStrafePlayerPhase {
    /// Vanilla `DragonStrafePlayerPhase.setTarget`.
    fn set_target(&mut self, dragon: &EnderDragonEntity, target: &SharedEntity) {
        self.attack_target = Some(Arc::clone(target));

        let current_node_index = dragon.find_closest_node();
        let target_position = target.position();
        let target_node_index =
            dragon.find_closest_node_at(target_position.x, target_position.y, target_position.z);

        let block_position = target.block_position();
        let position = dragon.position();
        let xd = f64::from(block_position.x()) - position.x;
        let zd = f64::from(block_position.z()) - position.z;
        let sd = (xd * xd + zd * zd).sqrt();
        let height_offset = (0.4 + sd / 80.0 - 1.0).min(10.0);
        let final_y = (target_position.y + height_offset).floor() as i32;
        let final_node =
            super::ender_dragon::DragonNode::new(block_position.x(), final_y, block_position.z());

        self.current_path =
            dragon.find_path(current_node_index, target_node_index, Some(final_node));
        if let Some(path) = self.current_path.as_mut() {
            path.advance();
            if let Some(location) = navigate_to_next_path_node(dragon, &mut self.current_path) {
                self.target_location = Some(location);
            }
        }
    }
}

/// Vanilla `DragonLandingApproachPhase`.
pub struct DragonLandingApproachPhase {
    current_path: Option<DragonPath>,
    target_location: Option<DVec3>,
}

impl DragonLandingApproachPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current_path: None,
            target_location: None,
        }
    }

    fn find_new_target(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        if self.current_path.as_ref().is_none_or(DragonPath::is_done) {
            let current_node_index = dragon.find_closest_node();
            let egg = dragon.podium_heightmap_pos(world, HeightmapType::MotionBlockingNoLeaves);
            let egg_center = DVec3::new(
                f64::from(egg.x()) + 0.5,
                f64::from(egg.y()) + 0.5,
                f64::from(egg.z()) + 0.5,
            );
            let player = world.nearest_player(egg_center, -1.0, |player| {
                !player.is_spectator() && dragon.can_attack(player)
            });

            let target_node_index = if let Some(player) = player {
                let position = player.position();
                let aim = normalize_vec(DVec3::new(position.x, 0.0, position.z));
                dragon.find_closest_node_at(-aim.x * 40.0, 105.0, -aim.z * 40.0)
            } else {
                dragon.find_closest_node_at(40.0, f64::from(egg.y()), 0.0)
            };

            let final_node = super::ender_dragon::DragonNode::new(egg.x(), egg.y(), egg.z());
            self.current_path =
                dragon.find_path(current_node_index, target_node_index, Some(final_node));
            if let Some(path) = self.current_path.as_mut() {
                path.advance();
            }
        }

        if let Some(location) = navigate_to_next_path_node(dragon, &mut self.current_path) {
            self.target_location = Some(location);
        }

        if self.current_path.as_ref().is_some_and(DragonPath::is_done) {
            dragon.set_phase(EnderDragonPhase::Landing);
        }
    }
}

impl Default for DragonLandingApproachPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonLandingApproachPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::LandingApproach
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.current_path = None;
        self.target_location = None;
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        let dist_to_target = self.target_location.map_or(0.0, |target| {
            let position = dragon.position();
            (target.x - position.x) * (target.x - position.x)
                + (target.y - position.y) * (target.y - position.y)
                + (target.z - position.z) * (target.z - position.z)
        });
        if dist_to_target < RETARGET_MIN_DISTANCE_SQR
            || dist_to_target > RETARGET_MAX_DISTANCE_SQR
            || dragon.horizontal_collision()
            || dragon.vertical_collision()
        {
            self.find_new_target(dragon, world);
        }
    }

    fn fly_target(&self) -> Option<DVec3> {
        self.target_location
    }
}

/// Vanilla `DragonLandingPhase`.
pub struct DragonLandingPhase {
    target_location: Option<DVec3>,
}

impl DragonLandingPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target_location: None,
        }
    }
}

impl Default for DragonLandingPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonLandingPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::Landing
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        if self.target_location.is_none() {
            let egg = dragon.podium_heightmap_pos(world, HeightmapType::MotionBlockingNoLeaves);
            self.target_location = Some(DVec3::new(
                f64::from(egg.x()) + 0.5,
                f64::from(egg.y()),
                f64::from(egg.z()) + 0.5,
            ));
        }

        let Some(target) = self.target_location else {
            return;
        };
        let position = dragon.position();
        let distance = (target.x - position.x) * (target.x - position.x)
            + (target.y - position.y) * (target.y - position.y)
            + (target.z - position.z) * (target.z - position.z);
        if distance < LANDING_ARRIVAL_DISTANCE_SQR {
            dragon
                .phases()
                .with_instance(EnderDragonPhase::SittingFlaming, &mut |instance| {
                    instance.reset_flame_count();
                });
            dragon.set_phase(EnderDragonPhase::SittingScanning);
        }
    }

    fn fly_speed(&self) -> f32 {
        1.5
    }

    fn turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let velocity = dragon.velocity();
        let rot_speed = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt() as f32 + 1.0;
        let dist = rot_speed.min(40.0);
        dist / rot_speed
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.target_location = None;
    }

    fn fly_target(&self) -> Option<DVec3> {
        self.target_location
    }
}

/// Vanilla `DragonTakeoffPhase`.
pub struct DragonTakeoffPhase {
    first_tick: bool,
    current_path: Option<DragonPath>,
    target_location: Option<DVec3>,
}

impl DragonTakeoffPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            first_tick: true,
            current_path: None,
            target_location: None,
        }
    }

    fn find_new_target(&mut self, dragon: &EnderDragonEntity) {
        let current_node_index = dragon.find_closest_node();
        let look = dragon.head_look_vector();
        let target_node_index = dragon.find_closest_node_at(-look.x * 40.0, 105.0, -look.z * 40.0);
        let mut target_node_index = target_node_index as i32;

        if dragon.dragon_fight_present() && dragon.alive_crystals() > 0 {
            target_node_index %= 12;
            if target_node_index < 0 {
                target_node_index += 12;
            }
        } else {
            target_node_index -= 12;
            target_node_index &= 7;
            target_node_index += 12;
        }

        self.current_path =
            dragon.find_path(current_node_index, target_node_index.max(0) as usize, None);
        if let Some(path) = self.current_path.as_mut() {
            path.advance();
            if !path.is_done() {
                if let Some(current) = path.next_node_pos() {
                    path.advance();
                    let y_target = f64::from(current.y()) + f64::from(dragon.random_f32()) * 20.0;
                    self.target_location = Some(DVec3::new(
                        f64::from(current.x()),
                        y_target,
                        f64::from(current.z()),
                    ));
                }
            }
        }
    }
}

impl Default for DragonTakeoffPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonTakeoffPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::Takeoff
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        if !self.first_tick && self.current_path.is_some() {
            let egg = dragon.podium_heightmap_pos(world, HeightmapType::MotionBlockingNoLeaves);
            if block_center_distance_sqr(egg, dragon.position()).sqrt() >= TAKEOFF_PODIUM_RANGE {
                dragon.set_phase(EnderDragonPhase::HoldingPattern);
            }
        } else {
            self.first_tick = false;
            self.find_new_target(dragon);
        }
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.first_tick = true;
        self.current_path = None;
        self.target_location = None;
    }

    fn fly_target(&self) -> Option<DVec3> {
        self.target_location
    }
}

/// Vanilla `DragonSittingFlamingPhase`.
pub struct DragonSittingFlamingPhase {
    flame_ticks: i32,
    flame_count: i32,
    flame: Option<SharedEntity>,
}

impl DragonSittingFlamingPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            flame_ticks: 0,
            flame_count: 0,
            flame: None,
        }
    }

    /// Vanilla `resetFlameCount`.
    fn reset_flame_count(&mut self) {
        self.flame_count = 0;
    }

    fn spawn_flame(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        let Some(owner) = dragon.shared_handle() else {
            return;
        };
        let head = dragon.head_position();
        let position = dragon.position();
        let look = normalize_vec(DVec3::new(head.x - position.x, 0.0, head.z - position.z));
        let x = head.x + look.x * f64::from(FLAME_CLOUD_RADIUS) / 2.0;
        let z = head.z + look.z * f64::from(FLAME_CLOUD_RADIUS) / 2.0;
        let initial_y = head.y + 0.5;
        let mut y = initial_y;
        let mut pos = BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);

        while world.get_block_state(pos).is_air() {
            y -= 1.0;
            if y < 0.0 {
                y = initial_y;
                break;
            }
            pos = BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);
        }

        let y = y.floor() + 1.0;
        let cloud = AreaEffectCloudEntity::new_at_position(world, DVec3::new(x, y, z));
        cloud.set_owner(&owner);
        cloud.set_radius(FLAME_CLOUD_RADIUS);
        cloud.set_duration(FLAME_CLOUD_DURATION);
        cloud.set_custom_particle(Some(ParticleData::new(
            &vanilla_particle_types::DRAGON_BREATH,
            PowerParticleOption::new(1.0),
        )));
        cloud.set_potion_duration_scale(FLAME_CLOUD_POTION_DURATION_SCALE);
        cloud.add_effect(MobEffectInstance::simple(
            vanilla_mob_effects::INSTANT_DAMAGE,
            1,
            0,
        ));

        let cloud: SharedEntity = cloud;
        if let Err(error) = world.try_add_entity(Arc::clone(&cloud)) {
            log::debug!("failed to spawn ender dragon breath cloud: {error}");
            return;
        }
        self.flame = Some(cloud);
    }
}

impl Default for DragonSittingFlamingPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonSittingFlamingPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::SittingFlaming
    }

    fn reset_flame_count(&mut self) {
        self.reset_flame_count();
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        self.flame_ticks += 1;
        if self.flame_ticks >= FLAME_DURATION {
            if self.flame_count >= SITTING_FLAME_ATTACKS_COUNT {
                dragon.set_phase(EnderDragonPhase::Takeoff);
            } else {
                dragon.set_phase(EnderDragonPhase::SittingScanning);
            }
        } else if self.flame_ticks == WARMUP_TIME {
            self.spawn_flame(dragon, world);
        }
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.flame_ticks = 0;
        self.flame_count += 1;
    }

    fn end(&mut self, _dragon: &EnderDragonEntity) {
        if let Some(flame) = self.flame.take() {
            flame.set_removed(crate::entity::RemovalReason::Discarded);
        }
    }

    fn on_hurt(&mut self, _dragon: &EnderDragonEntity, source: &DamageSource, damage: f32) -> f32 {
        sitting_on_hurt(source, damage)
    }
}

/// Vanilla `DragonSittingScanningPhase`.
pub struct DragonSittingScanningPhase {
    scanning_time: i32,
}

impl DragonSittingScanningPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self { scanning_time: 0 }
    }
}

impl Default for DragonSittingScanningPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonSittingScanningPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::SittingScanning
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        self.scanning_time += 1;
        let dragon_position = dragon.position();
        let attack_target =
            world.nearest_player(dragon_position, SITTING_ATTACK_VIEW_RANGE, |player| {
                !player.is_spectator()
                    && dragon.can_attack(player)
                    && (player.position().y - dragon_position.y).abs()
                        <= SITTING_ATTACK_Y_VIEW_RANGE
            });

        if let Some(attack_target) = attack_target {
            if self.scanning_time > 25 {
                dragon.set_phase(EnderDragonPhase::SittingAttacking);
            } else {
                let position = dragon.position();
                let target_position = attack_target.position();
                let aim = normalize_vec(DVec3::new(
                    target_position.x - position.x,
                    0.0,
                    target_position.z - position.z,
                ));
                let yaw = dragon.rotation().0 * DEG_TO_RAD;
                let dir = normalize_vec(DVec3::new(yaw.sin() as f64, 0.0, -(yaw.cos() as f64)));
                let dot = dir.dot(aim) as f32;
                let angle = dot.acos() * RAD_TO_DEG + 0.5;
                if !(0.0..10.0).contains(&angle) {
                    let head = dragon.head_position();
                    let x_attack_dist = target_position.x - head.x;
                    let z_attack_dist = target_position.z - head.z;
                    let y_rot_delta = wrap_degrees(
                        180.0
                            - (x_attack_dist.atan2(z_attack_dist) as f32) * RAD_TO_DEG
                            - dragon.rotation().0,
                    )
                    .clamp(-100.0, 100.0);
                    let mut y_rot_a = dragon.y_rot_a() * 0.8;
                    let mut dist = ((x_attack_dist * x_attack_dist + z_attack_dist * z_attack_dist)
                        .sqrt() as f32)
                        + 1.0;
                    let rot_speed = dist;
                    if dist > 40.0 {
                        dist = 40.0;
                    }
                    y_rot_a += y_rot_delta * (0.7 / dist / rot_speed);
                    dragon.set_y_rot_a(y_rot_a);
                    dragon.set_y_rot(dragon.rotation().0 + y_rot_a);
                }
            }
        } else if self.scanning_time >= SITTING_SCANNING_IDLE_TICKS {
            let charge_target =
                world.nearest_player(dragon_position, SITTING_CHARGE_VIEW_RANGE, |player| {
                    !player.is_spectator() && dragon.can_attack(player)
                });
            dragon.set_phase(EnderDragonPhase::Takeoff);
            if let Some(charge_target) = charge_target {
                let target: SharedEntity = charge_target;
                dragon.set_phase(EnderDragonPhase::ChargingPlayer);
                dragon
                    .phases()
                    .with_instance(EnderDragonPhase::ChargingPlayer, &mut |instance| {
                        instance.set_charge_target(target.position());
                    });
            }
        }
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.scanning_time = 0;
    }

    fn on_hurt(&mut self, _dragon: &EnderDragonEntity, source: &DamageSource, damage: f32) -> f32 {
        sitting_on_hurt(source, damage)
    }
}

/// Vanilla `DragonSittingAttackingPhase`.
pub struct DragonSittingAttackingPhase {
    attacking_ticks: i32,
}

impl DragonSittingAttackingPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self { attacking_ticks: 0 }
    }
}

impl Default for DragonSittingAttackingPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonSittingAttackingPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::SittingAttacking
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, _world: &Arc<World>) {
        if self.attacking_ticks >= ROAR_DURATION {
            dragon.set_phase(EnderDragonPhase::SittingFlaming);
        }
        self.attacking_ticks += 1;
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.attacking_ticks = 0;
    }

    fn on_hurt(&mut self, _dragon: &EnderDragonEntity, source: &DamageSource, damage: f32) -> f32 {
        sitting_on_hurt(source, damage)
    }
}

/// Vanilla `DragonChargePlayerPhase`.
pub struct DragonChargePlayerPhase {
    target_location: Option<DVec3>,
    time_since_charge: i32,
}

impl DragonChargePlayerPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target_location: None,
            time_since_charge: 0,
        }
    }

    /// Vanilla `setTarget`.
    fn set_target(&mut self, target: DVec3) {
        self.target_location = Some(target);
    }
}

impl Default for DragonChargePlayerPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonChargePlayerPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::ChargingPlayer
    }

    fn set_charge_target(&mut self, target: DVec3) {
        self.set_target(target);
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, _world: &Arc<World>) {
        let Some(target) = self.target_location else {
            log::debug!("aborting ender dragon charge: no target was set");
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
            return;
        };

        if self.time_since_charge > 0 {
            let previous = self.time_since_charge;
            self.time_since_charge += 1;
            if previous >= CHARGE_RECOVERY_TIME {
                dragon.set_phase(EnderDragonPhase::HoldingPattern);
                return;
            }
        }

        let position = dragon.position();
        let distance = (target.x - position.x) * (target.x - position.x)
            + (target.y - position.y) * (target.y - position.y)
            + (target.z - position.z) * (target.z - position.z);
        if distance < RETARGET_MIN_DISTANCE_SQR
            || distance > RETARGET_MAX_DISTANCE_SQR
            || dragon.horizontal_collision()
            || dragon.vertical_collision()
        {
            self.time_since_charge += 1;
        }
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.target_location = None;
        self.time_since_charge = 0;
    }

    fn fly_speed(&self) -> f32 {
        3.0
    }

    fn fly_target(&self) -> Option<DVec3> {
        self.target_location
    }
}

/// Vanilla `DragonDeathPhase`.
pub struct DragonDeathPhase {
    target_location: Option<DVec3>,
    time: i32,
}

impl DragonDeathPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target_location: None,
            time: 0,
        }
    }
}

impl Default for DragonDeathPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonDeathPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::Dying
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        self.time += 1;
        if self.target_location.is_none() {
            let egg = dragon.podium_heightmap_pos(world, HeightmapType::MotionBlocking);
            self.target_location = Some(DVec3::new(
                f64::from(egg.x()) + 0.5,
                f64::from(egg.y()),
                f64::from(egg.z()) + 0.5,
            ));
        }

        let Some(target) = self.target_location else {
            return;
        };
        let position = dragon.position();
        let distance = (target.x - position.x) * (target.x - position.x)
            + (target.y - position.y) * (target.y - position.y)
            + (target.z - position.z) * (target.z - position.z);
        if distance >= RETARGET_MIN_DISTANCE_SQR
            && distance <= RETARGET_MAX_DISTANCE_SQR
            && !dragon.horizontal_collision()
            && !dragon.vertical_collision()
        {
            dragon.set_health(1.0);
        } else {
            dragon.set_health(0.0);
        }
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.target_location = None;
        self.time = 0;
    }

    fn fly_speed(&self) -> f32 {
        3.0
    }

    fn fly_target(&self) -> Option<DVec3> {
        self.target_location
    }
}

/// Vanilla `DragonHoverPhase`.
pub struct DragonHoverPhase {
    target_location: Option<DVec3>,
}

impl DragonHoverPhase {
    /// Creates the phase instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target_location: None,
        }
    }
}

impl Default for DragonHoverPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl DragonPhaseInstance for DragonHoverPhase {
    fn phase(&self) -> EnderDragonPhase {
        EnderDragonPhase::Hovering
    }

    fn do_server_tick(&mut self, dragon: &EnderDragonEntity, _world: &Arc<World>) {
        if self.target_location.is_none() {
            self.target_location = Some(dragon.position());
        }
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn begin(&mut self, _dragon: &EnderDragonEntity) {
        self.target_location = None;
    }

    fn fly_speed(&self) -> f32 {
        1.0
    }

    fn fly_target(&self) -> Option<DVec3> {
        self.target_location
    }
}

/// Vanilla `EnderDragonPhaseManager`.
///
/// Each instance lives in its own mutex so a phase can switch phases from inside
/// its own tick without re-entering a lock. The running instance is temporarily
/// taken out of its slot for the duration of `doServerTick`, which is what makes
/// `begin`/`end` ordering match vanilla's `setPhase`.
pub struct EnderDragonPhaseManager {
    current: SyncMutex<EnderDragonPhase>,
    instances: [SyncMutex<Option<Box<dyn DragonPhaseInstance>>>; EnderDragonPhase::COUNT],
}

impl EnderDragonPhaseManager {
    /// Creates a manager with one instance per phase.
    #[must_use]
    pub fn new() -> Self {
        Self {
            current: SyncMutex::new(EnderDragonPhase::Hovering),
            instances: std::array::from_fn(|index| {
                SyncMutex::new(Some(create_instance(EnderDragonPhase::from_id(
                    index as i32,
                ))))
            }),
        }
    }

    /// Vanilla `setPhase`.
    pub fn set_phase(&self, dragon: &EnderDragonEntity, target: EnderDragonPhase) {
        {
            let current = self.current.lock();
            if *current == target {
                return;
            }
        }

        let previous = {
            let mut current = self.current.lock();
            std::mem::replace(&mut *current, target)
        };

        // Vanilla ends the previous phase before starting the new one. When the
        // running phase switches itself, its instance is currently taken out of
        // the slot; `run_server_tick` ends it right after the tick instead.
        {
            let mut slot = self.instances[previous.index()].lock();
            if let Some(instance) = slot.as_mut() {
                instance.end(dragon);
            }
        }

        dragon.set_synced_phase(target);

        let mut slot = self.instances[target.index()].lock();
        if slot.is_none() {
            *slot = Some(create_instance(target));
        }
        if let Some(instance) = slot.as_mut() {
            instance.begin(dragon);
        }
    }

    /// Returns the current phase.
    #[must_use]
    pub fn current(&self) -> EnderDragonPhase {
        *self.current.lock()
    }

    /// Runs vanilla `currentPhase.doServerTick(level)`.
    pub fn run_server_tick(&self, dragon: &EnderDragonEntity, world: &Arc<World>) {
        let phase = dragon.phase();
        let Some(mut instance) = self.take(phase) else {
            return;
        };
        instance.do_server_tick(dragon, world);
        if dragon.phase() == phase {
            self.store(phase, instance);
        } else {
            instance.end(dragon);
        }
    }

    /// Runs `action` against the instance registered for `phase`.
    pub fn with_instance(
        &self,
        phase: EnderDragonPhase,
        action: &mut dyn FnMut(&mut dyn DragonPhaseInstance),
    ) {
        let mut slot = self.instances[phase.index()].lock();
        if slot.is_none() {
            *slot = Some(create_instance(phase));
        }
        if let Some(instance) = slot.as_mut() {
            action(instance.as_mut());
        }
    }

    /// Vanilla `getCurrentPhase().getFlyTargetLocation()`.
    #[must_use]
    pub fn fly_target(&self) -> Option<DVec3> {
        let mut target = None;
        self.with_current(&mut |instance| target = instance.fly_target());
        target
    }

    /// Vanilla `getCurrentPhase().getFlySpeed()`.
    #[must_use]
    pub fn fly_speed(&self) -> f32 {
        let mut speed = 0.6;
        self.with_current(&mut |instance| speed = instance.fly_speed());
        speed
    }

    /// Vanilla `getCurrentPhase().getTurnSpeed()`.
    #[must_use]
    pub fn turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let mut speed = 0.6;
        self.with_current(&mut |instance| speed = instance.turn_speed(dragon));
        speed
    }

    /// Vanilla `getCurrentPhase().onHurt(source, damage)`.
    pub fn on_hurt(&self, dragon: &EnderDragonEntity, source: &DamageSource, damage: f32) -> f32 {
        let mut result = damage;
        self.with_current(&mut |instance| result = instance.on_hurt(dragon, source, damage));
        result
    }

    /// Vanilla `getCurrentPhase().onCrystalDestroyed(...)`.
    pub fn on_crystal_destroyed(
        &self,
        dragon: &EnderDragonEntity,
        crystal: &SharedEntity,
        pos: BlockPos,
        source: &DamageSource,
        player: Option<&crate::player::Player>,
    ) {
        self.with_current(&mut |instance| {
            instance.on_crystal_destroyed(dragon, crystal, pos, source, player);
        });
    }

    /// Runs `action` against the current instance without changing phases.
    fn with_current(&self, action: &mut dyn FnMut(&mut dyn DragonPhaseInstance)) {
        let phase = self.current();
        let Some(mut instance) = self.take(phase) else {
            return;
        };
        action(instance.as_mut());
        self.store(phase, instance);
    }

    fn take(&self, phase: EnderDragonPhase) -> Option<Box<dyn DragonPhaseInstance>> {
        let mut slot = self.instances[phase.index()].lock();
        slot.take()
    }

    fn store(&self, phase: EnderDragonPhase, instance: Box<dyn DragonPhaseInstance>) {
        let mut slot = self.instances[phase.index()].lock();
        if slot.is_none() {
            *slot = Some(instance);
        }
    }
}

impl Default for EnderDragonPhaseManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla `EnderDragonPhase.createInstance`.
fn create_instance(phase: EnderDragonPhase) -> Box<dyn DragonPhaseInstance> {
    match phase {
        EnderDragonPhase::HoldingPattern => Box::new(DragonHoldingPatternPhase::new()),
        EnderDragonPhase::StrafePlayer => Box::new(DragonStrafePlayerPhase::new()),
        EnderDragonPhase::LandingApproach => Box::new(DragonLandingApproachPhase::new()),
        EnderDragonPhase::Landing => Box::new(DragonLandingPhase::new()),
        EnderDragonPhase::Takeoff => Box::new(DragonTakeoffPhase::new()),
        EnderDragonPhase::SittingFlaming => Box::new(DragonSittingFlamingPhase::new()),
        EnderDragonPhase::SittingScanning => Box::new(DragonSittingScanningPhase::new()),
        EnderDragonPhase::SittingAttacking => Box::new(DragonSittingAttackingPhase::new()),
        EnderDragonPhase::ChargingPlayer => Box::new(DragonChargePlayerPhase::new()),
        EnderDragonPhase::Dying => Box::new(DragonDeathPhase::new()),
        EnderDragonPhase::Hovering => Box::new(DragonHoverPhase::new()),
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

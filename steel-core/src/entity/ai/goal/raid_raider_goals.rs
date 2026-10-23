//! Raider-specific goals (vanilla `Raider` inner goal classes and `AbstractIllager`'s door
//! goal), grouped in one module because they all operate on the [`Raider`] capability and
//! are only registered by raid mobs.
//!
//! The `mod`/`pub use` lines for `entity/ai/goal/mod.rs` are owned by the iron-golem slice
//! and are reported to the integrator rather than added here.

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_registry::vanilla_entities;
use steel_utils::BlockPos;
use steel_utils::random::legacy_random::LegacyRandom;

use steel_utils::Downcast as _;

use super::open_door::OpenDoorGoal;
use super::random_pos::{HALF_PI, TENTH_PI, default_random_pos_towards};
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::ai::path::Path;
use crate::entity::entities::objects::items::ItemEntity;
use crate::entity::{Entity, ItemStack, LivingEntity, PathfinderMob, SharedEntity};
use crate::world::World;
use crate::world::village::poi::is_village_poi;

/// Vanilla `ObtainRaidLeaderBannerGoal` unreachable-banner retry delay (`gameTime + 600`).
const UNREACHABLE_BANNER_RETRY_TICKS: i64 = 600;
/// Vanilla `pursuedBannerItemEntity.closerThan(mob, 1.414)`.
const BANNER_PICKUP_DISTANCE_SQR: f64 = 1.414 * 1.414;

/// Vanilla `Raider.ObtainRaidLeaderBannerGoal`.
pub struct RaiderObtainLeaderBannerGoal {
    /// Vanilla `unreachableBannerCache`: entity id -> game time the banner may be retried.
    unreachable_banner_cache: FxHashMap<i32, i64>,
    path_to_banner: Option<Path>,
    pursued_banner: Option<SharedEntity>,
}

impl RaiderObtainLeaderBannerGoal {
    /// Creates the goal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            unreachable_banner_cache: FxHashMap::default(),
            path_to_banner: None,
            pursued_banner: None,
        }
    }

    /// Vanilla `ObtainRaidLeaderBannerGoal.cannotPickUpBanner`.
    fn cannot_pick_up_banner(mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return true;
        };
        let Some(raider) = mob.as_raider() else {
            return true;
        };
        let Some(raid) = raider.current_raid(&world) else {
            return true;
        };
        // Vanilla `!hasActiveRaid() || getCurrentRaid().isOver()`.
        if !raid.is_active() || raid.is_over() || !raider.can_be_leader() {
            return true;
        }

        let head = raider.equipment_in_slot(steel_registry::equipment::EquipmentSlot::Head);
        if ItemStack::matches(&head, &crate::entity::raid::Raid::ominous_banner_instance()) {
            return true;
        }

        // Vanilla `raid.getLeader(wave) != null && leader.isAlive()`.
        let leader = raider.current_raid_id().and_then(|raid_id| {
            world
                .raids()
                .lock()
                .get(raid_id)
                .and_then(|raid| raid.get_leader(raider.wave()))
        });
        leader.is_some_and(|leader_id| !is_entity_removed(&world, leader_id))
    }
}

/// Vanilla `leader != null && leader.isAlive()` for a raid leader kept only by UUID.
///
/// A leader that is not loaded cannot be shown to be dead, and vanilla keeps a live
/// reference to it, so a missing lookup counts as alive.
fn is_entity_removed(world: &World, uuid: uuid::Uuid) -> bool {
    world
        .get_entity_by_uuid(&uuid)
        .is_some_and(|entity| entity.is_removed())
}

impl Default for RaiderObtainLeaderBannerGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for RaiderObtainLeaderBannerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if Self::cannot_pick_up_banner(mob) {
            return false;
        }
        let Some(world) = mob.level() else {
            return false;
        };

        let follow_range = mob
            .attributes()
            .lock()
            .get_value(steel_registry::vanilla_attributes::FOLLOW_RANGE)
            .unwrap_or(16.0);
        let area = mob
            .bounding_box()
            .inflate_xyz(follow_range, 8.0, follow_range);

        let game_time = world.game_time();
        let mut temp_cache = FxHashMap::default();
        let banners = world.get_entities_in_aabb_matching(&area, |entity| {
            entity.downcast_ref::<ItemEntity>().is_some_and(|item| {
                !item.is_removed()
                    && item.get_pickup_delay() == 0
                    && ItemStack::matches(
                        &item.get_item(),
                        &crate::entity::raid::Raid::ominous_banner_instance(),
                    )
            })
        });

        for banner in banners {
            let unreachable_until = self
                .unreachable_banner_cache
                .get(&banner.id())
                .copied()
                .unwrap_or(i64::MIN);
            if game_time < unreachable_until {
                temp_cache.insert(banner.id(), unreachable_until);
                continue;
            }

            // Vanilla `navigation.createPath(banner, 1)` is the entity overload
            // (`blockPosition`, radius 16, `above = true`); Steel's pathfinder has no
            // radius-offset/above target, so this uses the plain block-position overload.
            let path = mob.create_path_to(banner.block_position(), 1);
            if let Some(path) = path
                && path.can_reach()
            {
                self.path_to_banner = Some(path);
                self.pursued_banner = Some(banner);
                return true;
            }

            temp_cache.insert(banner.id(), game_time + UNREACHABLE_BANNER_RETRY_TICKS);
        }

        self.unreachable_banner_cache = temp_cache;
        false
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(banner) = &self.pursued_banner else {
            return false;
        };
        if self.path_to_banner.is_none() {
            return false;
        }
        if banner.is_removed() {
            return false;
        }
        // Vanilla keeps the live `Path` instance and checks `pathToBanner.isDone()`; Steel's
        // navigation owns the path after `start`, so "still pathfinding" is the equivalent.
        mob.is_path_finding() && !Self::cannot_pick_up_banner(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.move_to_path(self.path_to_banner.clone(), 1.15);
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.path_to_banner = None;
        self.pursued_banner = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(banner) = &self.pursued_banner else {
            return;
        };
        // Vanilla `pursuedBannerItemEntity.closerThan(mob, 1.414)`.
        if banner.position().distance_squared(mob.position()) >= BANNER_PICKUP_DISTANCE_SQR {
            return;
        }
        let Some(world) = mob.level() else {
            return;
        };
        let Some(raider) = mob.as_raider() else {
            return;
        };
        if let Some(item) = banner.downcast_ref::<ItemEntity>() {
            raider.pick_up_item_raider(&world, item);
        }
    }
}

/// Vanilla `Raider.RaiderCelebration`.
pub struct RaiderCelebrationGoal;

impl RaiderCelebrationGoal {
    /// Creates the goal.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for RaiderCelebrationGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for RaiderCelebrationGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        LivingEntity::is_alive(mob)
            && mob.target().is_none()
            && raider
                .current_raid(&world)
                .is_some_and(|raid| raid.is_loss())
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(raider) = mob.as_raider() {
            raider.set_celebrating(true);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(raider) = mob.as_raider() {
            raider.set_celebrating(false);
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(raider) = mob.as_raider() else {
            return;
        };
        if !mob.is_silent() && rand::random_range(0..reduced_tick_delay(100)) == 0 {
            mob.make_sound(Some(raider.celebrate_sound()));
        }
        if !mob.is_passenger() && rand::random_range(0..reduced_tick_delay(50)) == 0 {
            mob.jump_control_jump();
        }
    }
}

/// Vanilla `Raider.RaiderMoveThroughVillageGoal`.
pub struct RaiderMoveThroughVillageGoal {
    speed_modifier: f64,
    distance_to_poi: i32,
    poi_pos: Option<BlockPos>,
    visited: Vec<BlockPos>,
    stuck: bool,
}

impl RaiderMoveThroughVillageGoal {
    /// Vanilla `RaiderMoveThroughVillageGoal(raider, speed, distanceToPoi)`.
    #[must_use]
    pub const fn new(speed_modifier: f64, distance_to_poi: i32) -> Self {
        Self {
            speed_modifier,
            distance_to_poi,
            poi_pos: None,
            visited: Vec::new(),
            stuck: false,
        }
    }

    fn has_suitable_poi(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        let pos = mob.block_position();
        let mut random = LegacyRandom::from_seed(rand::random());
        let visited = self.visited.clone();

        let found = world.poi_storage.lock().get_random(
            &|poi_type| poi_type.key == steel_registry::vanilla_poi_types::HOME.key,
            &|candidate| !visited.contains(&candidate),
            pos,
            48,
            crate::poi::OccupationStatus::Any,
            &mut random,
        );

        let Some(poi) = found else {
            return false;
        };
        self.poi_pos = Some(poi);
        true
    }

    fn is_valid_raid(mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        mob.as_raider().is_some_and(|raider| {
            raider
                .current_raid(&world)
                .is_some_and(|raid| raid.is_active() && !raid.is_over())
        })
    }
}

impl Goal for RaiderMoveThroughVillageGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.visited.len() > 2 {
            self.visited.remove(0);
        }
        Self::is_valid_raid(mob) && self.has_suitable_poi(mob) && mob.target().is_none()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if !mob.is_path_finding() {
            return false;
        }
        let Some(poi) = self.poi_pos else {
            return false;
        };
        // Vanilla `poiPos.closerToCenterThan(position, getBbWidth() + distanceToPoi)`.
        let reach = mob.bounding_box().width() + f64::from(self.distance_to_poi);
        mob.target().is_none() && !self.stuck && !poi.closer_to_center_than(mob.position(), reach)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(poi) = self.poi_pos {
            mob.move_to_pos(
                DVec3::new(
                    f64::from(poi.x()) + 0.5,
                    f64::from(poi.y()),
                    f64::from(poi.z()) + 0.5,
                ),
                self.speed_modifier,
            );
        }
        mob.set_no_action_time(0);
        self.stuck = false;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(poi) = self.poi_pos
            && poi.closer_to_center_than(mob.position(), f64::from(self.distance_to_poi))
        {
            self.visited.push(poi);
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        // Vanilla only re-picks a random position once the current path is finished.
        if mob.is_path_finding() {
            return;
        }
        let Some(poi) = self.poi_pos else {
            return;
        };
        let towards = DVec3::new(
            f64::from(poi.x()) + 0.5,
            f64::from(poi.y()),
            f64::from(poi.z()) + 0.5,
        );
        let next = default_random_pos_towards(mob, 16, 7, towards, TENTH_PI)
            .or_else(|| default_random_pos_towards(mob, 8, 7, towards, HALF_PI));

        let Some(next) = next else {
            self.stuck = true;
            return;
        };
        mob.move_to_pos(next, self.speed_modifier);
    }
}

/// Vanilla `AbstractIllager.RaiderOpenDoorGoal`: `OpenDoorGoal(raider, false)` that only
/// opens doors while the raider has an active raid.
pub struct RaiderOpenDoorGoal {
    inner: OpenDoorGoal,
}

impl RaiderOpenDoorGoal {
    /// Creates the goal.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: OpenDoorGoal::new(false),
        }
    }
}

impl Default for RaiderOpenDoorGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for RaiderOpenDoorGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_use(mob)
            && mob.level().is_some_and(|world| {
                mob.as_raider()
                    .is_some_and(|raider| raider.has_active_raid(&world))
            })
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Vanilla `Raider.ALLOWED_ITEMS` check used by the banner goal and `pickUpItem`.
#[must_use]
pub fn is_ominous_banner(item_stack: &ItemStack) -> bool {
    ItemStack::matches(
        item_stack,
        &crate::entity::raid::Raid::ominous_banner_instance(),
    )
}

/// The village POI predicate the raider move-through-village goal uses for homes.
#[must_use]
pub fn is_raider_walk_poi(poi_type: steel_registry::poi::PoiTypeRef) -> bool {
    is_village_poi(poi_type)
}

/// Iron golem and villager types an illager may target, for reuse by target goals.
#[must_use]
pub const fn targetable_golem() -> steel_registry::entity_type::EntityTypeRef {
    &vanilla_entities::IRON_GOLEM
}

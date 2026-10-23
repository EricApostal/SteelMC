//! Vanilla `Raider`: the capability shared by illagers, ravagers and witches.
//!
//! Vanilla implements this state in the abstract `Raider` class on top of
//! `PatrollingMonster`. Steel has neither a `Raider` class nor a patrol base, so the state
//! lives in [`RaiderBase`] and the behaviour in the [`Raider`] capability trait, which
//! every raid mob implements alongside [`Monster`](crate::entity::Monster).

use std::sync::Arc;

use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_protocol::packets::game::CTakeItemEntity;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entities;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, ChunkPos};
use uuid::Uuid;

use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::items::ItemEntity;
use crate::entity::{Entity, LivingEntity, Mob, RemovalReason, SharedEntity};
use crate::world::World;

use super::raid::{MAX_NO_ACTION_TIME, Raid};
use super::raids::RaidSnapshot;

/// Vanilla `Raider` fields plus the `PatrollingMonster` fields it inherits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RaiderState {
    /// Vanilla `Raider.raid`, saved as `RaidId`.
    pub raid_id: Option<i32>,
    /// Vanilla `Raider.wave`: the raid wave the raider belongs to.
    pub wave: i32,
    /// Vanilla `Raider.canJoinRaid`: whether the raider may still join a raid wave.
    pub can_join_raid: bool,
    /// Vanilla `Raider.ticksOutsideRaid`: ticks spent outside the raid bounds.
    pub ticks_outside_raid: i32,
    /// Vanilla `PatrollingMonster.patrolTarget`: the village patrol destination.
    pub patrol_target: Option<BlockPos>,
    /// Vanilla `PatrollingMonster.isPatrolLeader`: whether the raider leads a patrol.
    pub patrol_leader: bool,
    /// Vanilla `PatrollingMonster.patrolling`: whether the raider has an active patrol.
    pub patrolling: bool,
}

/// Shared raider state storage, mirroring how [`MobBase`](crate::entity::MobBase) stores
/// state shared by every mob.
#[derive(Debug, Default)]
pub struct RaiderBase {
    state: SyncMutex<RaiderState>,
}

impl RaiderBase {
    /// Creates empty raider state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Locks the raider state for reading or writing.
    pub fn state(&self) -> parking_lot::MutexGuard<'_, RaiderState> {
        self.state.lock()
    }

    /// Vanilla `Raider.addAdditionalSaveData` plus `PatrollingMonster.addAdditionalSaveData`.
    pub fn save(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("Wave", state.wave);
        nbt.insert("CanJoinRaid", i8::from(state.can_join_raid));
        if let Some(raid_id) = state.raid_id {
            nbt.insert("RaidId", raid_id);
        }
        if let Some(target) = state.patrol_target {
            nbt.insert(
                "patrol_target",
                NbtTag::IntArray(vec![target.x(), target.y(), target.z()]),
            );
        }
        nbt.insert("PatrolLeader", i8::from(state.patrol_leader));
        nbt.insert("Patrolling", i8::from(state.patrolling));
    }

    /// Vanilla `Raider.readAdditionalSaveData` plus `PatrollingMonster.readAdditionalSaveData`.
    ///
    /// Returns the saved raid id so the caller can re-register the mob with that raid,
    /// exactly like vanilla's `readAdditionalSaveData`.
    pub fn load(&self, nbt: &BorrowedNbtCompoundView<'_, '_>) -> Option<i32> {
        let mut state = self.state.lock();
        state.wave = nbt.int("Wave").unwrap_or(0);
        state.can_join_raid = nbt.byte("CanJoinRaid").is_some_and(|value| value != 0);
        state.ticks_outside_raid = 0;
        state.patrol_target = nbt
            .int_array("patrol_target")
            .filter(|array| array.len() == 3)
            .map(|array| BlockPos::new(array[0], array[1], array[2]));
        state.patrol_leader = nbt.byte("PatrolLeader").is_some_and(|value| value != 0);
        state.patrolling = nbt.byte("Patrolling").is_some_and(|value| value != 0);
        let raid_id = nbt.int("RaidId");
        state.raid_id = raid_id;
        raid_id
    }
}

/// Vanilla `Raider`, plus the `PatrollingMonster` members it inherits.
pub trait Raider: Mob {
    /// Shared raider state.
    fn raider_base(&self) -> &RaiderBase;

    /// Vanilla `Raider.IS_CELEBRATING`.
    fn is_celebrating(&self) -> bool;

    /// Vanilla `Raider.setCelebrating`.
    fn set_celebrating(&self, celebrating: bool);

    /// Vanilla `Raider.applyRaidBuffs`.
    ///
    /// The raid snapshot replaces vanilla's `getCurrentRaid()` lookup because the raid
    /// manager lock is already held while a wave spawns.
    fn apply_raid_buffs(
        &self,
        world: &Arc<World>,
        raid: &RaidSnapshot,
        wave: i32,
        is_captain: bool,
    );

    /// Vanilla `Raider.getCelebrateSound`.
    fn celebrate_sound(&self) -> SoundEventRef;

    /// Vanilla `Raider.canJoinRaid`.
    fn can_join_raid(&self) -> bool {
        self.raider_base().state().can_join_raid
    }

    /// Vanilla `Raider.setCanJoinRaid`.
    fn set_can_join_raid(&self, can_join_raid: bool) {
        self.raider_base().state().can_join_raid = can_join_raid;
    }

    /// Vanilla `PatrollingMonster.canBeLeader`.
    fn can_be_leader(&self) -> bool {
        true
    }

    /// Vanilla `Raider.canJoinPatrol`.
    fn can_join_patrol(&self, world: &World) -> bool {
        !self.has_active_raid(world)
    }

    /// Vanilla `Raider.getCurrentRaid`'s id, without a world lookup.
    fn current_raid_id(&self) -> Option<i32> {
        self.raider_base().state().raid_id
    }

    /// Vanilla `Raider.setCurrentRaid`.
    fn set_current_raid(&self, raid_id: Option<i32>) {
        self.raider_base().state().raid_id = raid_id;
    }

    /// Vanilla `Raider.setWave`.
    fn set_wave(&self, wave: i32) {
        self.raider_base().state().wave = wave;
    }

    /// Vanilla `Raider.getWave`.
    fn wave(&self) -> i32 {
        self.raider_base().state().wave
    }

    /// Vanilla `Raider.setTicksOutsideRaid`.
    fn set_ticks_outside_raid(&self, ticks_outside_raid: i32) {
        self.raider_base().state().ticks_outside_raid = ticks_outside_raid;
    }

    /// Vanilla `Raider.getTicksOutsideRaid`.
    fn ticks_outside_raid(&self) -> i32 {
        self.raider_base().state().ticks_outside_raid
    }

    /// Vanilla `PatrollingMonster.getPatrolTarget`.
    fn patrol_target(&self) -> Option<BlockPos> {
        self.raider_base().state().patrol_target
    }

    /// Vanilla `PatrollingMonster.hasPatrolTarget`.
    fn has_patrol_target(&self) -> bool {
        self.patrol_target().is_some()
    }

    /// Vanilla `PatrollingMonster.setPatrolTarget`.
    fn set_patrol_target(&self, target: BlockPos) {
        let mut state = self.raider_base().state();
        state.patrol_target = Some(target);
        state.patrolling = true;
    }

    /// Vanilla `PatrollingMonster.isPatrolLeader`.
    fn is_patrol_leader(&self) -> bool {
        self.raider_base().state().patrol_leader
    }

    /// Vanilla `PatrollingMonster.setPatrolLeader`.
    fn set_patrol_leader(&self, leader: bool) {
        let mut state = self.raider_base().state();
        state.patrol_leader = leader;
        state.patrolling = true;
    }

    /// Vanilla `PatrollingMonster.isPatrolling`.
    fn is_patrolling(&self) -> bool {
        self.raider_base().state().patrolling
    }

    /// Vanilla `PatrollingMonster.setPatrolling`.
    fn set_patrolling(&self, patrolling: bool) {
        self.raider_base().state().patrolling = patrolling;
    }

    /// Vanilla `PatrollingMonster.findPatrolTarget`.
    fn find_patrol_target(&self) {
        let pos = self.block_position();
        let offset_x = rand::random_range(-500..1000);
        let offset_z = rand::random_range(-500..1000);
        self.set_patrol_target(pos.offset(offset_x, 0, offset_z));
    }

    /// This raider as the shared entity handle the raid bookkeeping keeps.
    fn raider_entity(&self) -> Option<SharedEntity> {
        let world = self.level()?;
        world.get_entity_by_uuid(&self.uuid())
    }

    /// Vanilla `Raider.getCurrentRaid`, as a read-only snapshot.
    fn current_raid(&self, world: &World) -> Option<RaidSnapshot> {
        let raid_id = self.raider_base().state().raid_id?;
        world.raids().lock().snapshot(raid_id)
    }

    /// Vanilla `Raider.hasRaid`.
    fn has_raid(&self, world: &World) -> bool {
        self.current_raid(world).is_some() || world.get_raid_at(self.block_position()).is_some()
    }

    /// Vanilla `Raider.hasActiveRaid`.
    fn has_active_raid(&self, world: &World) -> bool {
        self.current_raid(world)
            .is_some_and(|raid| raid.is_active())
    }

    /// Vanilla `Raids.canJoinRaid` for this raider.
    fn is_raid_eligible(&self) -> bool {
        LivingEntity::is_alive(self)
            && self.can_join_raid()
            && LivingEntity::no_action_time(self) <= MAX_NO_ACTION_TIME
    }

    /// Vanilla `Raider.isCaptain`: an ominous banner worn as a patrol leader.
    fn is_captain(&self) -> bool {
        let head = self.equipment_in_slot(EquipmentSlot::Head);
        !head.is_empty()
            && ItemStack::matches(&head, &Raid::ominous_banner_instance())
            && self.is_patrol_leader()
    }

    /// Vanilla `Raider.aiStep` raid bookkeeping; runs before the mob AI step.
    fn raider_ai_step(&self, entity: &SharedEntity) {
        if !LivingEntity::is_alive(self) || !self.can_join_raid() {
            return;
        }

        let Some(world) = self.level() else {
            return;
        };

        if self.raider_base().state().raid_id.is_none() {
            // Vanilla looks for a raid once a second and needs the raider to be eligible.
            if world.game_time() % 20 != 0 || !self.is_raid_eligible() {
                return;
            }
            let Some(raid) = world.get_raid_at(self.block_position()) else {
                return;
            };
            world.raids().lock().join_raid(
                &world,
                raid.id,
                entity,
                raid.groups_spawned,
                None,
                true,
            );
            return;
        }

        // A raider with a player or golem target never counts as idle.
        let raid_target = self.target().is_some_and(|target| {
            target.entity_type() == &vanilla_entities::PLAYER
                || target.entity_type() == &vanilla_entities::IRON_GOLEM
        });
        if raid_target {
            self.set_no_action_time(0);
        }
    }

    /// Vanilla `Raider.die` raid cleanup; runs from the death hook before the shared death
    /// processing.
    fn raider_on_death(&self, source: &DamageSource) {
        let Some(raid_id) = self.raider_base().state().raid_id else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };

        let killer_is_player = source
            .causing_entity()
            .is_some_and(|killer| killer.as_player().is_some());
        let uuid = self.uuid();
        let wave = self.wave();
        let patrol_leader = self.is_patrol_leader();

        {
            let mut raids = world.raids().lock();
            if patrol_leader {
                raids.remove_leader(raid_id, wave);
            }
            if killer_is_player {
                raids.add_hero_of_the_village(raid_id, uuid);
            }
            raids.remove_raider(&world, raid_id, uuid, false);
        }
        self.set_current_raid(None);
    }

    /// Vanilla `Raider.pickUpItem`: take the ominous banner and become the wave leader.
    ///
    /// Returns `false` when the pickup is not a raid-leader banner pickup, so callers fall
    /// back to the regular mob pickup.
    fn pick_up_item_raider(&self, world: &Arc<World>, item_entity: &ItemEntity) -> bool {
        let item_stack = item_entity.get_item();
        let wave = self.wave();
        let has_leader = self.current_raid_id().is_some_and(|raid_id| {
            world
                .raids()
                .lock()
                .get(raid_id)
                .is_some_and(|raid| raid.has_leader(wave))
        });

        if !self.has_active_raid(world)
            || has_leader
            || !ItemStack::matches(&item_stack, &Raid::ominous_banner_instance())
        {
            return false;
        }

        // Vanilla rolls the replaced headgear against the equipment drop chance.
        let slot = EquipmentSlot::Head;
        let current = self.equipment_in_slot(slot);
        let drop_chance = self.equipment_drop_chance(slot);
        if !current.is_empty() && (rand::random::<f32>() - 0.1).max(0.0) < drop_chance {
            self.spawn_at_location(current, 0.0);
        }

        self.living_base()
            .equipment()
            .lock()
            .set(slot, item_stack.clone());
        self.set_patrol_leader(true);

        let count = item_stack.count();
        item_entity.set_item(ItemStack::empty());
        item_entity.set_removed(RemovalReason::Discarded);
        world.broadcast_to_nearby(
            ChunkPos::from_entity_pos(item_entity.position()),
            CTakeItemEntity::new(item_entity.id(), self.id(), count),
            None,
        );

        if let (Some(raid_id), Some(entity)) =
            (self.raider_base().state().raid_id, self.raider_entity())
        {
            world.raids().lock().set_leader(raid_id, wave, &entity);
        }
        true
    }

    /// Vanilla `Raider.readAdditionalSaveData`: re-register this raider with its saved raid.
    fn rejoin_saved_raid(&self, raid_id: i32) {
        let Some(world) = self.level() else {
            return;
        };
        world
            .raids()
            .lock()
            .add_saved_raider(raid_id, self.wave(), &world, self.raider_entity());
    }

    /// Vanilla `Raider.requiresCustomPersistence`.
    fn raider_requires_custom_persistence(&self) -> bool {
        self.raider_base().state().raid_id.is_some()
    }

    /// Vanilla `Raider.removeWhenFarAway`.
    fn raider_remove_when_far_away(&self, dist_sqr: f64) -> bool {
        if self.raider_base().state().raid_id.is_some() {
            return false;
        }
        self.remove_when_far_away(dist_sqr)
    }
}

/// Vanilla `Raider.ALLOWED_ITEMS`: a dropped ominous banner, ready to be picked up.
#[must_use]
pub fn raider_allowed_item(item_stack: &ItemStack) -> bool {
    ItemStack::matches(item_stack, &Raid::ominous_banner_instance())
}

/// Vanilla `Raider` death helper used by the raid manager to drop a raider from its wave.
pub fn raider_uuid_of(entity: &SharedEntity) -> Uuid {
    entity.uuid()
}

/// Vanilla `Pillager.applyRaidBuffs` provider gate.
///
/// Vanilla compares the wave against `Raid.getNumGroups(NORMAL)` (5) and
/// `Raid.getNumGroups(EASY)` (3); below both, vanilla leaves the crossbow unbuffed.
#[must_use]
pub fn pillager_raid_enchant_provider(
    wave: i32,
) -> Option<&'static steel_registry::enchantment::provider::EnchantmentProvider> {
    let normal_groups = Raid::num_groups(steel_utils::types::Difficulty::Normal);
    let easy_groups = Raid::num_groups(steel_utils::types::Difficulty::Easy);

    if wave > normal_groups {
        Some(steel_registry::enchantment::provider::raid_pillager_post_wave_5())
    } else if wave > easy_groups {
        Some(steel_registry::enchantment::provider::raid_pillager_post_wave_3())
    } else {
        None
    }
}

/// Vanilla `Vindicator.applyRaidBuffs` provider gate.
///
/// Vanilla always enchants the iron axe: post-wave-5 past `Raid.getNumGroups(NORMAL)`,
/// otherwise the base `RAID_VINDICATOR` provider.
#[must_use]
pub fn vindicator_raid_enchant_provider(
    wave: i32,
) -> &'static steel_registry::enchantment::provider::EnchantmentProvider {
    if wave > Raid::num_groups(steel_utils::types::Difficulty::Normal) {
        steel_registry::enchantment::provider::raid_vindicator_post_wave_5()
    } else {
        steel_registry::enchantment::provider::raid_vindicator()
    }
}

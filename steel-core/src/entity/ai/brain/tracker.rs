//! Vanilla `PositionTracker` implementors and the `WalkTarget` memory value.

use std::sync::Arc;

use glam::DVec3;
use steel_utils::BlockPos;

use super::memory::MemoryStore;
use super::memory::module_type::NEAREST_VISIBLE_LIVING_ENTITIES;
use crate::entity::{LivingEntity, SharedEntity};

/// Vanilla `PositionTracker`.
pub trait PositionTracker: Send + Sync {
    fn current_position(&self) -> DVec3;

    fn current_block_position(&self) -> BlockPos;

    /// Vanilla tests visibility through the body's `NEAREST_VISIBLE_LIVING_ENTITIES`
    /// memory; Steel passes the memory store instead of reaching back into the entity.
    fn is_visible_by(&self, memories: &MemoryStore) -> bool;
}

/// Vanilla `BlockPosTracker`.
pub struct BlockPosTracker {
    block_pos: BlockPos,
    center_position: DVec3,
}

impl BlockPosTracker {
    #[must_use]
    pub fn from_block_pos(block_pos: BlockPos) -> Self {
        let (x, y, z) = block_pos.get_center();
        Self {
            block_pos,
            center_position: DVec3::new(x, y, z),
        }
    }

    #[must_use]
    pub fn from_vec(position: DVec3) -> Self {
        Self {
            block_pos: BlockPos::containing(position.x, position.y, position.z),
            center_position: position,
        }
    }
}

impl PositionTracker for BlockPosTracker {
    fn current_position(&self) -> DVec3 {
        self.center_position
    }

    fn current_block_position(&self) -> BlockPos {
        self.block_pos
    }

    fn is_visible_by(&self, _memories: &MemoryStore) -> bool {
        true
    }
}

/// Vanilla `EntityTracker`.
pub struct EntityTracker {
    entity: SharedEntity,
    track_eye_height: bool,
    target_eye_height: bool,
}

impl EntityTracker {
    #[must_use]
    pub fn new(entity: SharedEntity, track_eye_height: bool) -> Self {
        Self {
            entity,
            track_eye_height,
            target_eye_height: false,
        }
    }

    #[must_use]
    pub fn with_target_eye_height(
        entity: SharedEntity,
        track_eye_height: bool,
        target_eye_height: bool,
    ) -> Self {
        Self {
            entity,
            track_eye_height,
            target_eye_height,
        }
    }

    #[must_use]
    pub fn entity(&self) -> &SharedEntity {
        &self.entity
    }
}

impl PositionTracker for EntityTracker {
    fn current_position(&self) -> DVec3 {
        let position = self.entity.position();
        if self.track_eye_height {
            DVec3::new(position.x, self.entity.get_eye_y(), position.z)
        } else {
            position
        }
    }

    fn current_block_position(&self) -> BlockPos {
        if self.target_eye_height {
            let eye = DVec3::new(
                self.entity.position().x,
                self.entity.get_eye_y(),
                self.entity.position().z,
            );
            BlockPos::containing(eye.x, eye.y, eye.z)
        } else {
            self.entity.block_position()
        }
    }

    fn is_visible_by(&self, memories: &MemoryStore) -> bool {
        let Some(living) = self.entity.as_living_entity() else {
            return true;
        };

        if !LivingEntity::is_alive(living) {
            return false;
        }

        memories
            .get(&NEAREST_VISIBLE_LIVING_ENTITIES)
            .is_some_and(|visible| visible.contains(&self.entity))
    }
}

/// Vanilla `WalkTarget`.
#[derive(Clone)]
pub struct WalkTarget {
    target: Arc<dyn PositionTracker>,
    speed_modifier: f32,
    close_enough_dist: i32,
}

impl WalkTarget {
    #[must_use]
    pub fn new(
        target: Arc<dyn PositionTracker>,
        speed_modifier: f32,
        close_enough_dist: i32,
    ) -> Self {
        Self {
            target,
            speed_modifier,
            close_enough_dist,
        }
    }

    #[must_use]
    pub fn from_block_pos(
        block_pos: BlockPos,
        speed_modifier: f32,
        close_enough_dist: i32,
    ) -> Self {
        Self::new(
            Arc::new(BlockPosTracker::from_block_pos(block_pos)),
            speed_modifier,
            close_enough_dist,
        )
    }

    #[must_use]
    pub fn from_vec(position: DVec3, speed_modifier: f32, close_enough_dist: i32) -> Self {
        Self::new(
            Arc::new(BlockPosTracker::from_vec(position)),
            speed_modifier,
            close_enough_dist,
        )
    }

    #[must_use]
    pub fn from_entity(entity: SharedEntity, speed_modifier: f32, close_enough_dist: i32) -> Self {
        Self::new(
            Arc::new(EntityTracker::new(entity, false)),
            speed_modifier,
            close_enough_dist,
        )
    }

    #[must_use]
    pub fn target(&self) -> &Arc<dyn PositionTracker> {
        &self.target
    }

    #[must_use]
    pub fn speed_modifier(&self) -> f32 {
        self.speed_modifier
    }

    #[must_use]
    pub fn close_enough_dist(&self) -> i32 {
        self.close_enough_dist
    }
}

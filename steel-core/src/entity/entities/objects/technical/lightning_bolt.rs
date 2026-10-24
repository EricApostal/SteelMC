//! `LightningBolt` entity (`net.minecraft.world.entity.LightningBolt`).
//!
//! Ports the vanilla lifecycle: two-tick strike window, random flash count, fire ignition and the
//! entity damage sweep. Strike effects that belong to the level (rod powering, copper cleaning,
//! game event) are dispatched to [`crate::world::World::lightning_bolt_strike_effects`].

use std::sync::{Arc, Weak};

use uuid::Uuid;

use glam::DVec3;
use steel_macros::entity_behavior;
use steel_registry::blocks::behavior::PushReaction;
use steel_registry::entity_type::EntityTypeRef;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey, WorldAabb};

use crate::entity::{Entity, EntityBase, EntityBaseLoad, RemovalReason, SharedEntity};
use crate::world::World;

/// Vanilla `LightningBolt.START_LIFE`.
const START_LIFE: i32 = 2;

/// Vanilla `LightningBolt.DAMAGE_RADIUS`.
const DAMAGE_RADIUS: f64 = 3.0;

/// Mutable vanilla `LightningBolt` fields.
#[derive(Debug)]
struct LightningBoltState {
    life: i32,
    seed: i64,
    flashes: i32,
    visual_only: bool,
    /// Vanilla `LightningBolt.cause`: the player whose channeling trident produced this bolt.
    cause: Option<Uuid>,
}

/// A lightning bolt: visual strike plus server-side damage and fire.
#[entity_behavior(class = "LightningBolt")]
pub struct LightningBoltEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    state: SyncMutex<LightningBoltState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `LightningBoltEntity`.
unsafe impl DowncastType for LightningBoltEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/lightning_bolt");
}

impl LightningBoltEntity {
    /// Creates a new lightning bolt with vanilla's random `seed` and `flashes`.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            state: SyncMutex::new(LightningBoltState {
                life: START_LIFE,
                seed: rand::random(),
                flashes: rand::random_range(0..3) + 1,
                visual_only: false,
                cause: None,
            }),
        }
    }

    /// Creates a lightning bolt from saved data.
    ///
    /// Vanilla persists no bolt fields, so the saved entity only restores its base state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            state: SyncMutex::new(LightningBoltState {
                life: START_LIFE,
                seed: rand::random(),
                flashes: rand::random_range(0..3) + 1,
                visual_only: false,
                cause: None,
            }),
        }
    }

    /// Vanilla `LightningBolt.setVisualOnly`: suppresses damage and fire, keeping the flash.
    pub fn set_visual_only(&self, visual_only: bool) {
        self.state.lock().visual_only = visual_only;
    }

    /// Returns whether this bolt is a visual-only trap strike.
    #[must_use]
    pub fn is_visual_only(&self) -> bool {
        self.state.lock().visual_only
    }

    /// Vanilla `LightningBolt.setCause`: records the player whose channeling trident struck.
    pub fn set_cause(&self, cause: Option<Uuid>) {
        self.state.lock().cause = cause;
    }

    /// Vanilla `LightningBolt.getCause`.
    #[must_use]
    pub fn cause(&self) -> Option<Uuid> {
        self.state.lock().cause
    }
}

impl Entity for LightningBoltEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        // Vanilla `LightningBolt.tick` calls `super.tick()` (vanilla `Entity.baseTick`).
        self.default_tick();
        let Some(world) = self.level() else {
            return;
        };

        let (life, visual_only, flashes, mut seed) = {
            let state = self.state.lock();
            (state.life, state.visual_only, state.flashes, state.seed)
        };

        if life == START_LIFE {
            world.lightning_bolt_strike_effects(self.block_position(), visual_only);
        }

        let mut next_life = life - 1;
        let mut next_flashes = flashes;
        if next_life < 0 {
            if next_flashes == 0 {
                self.set_removed(RemovalReason::Discarded);
                return;
            }

            if next_life < -rand::random_range(0..10) {
                next_flashes -= 1;
                next_life = 1;
                seed = rand::random();
                world.lightning_spawn_fire(self.block_position(), 0);
            }
        }

        {
            let mut state = self.state.lock();
            state.life = next_life;
            state.flashes = next_flashes;
            state.seed = seed;
        }

        if next_life < 0 || visual_only {
            return;
        }

        let position = self.position();
        let search = WorldAabb::new(
            position.x - DAMAGE_RADIUS,
            position.y - DAMAGE_RADIUS,
            position.z - DAMAGE_RADIUS,
            position.x + DAMAGE_RADIUS,
            position.y + 6.0 + DAMAGE_RADIUS,
            position.z + DAMAGE_RADIUS,
        );
        let bolt: SharedEntity = Arc::clone(&self) as SharedEntity;
        let hit = world.get_entities_in_aabb_matching(&search, Entity::is_alive);
        for entity in hit {
            if entity.id() == self.id() {
                continue;
            }
            entity.thunder_hit(&world, &bolt);
        }
    }

    fn attackable(&self) -> bool {
        false
    }

    fn is_pushable(&self) -> bool {
        false
    }

    fn piston_push_reaction(&self) -> PushReaction {
        PushReaction::IgnoreEntity
    }

    fn can_use_portal(&self, _ignore_passenger: bool) -> bool {
        false
    }

    fn blocks_building(&self) -> bool {
        false
    }

    fn is_pickable(&self) -> bool {
        false
    }
}

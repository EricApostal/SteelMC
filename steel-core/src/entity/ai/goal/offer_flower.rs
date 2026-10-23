//! Vanilla `OfferFlowerGoal` (`goal/OfferFlowerGoal.java`).
//!
//! The iron golem occasionally offers a poppy to a nearby villager (or any mob in
//! `#minecraft:accepts_iron_golem_gift`), and hands the poppy over when the offer completes.

use steel_registry::TaggedRegistryExt as _;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_items;
use steel_utils::Downcast as _;

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::entities::mobs::golem::IronGolemEntity;
use crate::entity::{PathfinderMob, SharedEntity};
use crate::world::World;

/// Vanilla `OfferFlowerGoal.OFFER_TICKS`.
pub const OFFER_TICKS: i32 = 400;
const OFFER_RANGE: f64 = 6.0;
const OFFER_PROBABILITY: i32 = 8000;
const OFFER_BOX_XZ: f64 = 6.0;
const OFFER_BOX_Y: f64 = 2.0;
/// Vanilla `OfferFlowerGoal.tick`: `setLookAt(entity, 30.0F, 30.0F)`.
const OFFER_LOOK_Y_MAX_ROT_SPEED: f32 = 30.0;
const OFFER_LOOK_X_MAX_ROT_ANGLE: f32 = 30.0;

pub struct OfferFlowerGoal {
    entity: Option<SharedEntity>,
    tick: i32,
    target_context: TargetingConditions,
}

impl OfferFlowerGoal {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            entity: None,
            tick: 0,
            target_context: TargetingConditions::for_non_combat().range(OFFER_RANGE),
        }
    }

    fn golem(mob: &dyn PathfinderMob) -> Option<&IronGolemEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<IronGolemEntity>()
    }

    fn golem_bounding_box(mob: &dyn PathfinderMob) -> steel_utils::WorldAabb {
        mob.bounding_box()
            .inflate_xyz(OFFER_BOX_XZ, OFFER_BOX_Y, OFFER_BOX_XZ)
    }

    /// Vanilla `OfferFlowerGoal.stop` gift transfer for `#minecraft:accepts_iron_golem_gift`.
    fn give_gift(&self, mob: &dyn PathfinderMob) {
        let Some(entity) = &self.entity else {
            return;
        };
        let Some(target) = entity.as_mob() else {
            return;
        };
        if !steel_registry::REGISTRY.entity_types.is_in_tag(
            entity.entity_type(),
            &EntityTypeTag::ACCEPTS_IRON_GOLEM_GIFT,
        ) {
            return;
        }

        // Vanilla `CopperGolem.EQUIPMENT_SLOT_ANTENNA` is the saddle slot.
        let slot = EquipmentSlot::Saddle;
        let mut empty = false;
        target.with_equipment_slot(slot, &mut |stack| empty = stack.is_empty());
        if !empty || !Self::golem_bounding_box(mob).intersects(entity.bounding_box()) {
            return;
        }

        target.with_equipment_slot_mut(slot, &mut |stack| {
            *stack = ItemStack::new(&vanilla_items::POPPY);
        });
        target.set_guaranteed_drop(slot);
    }
}

impl Goal for OfferFlowerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        if !world.is_bright_outside() {
            return false;
        }
        if rand::random_range(0..OFFER_PROBABILITY) != 0 {
            return false;
        }

        self.entity = nearest_gift_candidate(mob, world.as_ref(), &self.target_context);
        self.entity.is_some()
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        self.tick > 0
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.tick = reduced_tick_delay(OFFER_TICKS);
        if let Some(golem) = Self::golem(mob) {
            golem.offer_flower(true);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(golem) = Self::golem(mob) {
            golem.offer_flower(false);
        }
        if self.tick == 0 {
            self.give_gift(mob);
        }

        self.entity = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if let Some(entity) = &self.entity {
            // Vanilla `setLookAt(Entity, ...)` aims at `entity.getEyeY()`.
            let position = entity.position();
            mob.mob_base().controls().lock().look_control.set_look_at(
                glam::DVec3::new(position.x, entity.get_eye_y(), position.z),
                OFFER_LOOK_Y_MAX_ROT_SPEED,
                OFFER_LOOK_X_MAX_ROT_ANGLE,
            );
        }

        self.tick -= 1;
    }
}

/// Vanilla `ServerLevel.getNearestEntity` over `CANDIDATE_FOR_IRON_GOLEM_GIFT`.
fn nearest_gift_candidate(
    mob: &dyn PathfinderMob,
    world: &World,
    context: &TargetingConditions,
) -> Option<SharedEntity> {
    let search_box = OfferFlowerGoal::golem_bounding_box(mob);
    // Vanilla `ServerLevel.getNearestEntity(..., golem.getX(), golem.getY(), golem.getZ(), box)`.
    let origin = mob.position();

    world.nearest_entity_in_aabb_matching(&search_box, origin, |entity| {
        let in_tag = steel_registry::REGISTRY.entity_types.is_in_tag(
            entity.entity_type(),
            &EntityTypeTag::CANDIDATE_FOR_IRON_GOLEM_GIFT,
        );
        in_tag
            && entity
                .as_living_entity()
                .is_some_and(|target| context.test(world, Some(mob), target))
    })
}

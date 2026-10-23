//! Experience bottle item behavior (`ExperienceBottleItem`).
//!
//! Throwing an experience bottle plays its throw sound at the player, spawns a
//! [`ThrownExperienceBottleEntity`] 0.7-power along the look direction with
//! vanilla's `-20.0F` pitch offset, awards the used-item stat, and consumes one
//! bottle. Mirrors vanilla `ExperienceBottleItem.use`.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::sound_events;
use steel_registry::stat::vanilla_stat_types;

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::entity::Entity as _;
use crate::entity::SharedEntity;
use crate::entity::entities::objects::projectiles::ThrownExperienceBottleEntity;
use crate::entity::projectile::spawn_projectile_from_rotation;
use crate::player::Player;

use super::throwable_potion::throw_pitch;

/// Vanilla `ExperienceBottleItem.use` spawn power.
const SHOOT_POWER: f32 = 0.7;

/// Vanilla `Projectile.spawnProjectileFromRotation` `yOffset`, added to the
/// shooter's pitch.
const THROW_Y_OFFSET: f32 = -20.0;

/// Vanilla spawn uncertainty for a thrown experience bottle.
const THROW_UNCERTAINTY: f32 = 1.0;

const THROW_SOUND_VOLUME: f32 = 0.5;

/// Behavior for the experience bottle item.
#[item_behavior(class = "ExperienceBottleItem")]
pub struct ExperienceBottleItem;

impl ItemBehavior for ExperienceBottleItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let player = context.player;
        let world = context.world;

        world.play_sound_at(
            &sound_events::ENTITY_EXPERIENCE_BOTTLE_THROW,
            SoundSource::Neutral,
            player.position(),
            THROW_SOUND_VOLUME,
            throw_pitch(),
            None,
        );

        let owner: SharedEntity = Arc::<Player>::clone(player);
        let bottle_stack = context.inv.with_item(|item| item.clone());
        let mut thrown_item = bottle_stack.clone();
        let spawned = spawn_projectile_from_rotation(
            world,
            owner.as_ref(),
            &mut thrown_item,
            THROW_Y_OFFSET,
            SHOOT_POWER,
            THROW_UNCERTAINTY,
            || ThrownExperienceBottleEntity::new_from_owner(world, &owner, bottle_stack),
        );
        let Some(_projectile) = spawned else {
            return InteractionResult::Fail;
        };

        player.award_stat(&vanilla_stat_types::ITEM_USED, thrown_item.item);
        let has_infinite_materials = player.has_infinite_materials();
        context
            .inv
            .with_item(|item| item.consume_one(has_infinite_materials));

        InteractionResult::Success
    }
}

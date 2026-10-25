//! Wind charge item behavior (`WindChargeItem`).
//!
//! Mirrors vanilla `WindChargeItem.use`: throwing a wind charge spawns a
//! [`WindChargeEntity`] at the thrower's eye height, shot along their look
//! direction with power 1.5, plays the throw sound, awards the item-used stat and
//! consumes one wind charge unless the player has infinite materials.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{sound_events, vanilla_entities};

use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::item::ItemBehavior;
use crate::enchantment_helper;
use crate::entity::entities::WindChargeEntity;
use crate::entity::{Entity, Projectile, SharedEntity, next_entity_id};
use crate::player::Player;

/// Vanilla `WindChargeItem.PROJECTILE_SHOOT_POWER`.
const SHOOT_POWER: f32 = 1.5;

/// Vanilla `WindChargeItem.use` throw uncertainty (`spawnProjectileFromRotation`).
const THROW_UNCERTAINTY: f32 = 1.0;

/// Vanilla `WindChargeItem.use` throw volume.
const THROW_SOUND_VOLUME: f32 = 0.5;

/// Vanilla `WindChargeItem.use` throw pitch jitter scale: `0.4 / (random * 0.4 + 0.8)`.
const THROW_PITCH_JITTER_SCALE: f32 = 0.4;
const THROW_PITCH_JITTER_BASE: f32 = 0.8;

/// Behavior for the wind charge item.
#[item_behavior(class = "WindChargeItem")]
pub struct WindChargeItem;

impl ItemBehavior for WindChargeItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let player = context.player;
        let world = context.world;

        // Vanilla creates the wind charge at feet x/z with the eye height y, then
        // `spawnProjectileFromRotation` shoots it along the thrower's rotation.
        let player_position = player.position();
        let position = DVec3::new(player_position.x, player.get_eye_y(), player_position.z);
        let wind_charge = WindChargeEntity::new(
            &vanilla_entities::WIND_CHARGE,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        );
        let owner: SharedEntity = Arc::<Player>::clone(player);
        wind_charge.set_owner_entity(Some(&owner));
        let (yaw, pitch) = player.rotation();
        wind_charge.shoot_from_rotation(
            player.as_ref(),
            pitch,
            yaw,
            0.0,
            SHOOT_POWER,
            THROW_UNCERTAINTY,
        );

        let wind_charge: SharedEntity = Arc::new(wind_charge);
        if let Err(error) = world.try_add_entity(Arc::clone(&wind_charge)) {
            log::debug!("failed to spawn wind charge: {error}");
            return InteractionResult::Fail;
        }

        let pitch = THROW_PITCH_JITTER_SCALE
            / rand::random_range(
                THROW_PITCH_JITTER_BASE..THROW_PITCH_JITTER_BASE + THROW_PITCH_JITTER_SCALE,
            );
        world.play_sound_at(
            &sound_events::ENTITY_WIND_CHARGE_THROW,
            SoundSource::Neutral,
            player_position,
            THROW_SOUND_VOLUME,
            pitch,
            None,
        );

        let mut thrown_item = context.inv.with_item(|item| item.clone());
        enchantment_helper::on_projectile_spawned(
            world,
            &mut thrown_item,
            wind_charge.as_ref(),
            Some(&owner),
        );

        player.award_stat(&vanilla_stat_types::ITEM_USED, thrown_item.item);
        let has_infinite_materials = player.has_infinite_materials();
        context
            .inv
            .with_item(|item| item.consume_one(has_infinite_materials));

        InteractionResult::Success
    }
}

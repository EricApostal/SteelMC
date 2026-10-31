//! Vanilla `DyeItem` behavior: dyes an alive, unsheared sheep.

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::DYE;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_events::ITEM_DYE_USE;
use steel_utils::Downcast as _;
use steel_utils::types::InteractionHand;

use crate::behavior::{InteractionResult, ItemBehavior};
use crate::entity::entities::SheepEntity;
use crate::entity::{Entity, LivingEntity};
use crate::player::Player;

/// Behavior for the sixteen dye items (`DyeItem`).
///
/// Ports vanilla `DyeItem.interactLivingEntity`: dying a sheep plays the `DYE_USE`
/// sound, sets the sheep's wool color, and consumes one dye from the stack.
#[item_behavior(class = "DyeItem")]
pub struct DyeItem;

impl ItemBehavior for DyeItem {
    fn interact_living_entity(
        &self,
        stack: &mut ItemStack,
        player: &Player,
        target: &dyn LivingEntity,
        _hand: InteractionHand,
    ) -> InteractionResult {
        let Some(dye_color) = stack.get(DYE).copied() else {
            return InteractionResult::Pass;
        };
        let Some(sheep) = target.downcast_ref::<SheepEntity>() else {
            return InteractionResult::Pass;
        };

        if !Entity::is_alive(sheep) || sheep.is_sheared() || sheep.color() == dye_color {
            return InteractionResult::Pass;
        }

        // Vanilla `DyeItem.interactLivingEntity`:
        // `sheep.level().playSound(player, sheep, DYE_USE, PLAYERS, 1.0F, 1.0F)`.
        // The entity overload sends `ClientboundSoundEntityPacket` (the sound
        // follows the sheep) and excludes the dyeing player, who hears it
        // client-side.
        if let Some(world) = sheep.level() {
            world.play_sound_from_entity(
                &ITEM_DYE_USE,
                SoundSource::Players,
                sheep.id(),
                sheep.position(),
                1.0,
                1.0,
                Some(player.id()),
            );
        }
        sheep.set_color(dye_color);
        stack.shrink_one();

        InteractionResult::Success
    }
}

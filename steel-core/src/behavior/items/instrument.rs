//! Goat horn instrument item behavior implementation.
//!
//! Mirrors vanilla `InstrumentItem`: the horn only works while the stack keeps
//! a `minecraft:instrument` component; playing it starts the item's active use,
//! plays the instrument sound, damages the horn and applies the instrument's
//! cooldown.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::INSTRUMENT;
use steel_registry::instrument::InstrumentValue;
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_events;

use crate::behavior::{InteractionResult, ItemBehavior, ItemUseAnimation, UseItemContext};
use crate::entity::{Entity, LivingEntity};
use crate::player::Player;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Behavior for instrument items (currently only the goat horn).
#[item_behavior]
pub struct InstrumentItem;

impl InstrumentItem {
    /// Plays the held instrument at the player's own position and awards the
    /// vanilla `INSTRUMENT_PLAY` game event.
    ///
    /// Vanilla plays through `Level.playSound(player, player, ...)`, so the
    /// player is excluded from the broadcast and receives the sound client-side.
    fn play(world: &Arc<World>, player: &Player, instrument: &InstrumentValue) {
        let volume = instrument.range() / 16.0;
        world.play_sound_holder_at(
            instrument.sound_event(),
            SoundSource::Records,
            player.position(),
            volume,
            1.0,
            Some(player.id()),
        );
        world.game_event(
            &vanilla_game_events::INSTRUMENT_PLAY,
            player.block_position(),
            &GameEventContext::new(Some(player as &dyn Entity), None),
        );
    }
}

impl ItemBehavior for InstrumentItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let player: &Arc<Player> = context.player;
        let world: &Arc<World> = context.world;

        // Vanilla reads the instrument off the held stack first and fails the
        // use outright when the component is absent.
        if context
            .inv
            .with_item(|stack| stack.get(INSTRUMENT).is_none())
        {
            return InteractionResult::Fail;
        }

        // Vanilla `InstrumentItem.use` order: start the use, then play.
        context.player.start_using_item(context.hand);
        let Some(instrument) = context.inv.with_item(|stack| {
            stack
                .get(INSTRUMENT)
                .map(|component| component.value().clone())
        }) else {
            return InteractionResult::Fail;
        };
        Self::play(world, player, &instrument);

        // Vanilla damages the horn by `instrument.durabilityDamage()` and then
        // starts `floor(useDuration * 20)` ticks of cooldown.
        let durability_damage = instrument.durability_damage();
        if durability_damage > 0 {
            let has_infinite_materials = player.has_infinite_materials();
            context
                .inv
                .with_item(|stack| stack.hurt_and_break(durability_damage, has_infinite_materials));
        }

        let use_duration = instrument.use_duration();
        if use_duration > 0.0 {
            let cooldown_ticks = (use_duration * 20.0).floor() as i32;
            context
                .inv
                .with_item(|stack| player.add_item_cooldown(stack, cooldown_ticks));
        }

        let item = context.inv.with_item(|stack| stack.item());
        player.award_stat(&vanilla_stat_types::ITEM_USED, item);
        InteractionResult::Consume
    }

    fn get_use_duration(&self, stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        stack.get(INSTRUMENT).map_or(0, |component| {
            (component.value().use_duration() * 20.0).floor() as i32
        })
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::TootHorn
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::data_components::components::InstrumentComponent;
    use steel_registry::data_components::vanilla_components::INSTRUMENT;
    use steel_registry::instrument::InstrumentValue;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::sound_event::SoundEventHolder;
    use steel_registry::{RegistryHolder, init_vanilla_registry, vanilla_items};
    use steel_utils::Identifier;
    use steel_utils::types::InteractionHand;
    use text_components::TextComponent;

    use crate::behavior::item::ItemBehavior;
    use crate::behavior::{InteractionResult, UseItemContext, init_behaviors};
    use crate::player::Player;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};
    use crate::world::World;

    use super::InstrumentItem;

    /// Builds a damageable stack carrying an inline instrument.
    ///
    /// Vanilla goat horns are not damageable (no `max_damage`), so a data-driven
    /// instrument with `durability_damage` can only exist on a damageable item.
    fn inline_instrument(use_duration: f32, durability_damage: i32) -> ItemStack {
        let value = InstrumentValue::new(
            SoundEventHolder::Direct {
                sound_id: Identifier::vanilla_static("test_horn"),
                fixed_range: Some(16.0),
            },
            use_duration,
            16.0,
            durability_damage,
            TextComponent::plain("Test horn"),
        )
        .expect("inline instrument should be valid");
        let mut stack = ItemStack::new(&vanilla_items::DIAMOND_SWORD);
        stack.set(
            INSTRUMENT,
            InstrumentComponent::new(RegistryHolder::direct(value)),
        );
        stack
    }

    fn play_horn(player: &Arc<Player>, world: &Arc<World>, stack: ItemStack) -> InteractionResult {
        player
            .inventory
            .lock()
            .set_item_in_hand(InteractionHand::MainHand, stack);
        let mut context = UseItemContext::new(
            player,
            InteractionHand::MainHand,
            world,
            Arc::clone(&player.inventory),
        );
        InstrumentItem.use_item(&mut context)
    }

    fn hand_item(player: &Player) -> ItemStack {
        player
            .inventory
            .lock()
            .get_item_in_hand(InteractionHand::MainHand)
            .clone()
    }

    #[test]
    fn playing_an_instrument_damages_it_and_applies_its_cooldown() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("instrument_damage_cooldown");
        let world = &world_fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "InstrumentTester", 1).build();
        let instrument = inline_instrument(3.5, 2);

        assert_eq!(
            play_horn(&player, world, instrument),
            InteractionResult::Consume
        );

        let after = hand_item(&player);
        assert_eq!(after.get_damage_value(), 2);
        // Vanilla `floor(useDuration * 20)` = 70 ticks of cooldown.
        assert!(player.is_item_on_cooldown(&after));
    }
}

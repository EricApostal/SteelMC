//! Splash and lingering potion item behaviors (`ThrowablePotionItem`).
//!
//! Throwing a splash or lingering potion plays the potion's throw sound at the
//! player, spawns the corresponding thrown potion 0.5-power along the look
//! direction with vanilla's `-20.0F` pitch offset, awards the used-item stat, and
//! consumes one potion. Mirrors vanilla `SplashPotionItem.use` /
//! `LingeringPotionItem.use`, which both delegate to
//! `ThrowablePotionItem.use`.
// TODO: Implement inherited PotionItem.useOn water-to-mud conversion.
// TODO: Add the inherited water default instance when Steel has item-specific
// default-stack factories.
//
// The inherited `ProjectileItem` half (`asProjectile` / `createDispenseConfig`)
// lives here; `DispenserBlock` reaches it through
// `ItemBehavior::as_projectile_item`, so a dispenser shoots splash and lingering
// potions exactly like vanilla.

use std::borrow::Cow;
use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{sound_events, vanilla_entities};
use steel_utils::Direction;
use text_components::TextComponent;

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::items::{DispenseConfig, ProjectileItem};
use crate::entity::Entity as _;
use crate::entity::SharedEntity;
use crate::entity::entities::objects::projectiles::{
    ThrownLingeringPotionEntity, ThrownSplashPotionEntity,
};
use crate::entity::next_entity_id;
use crate::entity::projectile::ThrowableItemProjectile as _;
use crate::entity::projectile::spawn_projectile_from_rotation;
use crate::player::Player;

use super::dynamic_name::potion_name;

/// Vanilla `ThrowablePotionItem.PROJECTILE_SHOOT_POWER`.
const PROJECTILE_SHOOT_POWER: f32 = 0.5;

/// Vanilla `Projectile.spawnProjectileFromRotation` `yOffset`, added to the
/// shooter's pitch.
const THROW_Y_OFFSET: f32 = -20.0;

/// Vanilla spawn uncertainty for a thrown potion.
const THROW_UNCERTAINTY: f32 = 1.0;

/// Vanilla throw volume for both potion kinds.
const THROW_SOUND_VOLUME: f32 = 0.5;

/// Vanilla throw pitch jitter: `0.4 / (random * 0.4 + 0.8)`.
const THROW_PITCH_JITTER_SCALE: f32 = 0.4;
const THROW_PITCH_JITTER_BASE: f32 = 0.8;

/// Vanilla's throw-pitch jitter, shared by the potion and experience bottles.
#[must_use]
pub(crate) fn throw_pitch() -> f32 {
    THROW_PITCH_JITTER_SCALE
        / rand::random_range(
            THROW_PITCH_JITTER_BASE..THROW_PITCH_JITTER_BASE + THROW_PITCH_JITTER_SCALE,
        )
}

/// Splash-potion behavior providing Vanilla's potion-content-dependent name.
#[item_behavior]
pub struct SplashPotionItem;

impl ItemBehavior for SplashPotionItem {
    fn get_name<'a>(&self, stack: &'a ItemStack) -> Cow<'a, TextComponent> {
        potion_name(stack)
    }

    fn as_projectile_item(&self) -> Option<&dyn ProjectileItem> {
        Some(self)
    }

    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let player = context.player;
        let world = context.world;

        // Vanilla `SplashPotionItem.use` plays the throw sound before delegating
        // to `ThrowablePotionItem.use`.
        world.play_sound_at(
            &sound_events::ENTITY_SPLASH_POTION_THROW,
            SoundSource::Players,
            player.position(),
            THROW_SOUND_VOLUME,
            throw_pitch(),
            None,
        );

        throw_potion(context, |world, owner, stack| {
            ThrownSplashPotionEntity::new_from_owner(world, owner, stack)
        })
    }
}

impl ProjectileItem for SplashPotionItem {
    /// Vanilla `ThrowablePotionItem.asProjectile` →
    /// `SplashPotion.createPotion(Level, Position, ItemStack)`.
    fn as_projectile(
        &self,
        world: &Arc<crate::world::World>,
        position: DVec3,
        stack: &ItemStack,
        _direction: Direction,
    ) -> Option<SharedEntity> {
        let entity = ThrownSplashPotionEntity::new(
            &vanilla_entities::SPLASH_POTION,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        );
        entity.set_item_clamped(stack.clone());
        Some(Arc::new(entity))
    }

    /// Vanilla `ThrowablePotionItem.createDispenseConfig`.
    fn create_dispense_config(&self) -> DispenseConfig {
        DispenseConfig::throwable_potion()
    }
}

/// Lingering-potion behavior providing Vanilla's potion-content-dependent name.
#[item_behavior]
pub struct LingeringPotionItem;

impl ItemBehavior for LingeringPotionItem {
    fn get_name<'a>(&self, stack: &'a ItemStack) -> Cow<'a, TextComponent> {
        potion_name(stack)
    }

    fn as_projectile_item(&self) -> Option<&dyn ProjectileItem> {
        Some(self)
    }

    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let player = context.player;
        let world = context.world;

        // Vanilla `LingeringPotionItem.use` throws from the neutral source.
        world.play_sound_at(
            &sound_events::ENTITY_LINGERING_POTION_THROW,
            SoundSource::Neutral,
            player.position(),
            THROW_SOUND_VOLUME,
            throw_pitch(),
            None,
        );

        throw_potion(context, |world, owner, stack| {
            ThrownLingeringPotionEntity::new_from_owner(world, owner, stack)
        })
    }
}

impl ProjectileItem for LingeringPotionItem {
    /// Vanilla `ThrowablePotionItem.asProjectile` →
    /// `LingeringPotion.createPotion(Level, Position, ItemStack)`.
    fn as_projectile(
        &self,
        world: &Arc<crate::world::World>,
        position: DVec3,
        stack: &ItemStack,
        _direction: Direction,
    ) -> Option<SharedEntity> {
        let entity = ThrownLingeringPotionEntity::new(
            &vanilla_entities::LINGERING_POTION,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        );
        entity.set_item_clamped(stack.clone());
        Some(Arc::new(entity))
    }

    /// Vanilla `ThrowablePotionItem.createDispenseConfig`.
    fn create_dispense_config(&self) -> DispenseConfig {
        DispenseConfig::throwable_potion()
    }
}

/// Vanilla `ThrowablePotionItem.use`: spawn the potion, award the stat, consume.
fn throw_potion<E>(
    context: &mut UseItemContext,
    create: impl FnOnce(&Arc<crate::world::World>, &SharedEntity, ItemStack) -> E,
) -> InteractionResult
where
    E: crate::entity::Projectile + crate::entity::Entity + 'static,
{
    let player = context.player;
    let world = context.world;
    let owner: SharedEntity = Arc::<Player>::clone(player);

    let potion_stack = context.inv.with_item(|item| item.clone());
    let mut thrown_item = potion_stack.clone();
    let spawned = spawn_projectile_from_rotation(
        world,
        owner.as_ref(),
        &mut thrown_item,
        THROW_Y_OFFSET,
        PROJECTILE_SHOOT_POWER,
        THROW_UNCERTAINTY,
        || create(world, &owner, potion_stack),
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

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use steel_registry::data_components::components::PotionContents;
    use steel_registry::data_components::vanilla_components::POTION_CONTENTS;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{
        RegistryReference, init_vanilla_registry, vanilla_entities, vanilla_items, vanilla_potions,
    };
    use steel_utils::Direction;
    use steel_utils::Downcast as _;

    use crate::behavior::items::ProjectileItem as _;
    use crate::behavior::{ITEM_BEHAVIORS, init_behaviors};
    use crate::entity::Entity as _;
    use crate::entity::entities::objects::projectiles::{
        ThrownLingeringPotionEntity, ThrownSplashPotionEntity,
    };
    use crate::entity::projectile::ThrowableItemProjectile as _;
    use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

    use super::{LingeringPotionItem, SplashPotionItem, throw_pitch};

    /// Vanilla `ProjectileItem$DispenseConfig.DEFAULT`: power 1.1, uncertainty
    /// 6.0. Thrown potions use `power * 1.25` and `uncertainty * 0.5`.
    #[test]
    fn dispensed_potion_uses_vanilla_shoot_parameters() {
        init_vanilla_registry();
        init_behaviors();
        for item in [
            &*vanilla_items::SPLASH_POTION,
            &*vanilla_items::LINGERING_POTION,
        ] {
            let projectile = ITEM_BEHAVIORS
                .get_behavior(item)
                .as_projectile_item()
                .expect("thrown potions dispense as projectiles");
            let config = projectile.create_dispense_config();
            assert!(
                (config.power - 1.375).abs() < f32::EPSILON,
                "thrown potion dispense power is vanilla's 1.1 * 1.25, got {}",
                config.power
            );
            assert!(
                (config.uncertainty - 3.0).abs() < f32::EPSILON,
                "thrown potion dispense uncertainty is vanilla's 6.0 * 0.5, got {}",
                config.uncertainty
            );
            assert_eq!(config.override_dispense_event, None);
        }
    }

    fn potion_test_world(name: &'static str) -> TestWorld {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world(name);
        insert_ready_full_chunk(&world_fixture.world, steel_utils::ChunkPos::new(0, 0));
        world_fixture
    }

    fn stacked_splash_potion() -> ItemStack {
        let mut stack = ItemStack::new(&vanilla_items::SPLASH_POTION);
        stack.set(
            POTION_CONTENTS,
            PotionContents::new(
                Some(RegistryReference::new(&vanilla_potions::POISON)),
                None,
                Vec::new(),
                None,
            ),
        );
        stack
    }

    #[test]
    fn dispensed_splash_potion_spawns_a_thrown_potion_at_the_dispense_position() {
        let world_fixture = potion_test_world("itemfact_splash_dispense");
        let world = &world_fixture.world;
        let position = DVec3::new(8.5, 64.5, 8.5);
        let stack = stacked_splash_potion();

        let projectile = SplashPotionItem
            .as_projectile(&world, position, &stack, Direction::North)
            .expect("splash potion dispenses an entity");
        assert_eq!(projectile.entity_type(), &vanilla_entities::SPLASH_POTION);
        assert_eq!(projectile.position(), position);
        let potion = projectile
            .downcast_ref::<ThrownSplashPotionEntity>()
            .expect("splash potion dispenses the splash entity");
        assert_eq!(potion.get_item(), stack);
    }

    #[test]
    fn dispensed_lingering_potion_spawns_a_thrown_lingering_potion() {
        let world_fixture = potion_test_world("itemfact_lingering_dispense");
        let world = &world_fixture.world;
        let position = DVec3::new(8.5, 64.5, 8.5);
        let stack = ItemStack::new(&vanilla_items::LINGERING_POTION);

        let projectile = LingeringPotionItem
            .as_projectile(&world, position, &stack, Direction::North)
            .expect("lingering potion dispenses an entity");
        assert_eq!(
            projectile.entity_type(),
            &vanilla_entities::LINGERING_POTION
        );
        let potion = projectile
            .downcast_ref::<ThrownLingeringPotionEntity>()
            .expect("lingering potion dispenses the lingering entity");
        assert_eq!(potion.get_item(), stack);
    }

    /// Vanilla's throw-pitch jitter (`0.4 / (random * 0.4 + 0.8)`) stays in
    /// `[1/3, 1/2]` for every random draw.
    #[test]
    fn throw_pitch_stays_within_vanilla_bounds() {
        for _ in 0..1_000 {
            let pitch = throw_pitch();
            assert!(
                (1.0 / 3.0..=0.5).contains(&pitch),
                "pitch {pitch} outside vanilla bounds"
            );
        }
    }
}

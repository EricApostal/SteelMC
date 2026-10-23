//! Bow item behavior (`BowItem`).
//!
//! Mirrors vanilla `BowItem` / `ProjectileWeaponItem`: holding the bow charges
//! it for up to 72000 ticks, releasing draws ammo from the player's inventory,
//! spreads it by `projectile_spread`, fires it at `power * 3` with a crit at
//! full draw, damages the bow and awards the used-item stat.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::INTANGIBLE_PROJECTILE;
use steel_registry::enchantment::effect::EnchantmentEffectComponent;
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_item_tags, vanilla_items,
};
use steel_utils::Downcast as _;

use crate::behavior::items::arrow::create_arrow;
use crate::behavior::{
    ItemBehavior, ItemUseAnimation,
    context::{InteractionResult, UseItemContext},
};
use crate::entity::entities::objects::projectiles::AbstractArrow as _;
use crate::entity::entities::objects::projectiles::ArrowEntity;
use crate::entity::projectile::spawn_shot_projectile;
use crate::entity::{Entity, LivingEntity, SharedEntity};
use crate::inventory::container::Container as _;
use crate::player::Player;
use crate::player::player_inventory::PlayerInventory;
use crate::world::World;

/// Vanilla `BowItem.MAX_DRAW_DURATION`.
const MAX_DRAW_DURATION: f32 = 20.0;

/// Vanilla `BowItem.getUseDuration`.
const USE_DURATION: i32 = 72000;

/// Vanilla `BowItem` shot speed multiplier (`pow * 3.0F`).
const SHOOT_POWER_SCALE: f32 = 3.0;

/// Vanilla `BowItem` shot uncertainty.
const SHOOT_UNCERTAINTY: f32 = 1.0;

/// Behavior for the bow item.
#[item_behavior(class = "BowItem")]
pub struct BowItem;

impl ItemBehavior for BowItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        // Vanilla `BowItem.use`: a creative player may always draw, otherwise a
        // supported projectile must be available.
        let weapon = context.inv.with_item(|item| item.clone());
        let has_ammo = !find_ammo(context.player, &weapon).is_empty();
        if !context.player.has_infinite_materials() && !has_ammo {
            return InteractionResult::Fail;
        }

        context.player.start_using_item(context.hand);
        InteractionResult::Consume
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::Bow
    }

    fn get_use_duration(&self, _stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        USE_DURATION
    }

    fn use_on_release(&self, _stack: &ItemStack) -> bool {
        true
    }

    fn release_using(
        &self,
        stack: &mut ItemStack,
        world: &Arc<World>,
        user: &dyn LivingEntity,
        time_left: i32,
    ) -> bool {
        // Vanilla `BowItem.releaseUsing` only fires for players.
        let Some(player) = user.as_player() else {
            return false;
        };

        let ammo = find_ammo(player, stack);
        if ammo.is_empty() {
            return false;
        }

        let time_held = USE_DURATION - time_left;
        let power = power_for_time(time_held);
        if power < 0.1 {
            return false;
        }

        let Some(owner) = world.get_entity_by_id(user.id()) else {
            return false;
        };

        let drawn = draw(stack, &ammo, player);
        if !drawn.is_empty() {
            shoot_arrows(
                world,
                player,
                &owner,
                stack,
                &drawn,
                power * SHOOT_POWER_SCALE,
                power == 1.0,
            );
        }

        world.play_sound_at(
            &sound_events::ENTITY_ARROW_SHOOT,
            SoundSource::Players,
            player.position(),
            1.0,
            1.0 / (rand::random::<f32>() * 0.4 + 1.2) + power * 0.5,
            None,
        );
        player.award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        true
    }
}

/// Vanilla `BowItem.getPowerForTime`: the draw curve, capped at 1.
#[must_use]
pub fn power_for_time(time_held: i32) -> f32 {
    let mut power = time_held as f32 / MAX_DRAW_DURATION;
    power = (power * power + power * 2.0) / 3.0;
    power.min(1.0)
}

/// Returns the first supported projectile stack a player holds, following
/// vanilla `Player.getProjectile`: offhand, main hand, then the main inventory,
/// falling back to a plain arrow for infinite-materials players.
#[must_use]
pub fn find_ammo(player: &Player, _weapon: &ItemStack) -> ItemStack {
    let inventory = player.inventory.lock();

    let offhand = inventory.get_item(PlayerInventory::SLOT_OFFHAND);
    if is_supported_projectile(offhand) {
        return offhand.clone();
    }
    let selected = inventory.get_selected_item();
    if is_supported_projectile(selected) {
        return selected.clone();
    }
    for stack in inventory.get_items() {
        if is_supported_projectile(stack) {
            return stack.clone();
        }
    }

    drop(inventory);
    if player.has_infinite_materials() {
        return ItemStack::new(&vanilla_items::ARROW);
    }
    ItemStack::empty()
}

/// Returns whether `stack` is in the `minecraft:arrows` item tag (vanilla
/// `ProjectileWeaponItem.ARROW_ONLY`).
#[must_use]
pub fn is_supported_projectile(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && REGISTRY
            .items
            .is_in_tag(stack.item(), &vanilla_item_tags::ItemTag::ARROWS)
}

/// Vanilla `ProjectileWeaponItem.draw`: expands the ammo stack into the number
/// of projectiles the `projectile_count` enchantment grants, consuming the ammo
/// that is not covered by infinite materials or a zero `ammo_use`.
#[must_use]
pub fn draw(weapon: &ItemStack, ammo: &ItemStack, shooter: &Player) -> Vec<ItemStack> {
    if ammo.is_empty() {
        return Vec::new();
    }

    let count = weapon
        .apply_unconditional_enchantment_value_effects(
            EnchantmentEffectComponent::ProjectileCount,
            1.0,
        )
        .max(1.0) as usize;

    let mut drawn = Vec::with_capacity(count);
    for index in 0..count {
        let used = use_ammo(weapon, ammo, shooter, index > 0);
        if used.is_empty() {
            break;
        }
        drawn.push(used);
    }
    drawn
}

/// Vanilla `ProjectileWeaponItem.useAmmo` for one drawn projectile: the stack is
/// split out of the inventory unless the shot costs no ammo, in which case the
/// copy is marked `INTANGIBLE_PROJECTILE` so it cannot be picked up.
fn use_ammo(
    weapon: &ItemStack,
    ammo: &ItemStack,
    shooter: &Player,
    force_infinite: bool,
) -> ItemStack {
    let ammo_to_use = if force_infinite || shooter.has_infinite_materials() {
        0
    } else {
        weapon
            .apply_unconditional_enchantment_value_effects(EnchantmentEffectComponent::AmmoUse, 1.0)
            .max(0.0) as i32
    };

    if ammo_to_use > ammo.count() {
        return ItemStack::empty();
    }

    if ammo_to_use == 0 {
        let mut copy = ammo.copy_with_count(1);
        copy.set(INTANGIBLE_PROJECTILE, ());
        return copy;
    }

    take_ammo(shooter, ammo, ammo_to_use)
}

/// Splits `amount` items of `ammo` out of the player's inventory, mirroring
/// vanilla `ItemStack.split(ammoToUse)` on the stack `getProjectile` returned.
fn take_ammo(shooter: &Player, ammo: &ItemStack, amount: i32) -> ItemStack {
    let mut inventory = shooter.inventory.lock();

    let slot = std::iter::once(PlayerInventory::SLOT_OFFHAND)
        .chain(std::iter::once(usize::from(inventory.get_selected_slot())))
        .chain(0..PlayerInventory::INVENTORY_SIZE)
        .find(|slot| {
            let stack = inventory.get_item(*slot);
            !stack.is_empty() && ItemStack::is_same_item_same_components(stack, ammo)
        });

    let Some(slot) = slot else {
        return ItemStack::empty();
    };

    let stack = inventory.get_item_mut(slot);
    if amount > stack.count() {
        return ItemStack::empty();
    }
    let mut used = stack.copy_with_count(amount);
    stack.shrink(amount);
    if stack.is_empty() {
        inventory.set_item(slot, ItemStack::empty());
    }
    used.set_count(amount);
    used
}

/// Vanilla `ProjectileWeaponItem.shoot` for `BowItem.shootProjectile`: fires
/// every drawn projectile, spreading them by `projectile_spread`, marking full
/// draws as crits, and damaging the bow once per projectile.
fn shoot_arrows(
    world: &Arc<World>,
    shooter: &Player,
    owner: &SharedEntity,
    weapon: &mut ItemStack,
    drawn: &[ItemStack],
    power: f32,
    is_crit: bool,
) {
    let spread = weapon
        .apply_unconditional_enchantment_value_effects(
            EnchantmentEffectComponent::ProjectileSpread,
            0.0,
        )
        .max(0.0);
    let angle_step = if drawn.len() == 1 {
        0.0
    } else {
        2.0 * spread / (drawn.len() - 1) as f32
    };
    let angle_offset = ((drawn.len() - 1) % 2) as f32 * angle_step / 2.0;
    let mut direction = 1.0f32;

    let (yaw, pitch) = shooter.rotation();
    let has_infinite_materials = shooter.has_infinite_materials();

    for (index, projectile) in drawn.iter().enumerate() {
        if projectile.is_empty() {
            continue;
        }
        let angle = angle_offset + direction * ((index + 1) / 2) as f32 * angle_step;
        direction = -direction;

        let arrow = create_arrow(
            world,
            shooter.position(),
            projectile,
            Some(owner),
            Some(weapon),
        );
        if is_crit && let Some(arrow) = arrow.downcast_ref::<ArrowEntity>() {
            arrow.set_crit_arrow(true);
        }

        let mut projectile_item = projectile.clone();
        let _ = spawn_shot_projectile(world, arrow, &mut projectile_item, |projectile| {
            projectile.shoot_from_rotation(
                owner.as_ref(),
                pitch,
                yaw + angle,
                0.0,
                power,
                SHOOT_UNCERTAINTY,
            );
        });

        weapon.hurt_and_break(1, has_infinite_materials);
        if weapon.is_empty() {
            break;
        }
    }
}

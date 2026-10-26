//! Crossbow item behavior (`CrossbowItem`).
//!
//! Mirrors vanilla `CrossbowItem` / `ProjectileWeaponItem`: right-clicking a
//! crossbow that already carries projectiles in its `charged_projectiles`
//! component fires them (vanilla `CrossbowItem.use` -> `performShooting`);
//! otherwise the crossbow charges for `getChargeDuration` ticks, playing the
//! loading sounds and loading arrows or firework rockets into the component at
//! full charge (`onUseTick` -> `tryLoadProjectiles`). Releasing a fully charged
//! crossbow only damages/uses it (`releaseUsing` returns
//! `power >= 1.0 && isCharged(stack)`); it never fires, which is why the firing
//! path lives in [`ItemBehavior::use_item`] and not in
//! [`ItemBehavior::release_using`].
//!
//! `multishot` is the vanilla `projectile_spread`/`projectile_count` pair: every
//! drawn projectile is fired in turn, offset by `2 * spread / (count - 1)`
//! degrees per slot, rotating the shooter's view vector around its up vector
//! (`getProjectileShotVector` when a target is given). `piercing` is applied by
//! the shared arrow factory (`AbstractArrow`'s constructor reads the weapon's
//! `projectile_piercing` value in `createArrow`), so the crossbow only has to
//! pass itself as the firing weapon.

use std::f64::consts::FRAC_PI_2;
use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::REGISTRY;
use steel_registry::data_components::components::ChargedProjectiles;
use steel_registry::data_components::vanilla_components::{
    CHARGED_PROJECTILES, INTANGIBLE_PROJECTILE,
};
use steel_registry::enchantment::effect::{CrossbowChargingSounds, EnchantmentEffectComponent};
use steel_registry::item_stack::ItemStack;
use steel_registry::item_stack_template::ItemStackTemplate;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{
    RegistryExt as _, TaggedRegistryExt as _, sound_events, vanilla_entities, vanilla_item_tags,
    vanilla_items,
};
use steel_utils::Downcast as _;
use steel_utils::types::InteractionHand;

use crate::behavior::advancement_hooks::raise_shot_crossbow;
use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::items::arrow::create_arrow;
use crate::behavior::items::bow::{draw, find_ammo_matching, is_supported_projectile};
use crate::behavior::{ItemBehavior, ItemUseAnimation};
use crate::entity::entities::objects::projectiles::{
    AbstractArrow as _, ArrowEntity, FireworkRocketEntity, SpectralArrowEntity,
};
use crate::entity::projectile::{Projectile, spawn_shot_projectile};
use crate::entity::{LivingEntity, LivingEntityRef, SharedEntity, next_entity_id};
use crate::player::Player;
use crate::world::World;

/// Vanilla `CrossbowItem.getUseDuration`.
const USE_DURATION: i32 = 72000;

/// Vanilla `CrossbowItem.getChargeDuration`: `modifyCrossbowChargingTime(stack,
/// entity, 1.25F) * 20.0F`, floored. Quick Charge adds `-0.25` per level, so the
/// result is the familiar `25 - 5 * level` ticks.
const CHARGE_TIME_BASE: f32 = 1.25;
/// Vanilla `CrossbowItem.MAX_CHARGE_DURATION`.
const MAX_CHARGE_DURATION: f32 = 20.0;
/// Vanilla `CrossbowItem.ARROW_POWER` (`getShootingPower` without a firework).
const ARROW_POWER: f32 = 3.15;
/// Vanilla `CrossbowItem.FIREWORK_POWER` (`getShootingPower` with a firework).
const FIREWORK_POWER: f32 = 1.6;
/// Vanilla `CrossbowItem.MOB_ARROW_POWER`: the velocity a mob's crossbow bolt
/// flies at. Both `Pillager.performRangedAttack` and `Piglin.performRangedAttack`
/// pass this same literal `1.6F` to `CrossbowAttackMob.performCrossbowAttack`
/// (the goal and the piglin brain only hand over the unrelated `1.0F` bound of
/// vanilla's `RangedAttackMob`).
pub const MOB_ARROW_POWER: f32 = 1.6;
/// Vanilla `CrossbowItem.shootProjectile` inaccuracy (`shoot(..., power, 1.0F)`).
const SHOOT_UNCERTAINTY: f32 = 1.0;
/// Vanilla `CrossbowItem` firing a firework spawns it below the shooter's eyes.
const FIREWORK_SPAWN_EYE_OFFSET: f64 = 0.15;
/// Vanilla `CrossbowItem.onUseTick` start-sound threshold.
const START_SOUND_PERCENT: f32 = 0.2;
/// Vanilla `CrossbowItem.onUseTick` mid-sound threshold.
const MID_SOUND_PERCENT: f32 = 0.5;

/// Behavior for the crossbow item.
#[item_behavior(class = "CrossbowItem")]
pub struct CrossbowItem;

impl ItemBehavior for CrossbowItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        // Vanilla `CrossbowItem.use`: a charged crossbow fires immediately, an
        // uncharged one starts charging when the player holds supported ammo.
        let mut weapon = context.inv.with_item(|item| item.clone());
        let loaded_power = weapon
            .get(CHARGED_PROJECTILES)
            .filter(|projectiles| !projectiles.items().is_empty())
            .map(shooting_power);

        let Some(velocity) = loaded_power else {
            let ammo = find_ammo_matching(context.player, is_crossbow_ammo);
            if ammo.is_empty() {
                return InteractionResult::Fail;
            }
            context.player.start_using_item(context.hand);
            return InteractionResult::Consume;
        };

        // The shot mutates the held stack (clears the loaded projectiles, hurts
        // the crossbow) and raises the advancement/stat, so it runs outside the
        // inventory lock and the result is written back afterwards, exactly like
        // `Player::release_using_item_with_behavior`.
        let shooter_player: Arc<Player> = Arc::clone(context.player);
        let shooter: SharedEntity = shooter_player;
        perform_shooting(
            context.world,
            &shooter,
            &mut weapon,
            velocity,
            SHOOT_UNCERTAINTY,
            None,
        );
        context.inv.with_item(|item| *item = weapon);
        InteractionResult::Consume
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::Crossbow
    }

    fn get_use_duration(&self, _stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        USE_DURATION
    }

    /// Vanilla `CrossbowItem.useOnRelease`: `stack.is(this)`.
    fn use_on_release(&self, stack: &ItemStack) -> bool {
        stack.is(&vanilla_items::CROSSBOW)
    }

    fn release_using(
        &self,
        stack: &mut ItemStack,
        _world: &Arc<World>,
        _user: &dyn LivingEntity,
        time_left: i32,
    ) -> bool {
        // Vanilla `CrossbowItem.releaseUsing`: `getPowerForTime(getUseDuration
        // (stack, entity) - timeLeft, stack, entity) >= 1.0 && isCharged(stack)`.
        let time_held = USE_DURATION - time_left;
        power_for_time(time_held, stack) >= 1.0 && is_charged(stack)
    }

    fn on_use_tick(
        &self,
        world: &Arc<World>,
        user: LivingEntityRef<'_>,
        stack: &mut ItemStack,
        ticks_remaining: i32,
    ) {
        // Vanilla `CrossbowItem.onUseTick` runs on the server only, and loads
        // through `LivingEntity.getProjectile`: the shooter type decides the
        // ammo source, so the shared load below hands a player to their
        // inventory and a monster to its weapon's declared default.
        let entity = user.living();

        // Vanilla tracks `startSoundPlayed`/`midLoadSoundPlayed` per item
        // instance and resets them whenever the ratio drops below 0.2, i.e. at
        // the start of a charge. The ratio only ever rises during one
        // uninterrupted use, so comparing this tick's ratio against the previous
        // tick's (one more remaining tick) is the same decision without the
        // shared, non-thread-safe item state.
        let ratio = charge_ratio(ticks_remaining, stack);
        let previous_ratio = charge_ratio(ticks_remaining + 1, stack);
        let sounds = load_sounds(previous_ratio, ratio);
        let position = entity.position();

        let charging_sounds = get_charging_sounds(stack);
        if sounds.start
            && let Some(sound) = charging_sounds.start
        {
            play_load_sound(world, sound, SoundSource::Players, position, 0.5, 1.0);
        }
        if sounds.mid
            && let Some(sound) = charging_sounds.mid
        {
            play_load_sound(world, sound, SoundSource::Players, position, 0.5, 1.0);
        }
        if sounds.end
            && !is_charged(stack)
            && load_projectiles(entity, stack)
            && let Some(sound) = charging_sounds.end
        {
            let pitch = 1.0 / (rand::random::<f32>() * 0.5 + 1.0) + 0.2;
            play_load_sound(world, sound, entity.sound_source(), position, 1.0, pitch);
        }
    }
}

/// Vanilla `CrossbowItem.getShootingPower`: fireworks fly at `1.6`, everything
/// else at `3.15`.
fn shooting_power(projectiles: &ChargedProjectiles) -> f32 {
    if projectiles
        .items()
        .iter()
        .any(|template| template.item().key == vanilla_items::FIREWORK_ROCKET.key)
    {
        FIREWORK_POWER
    } else {
        ARROW_POWER
    }
}

/// Vanilla `CrossbowItem.isCharged`.
#[must_use]
pub fn is_charged(stack: &ItemStack) -> bool {
    stack
        .get(CHARGED_PROJECTILES)
        .is_some_and(|projectiles| !projectiles.items().is_empty())
}

/// Vanilla `CrossbowItem.getChargeDuration`, in ticks.
#[must_use]
pub fn get_charge_duration(stack: &ItemStack) -> i32 {
    let charge_time = stack.apply_unconditional_enchantment_value_effects(
        EnchantmentEffectComponent::CrossbowChargeTime,
        CHARGE_TIME_BASE,
    );
    // Vanilla inlines its `MAX_CHARGE_DURATION = 20.0F` constant into the
    // multiplier (the `javap` constant pool shows a literal `20.0f`); there is no
    // additional clamp.
    (charge_time * MAX_CHARGE_DURATION).floor() as i32
}

/// Vanilla `CrossbowItem.getPowerForTime` for a stack still holding
/// `ticks_remaining`: the fraction of the charge completed, capped at 1.
#[must_use]
pub fn charge_ratio(ticks_remaining: i32, stack: &ItemStack) -> f32 {
    let time_held = (USE_DURATION - ticks_remaining) as f32;
    (time_held / get_charge_duration(stack) as f32).min(1.0)
}

/// Vanilla `CrossbowItem.getPowerForTime(int i, stack, entity)` for an explicit
/// elapsed time (the `releaseUsing` shape).
#[must_use]
pub fn power_for_time(time_held: i32, stack: &ItemStack) -> f32 {
    (time_held as f32 / get_charge_duration(stack) as f32).min(1.0)
}

/// Vanilla `ProjectileWeaponItem.ARROW_OR_FIREWORK`: the crossbow's supported
/// projectiles — the `minecraft:arrows` tag plus firework rockets.
#[must_use]
pub fn is_crossbow_ammo(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && (stack.is(&vanilla_items::FIREWORK_ROCKET)
            || REGISTRY
                .items
                .is_in_tag(stack.item(), &vanilla_item_tags::ItemTag::ARROWS))
}

/// Vanilla `CrossbowItem.tryLoadProjectiles`: draws the shooter's supported
/// projectiles into the weapon's `charged_projectiles` component. Returns
/// whether anything was loaded.
pub fn try_load_projectiles(shooter: &Player, weapon: &mut ItemStack) -> bool {
    let ammo = find_ammo_matching(shooter, is_crossbow_ammo);
    let drawn = draw(weapon, &ammo, shooter);
    set_charged_projectiles(weapon, &drawn)
}

/// Vanilla `CrossbowItem.onUseTick`'s load for a shooter whose ammo is
/// resolved through `LivingEntity.getProjectile`: a player draws from their
/// inventory, a monster from the projectile its weapon declares, and any other
/// living entity has no projectile at all.
fn load_projectiles(shooter: &dyn LivingEntity, weapon: &mut ItemStack) -> bool {
    if let Some(player) = shooter.as_player() {
        try_load_projectiles(player, weapon)
    } else if shooter.as_monster().is_some() {
        try_load_monster_projectiles(shooter, weapon)
    } else {
        false
    }
}

/// Vanilla `Monster.getProjectile` for a projectile weapon: the stack the
/// monster visibly holds when the weapon's `getSupportedHeldProjectiles`
/// predicate accepts it (the off hand first, then the main hand), otherwise the
/// plain arrow every projectile weapon declares as its default.
///
/// Returns the ammo together with the hand it was taken from, which is the hand
/// vanilla's `useAmmo` splits the projectile out of.
fn monster_projectile(
    shooter: &dyn LivingEntity,
    weapon: &ItemStack,
) -> (ItemStack, Option<InteractionHand>) {
    let Some(supported) = held_projectile_set(weapon) else {
        return (ItemStack::empty(), None);
    };
    for hand in [InteractionHand::OffHand, InteractionHand::MainHand] {
        let stack = shooter.get_item_in_hand(hand);
        if supported(&stack) {
            return (stack, Some(hand));
        }
    }
    (ItemStack::new(&vanilla_items::ARROW), None)
}

/// The `ProjectileWeaponItem.getSupportedHeldProjectiles` predicate of the
/// projectile weapons Steel models: `ARROW_ONLY` for the bow and
/// `ARROW_OR_FIREWORK` for the crossbow. `None` means the stack is not a
/// projectile weapon, which vanilla's `Monster.getProjectile` reports as
/// `ItemStack.EMPTY`.
fn held_projectile_set(weapon: &ItemStack) -> Option<fn(&ItemStack) -> bool> {
    if weapon.is(&vanilla_items::CROSSBOW) {
        Some(is_crossbow_ammo)
    } else if weapon.is(&vanilla_items::BOW) {
        Some(is_supported_projectile)
    } else {
        None
    }
}

/// Vanilla `CrossbowItem.tryLoadProjectiles` for a monster shooter: the ammo is
/// the `Monster.getProjectile` stack, drawn through `ProjectileWeaponItem.draw`
/// with no inventory behind it. The first projectile is split out of the stack
/// the monster visibly holds (so holding matching ammo spends it), while the
/// extra projectiles `projectile_count` grants are intangible copies. Returns
/// whether anything was loaded.
pub fn try_load_monster_projectiles(shooter: &dyn LivingEntity, weapon: &mut ItemStack) -> bool {
    let (mut ammo, held_hand) = monster_projectile(shooter, weapon);
    if ammo.is_empty() {
        return false;
    }

    // Vanilla `draw` keeps its own copy of the ammo stack taken before the
    // loop, so every projectile after the first is an intangible copy of the
    // untouched stack rather than of the already-split one.
    let mut spare = ammo.copy_with_count(1);
    let mut drawn = Vec::with_capacity(projectile_count(weapon));
    for index in 0..projectile_count(weapon) {
        let projectile = if index == 0 {
            use_monster_ammo(weapon, &mut ammo, false)
        } else {
            use_monster_ammo(weapon, &mut spare, true)
        };
        if projectile.is_empty() {
            break;
        }
        drawn.push(projectile);
    }

    // Vanilla `useAmmo` splits the drawn projectile out of the stack
    // `getProjectile` returned, so a monster that visibly holds matching ammo
    // spends it. The declared default is a throwaway stack, so the split copy
    // of it is discarded with it.
    if let Some(hand) = held_hand {
        shooter.set_item_in_hand(hand, ammo);
    }

    set_charged_projectiles(weapon, &drawn)
}

/// Vanilla `ProjectileWeaponItem.draw`'s projectile count:
/// `EnchantmentHelper.processProjectileCount(level, weapon, shooter, 1)`.
fn projectile_count(weapon: &ItemStack) -> usize {
    weapon
        .apply_unconditional_enchantment_value_effects(
            EnchantmentEffectComponent::ProjectileCount,
            1.0,
        )
        .max(1.0) as usize
}

/// Vanilla `ProjectileWeaponItem.useAmmo` for a shooter with no inventory:
/// `ammo` is split in place, and a projectile that costs no ammo becomes an
/// intangible copy so it cannot be picked up. `force_infinite` is the vanilla
/// flag for every projectile after the first, which never costs ammo.
fn use_monster_ammo(weapon: &ItemStack, ammo: &mut ItemStack, force_infinite: bool) -> ItemStack {
    let ammo_to_use = if force_infinite {
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
    ammo.split(ammo_to_use)
}

/// Vanilla `ChargedProjectiles.ofNonEmpty`: drop empty stacks, cap at
/// `MAX_SIZE`, and store the result in the weapon's `charged_projectiles`
/// component. Returns whether anything was loaded.
fn set_charged_projectiles(weapon: &mut ItemStack, drawn: &[ItemStack]) -> bool {
    if drawn.is_empty() {
        return false;
    }
    let templates: Vec<ItemStackTemplate> = drawn
        .iter()
        .filter_map(|stack| ItemStackTemplate::from_stack(stack).ok())
        .take(ChargedProjectiles::MAX_SIZE)
        .collect();
    match ChargedProjectiles::new(templates) {
        Ok(projectiles) => {
            weapon.set(CHARGED_PROJECTILES, projectiles);
            true
        }
        Err(error) => {
            log::warn!("Tried to load invalid items as charged projectiles: {error}");
            false
        }
    }
}

/// The three vanilla load sounds as a plain default, i.e.
/// `CrossbowItem.DEFAULT_SOUNDS`.
const DEFAULT_CHARGING_SOUNDS: CrossbowChargingSounds = CrossbowChargingSounds {
    start: Some(&sound_events::ITEM_CROSSBOW_LOADING_START),
    mid: Some(&sound_events::ITEM_CROSSBOW_LOADING_MIDDLE),
    end: Some(&sound_events::ITEM_CROSSBOW_LOADING_END),
};

/// Vanilla `CrossbowItem.getChargingSounds`: `EnchantmentHelper.pickHighestLevel
/// (stack, CROSSBOW_CHARGING_SOUNDS)` with `DEFAULT_SOUNDS` as the fallback.
///
/// Quick Charge carries one `crossbow_charging_sounds` entry per level, so the
/// entry index is the level clamped into the list (`min(level, size) - 1`).
#[must_use]
pub fn get_charging_sounds(stack: &ItemStack) -> CrossbowChargingSounds {
    let Some(enchantments) = stack.get_enchantments() else {
        return DEFAULT_CHARGING_SOUNDS;
    };

    let mut best: Option<(i32, CrossbowChargingSounds)> = None;
    for (key, level) in enchantments.iter() {
        let level = i32::try_from(*level).unwrap_or(i32::MAX);
        if level <= 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        let entry = enchantment.effects.crossbow_charging_sounds.get(
            (level.min(enchantment.effects.crossbow_charging_sounds.len() as i32) - 1) as usize,
        );
        let Some(entry) = entry else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|(best_level, _)| level > *best_level)
        {
            best = Some((
                level,
                CrossbowChargingSounds {
                    start: entry.start,
                    mid: entry.mid,
                    end: entry.end,
                },
            ));
        }
    }

    best.map_or(DEFAULT_CHARGING_SOUNDS, |(_, sounds)| sounds)
}

/// Which loading sounds this tick of an active use must play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadSounds {
    /// The `start` sound crossed 20% charge.
    pub start: bool,
    /// The `mid` sound crossed 50% charge.
    pub mid: bool,
    /// The charge reached 100% (the caller still has to load successfully).
    pub end: bool,
}

/// Vanilla `CrossbowItem.onUseTick`'s threshold decisions, expressed against the
/// previous tick's charge ratio instead of the item's `startSoundPlayed` and
/// `midLoadSoundPlayed` flags.
#[must_use]
pub fn load_sounds(previous_ratio: f32, ratio: f32) -> LoadSounds {
    LoadSounds {
        start: previous_ratio < START_SOUND_PERCENT && ratio >= START_SOUND_PERCENT,
        mid: previous_ratio < MID_SOUND_PERCENT && ratio >= MID_SOUND_PERCENT,
        end: previous_ratio < 1.0 && ratio >= 1.0,
    }
}

/// Vanilla `CrossbowItem$ChargingSounds` lambdas: the `start` and `mid` sounds
/// play at `0.5` volume from the player category, the `end` sound plays at full
/// volume from the shooter's own category.
fn play_load_sound(
    world: &Arc<World>,
    sound: SoundEventRef,
    source: SoundSource,
    position: DVec3,
    volume: f32,
    pitch: f32,
) {
    world.play_sound_at(sound, source, position, volume, pitch, None);
}

/// Vanilla `CrossbowItem.performShooting`: clears the loaded projectiles, fires
/// them, then raises `shot_crossbow` and the `used` stat for a player shooter.
///
/// The caller owns the stack (vanilla mutates the stack in hand), so the cleared
/// and damaged weapon is written back here for the next read of that slot.
pub fn perform_shooting(
    world: &Arc<World>,
    shooter: &SharedEntity,
    weapon: &mut ItemStack,
    velocity: f32,
    inaccuracy: f32,
    target: Option<&SharedEntity>,
) {
    // Vanilla clears the component even when the shot then turns out to be
    // empty, so a stale component can never survive a shot attempt.
    let loaded = weapon
        .get(CHARGED_PROJECTILES)
        .cloned()
        .unwrap_or_else(ChargedProjectiles::empty);
    weapon.set(CHARGED_PROJECTILES, ChargedProjectiles::empty());
    if loaded.items().is_empty() {
        return;
    }

    let drawn: Vec<ItemStack> = loaded
        .items()
        .iter()
        .map(ItemStackTemplate::create)
        .collect();
    shoot(world, shooter, weapon, &drawn, velocity, inaccuracy, target);

    if let Some(player) = shooter.as_player() {
        raise_shot_crossbow(player, world, weapon);
        player.award_stat(&vanilla_stat_types::ITEM_USED, weapon.item());
    }
}

/// Vanilla `CrossbowAttackMob.performCrossbowAttack`: the mob releases the
/// crossbow in its weapon-holding hand at `velocity` with its own difficulty
/// uncertainty, aimed at its attack target. The `hand` is vanilla's
/// `ProjectileUtil.getWeaponHoldingHand`, and the cleared and damaged weapon is
/// written back to it.
///
/// Returns whether a loaded crossbow was released: vanilla always performs
/// `onCrossbowAttackPerformed` while only a crossbow stack shoots, so the
/// caller clears its own crossbow state while the shot is this caller's signal
/// that a bolt actually left.
pub fn perform_mob_crossbow_attack(
    world: &Arc<World>,
    shooter: &SharedEntity,
    target: &SharedEntity,
    velocity: f32,
    inaccuracy: f32,
) -> bool {
    let Some(living) = shooter.as_living_entity() else {
        return false;
    };
    let Some(hand) = weapon_holding_hand(living) else {
        return false;
    };
    let mut weapon = living.get_item_in_hand(hand);
    if !weapon.is(&vanilla_items::CROSSBOW) {
        return false;
    }
    let released = is_charged(&weapon);
    perform_shooting(
        world,
        shooter,
        &mut weapon,
        velocity,
        inaccuracy,
        Some(target),
    );
    living.set_item_in_hand(hand, weapon);
    released
}

/// Vanilla `ProjectileUtil.getWeaponHoldingHand`: the main hand when it holds
/// the crossbow, otherwise the off hand.
fn weapon_holding_hand(living: &dyn LivingEntity) -> Option<InteractionHand> {
    if living
        .get_item_in_hand(InteractionHand::MainHand)
        .is(&vanilla_items::CROSSBOW)
    {
        Some(InteractionHand::MainHand)
    } else if living
        .get_item_in_hand(InteractionHand::OffHand)
        .is(&vanilla_items::CROSSBOW)
    {
        Some(InteractionHand::OffHand)
    } else {
        None
    }
}

/// Vanilla `ProjectileWeaponItem.shoot` with `CrossbowItem.shootProjectile`:
/// spreads the drawn projectiles by the weapon's `projectile_spread`, fires each
/// one along a direction offset from the shooter's view, and damages the weapon
/// once per projectile.
fn shoot(
    world: &Arc<World>,
    shooter: &SharedEntity,
    weapon: &mut ItemStack,
    drawn: &[ItemStack],
    velocity: f32,
    inaccuracy: f32,
    target: Option<&SharedEntity>,
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
    let is_player = shooter.as_player().is_some();
    let has_infinite_materials = shooter
        .as_living_entity()
        .is_some_and(LivingEntity::has_infinite_materials);

    for (index, projectile_stack) in drawn.iter().enumerate() {
        if projectile_stack.is_empty() {
            continue;
        }
        let angle = angle_offset + direction * index.div_ceil(2) as f32 * angle_step;
        direction = -direction;

        let projectile = create_projectile(world, shooter, weapon, projectile_stack, is_player);
        // Vanilla `Projectile.spawnProjectile(projectile, level, weaponStack,
        // shootFunction)` hands the firing weapon to `onProjectileSpawned`.
        let mut firing_weapon = weapon.clone();
        let _ = spawn_shot_projectile(world, projectile, &mut firing_weapon, |projectile| {
            shoot_projectile(
                world, shooter, projectile, index, velocity, inaccuracy, angle, target,
            );
        });

        weapon.hurt_and_break(durability_use(projectile_stack), has_infinite_materials);
        if weapon.is_empty() {
            break;
        }
    }
}

/// Vanilla `CrossbowItem.shootProjectile`: aims at `target` when there is one,
/// otherwise rotates the shooter's view vector around its own up vector by
/// `angle` degrees, then shoots and plays the shot sound with a per-index pitch.
#[expect(
    clippy::too_many_arguments,
    reason = "argument list mirrors vanilla `shootProjectile` one-for-one"
)]
fn shoot_projectile(
    world: &Arc<World>,
    shooter: &SharedEntity,
    projectile: &(impl Projectile + ?Sized),
    index: usize,
    velocity: f32,
    inaccuracy: f32,
    angle: f32,
    target: Option<&SharedEntity>,
) {
    let (yaw, pitch) = shooter.rotation();
    let shot_vector = if let Some(target) = target {
        let target_position = target.position();
        let shooter_position = shooter.position();
        let delta_x = target_position.x - shooter_position.x;
        let delta_z = target_position.z - shooter_position.z;
        let horizontal_distance = (delta_x * delta_x + delta_z * delta_z).sqrt();
        // Vanilla `target.getY(0.3333333333333333F)` interpolates the target's
        // feet height a third of the way from its previous position.
        let old_y = target.old_position().y;
        let target_y = old_y + 0.333_333_333_333_333_3 * (target_position.y - old_y);
        let delta_y = target_y - projectile.position().y + horizontal_distance * 0.2_f64;
        projectile_shot_vector(shooter, DVec3::new(delta_x, delta_y, delta_z), angle)
    } else {
        view_vector_offset(shooter, yaw, pitch, angle)
    };

    projectile.shoot(shot_vector, velocity, inaccuracy);

    let pitch = get_shot_pitch(index);
    world.play_sound_at(
        &sound_events::ITEM_CROSSBOW_SHOOT,
        shooter.sound_source(),
        shooter.position(),
        1.0,
        pitch,
        None,
    );
}

/// Vanilla `CrossbowItem`'s `null`-target branch: the shooter's view vector
/// rotated `angle` degrees around `shooter.getUpVector(1.0)`, which vanilla
/// builds as `calculateViewVector(xRot - 90, yRot)`.
fn view_vector_offset(shooter: &SharedEntity, yaw: f32, pitch: f32, angle: f32) -> DVec3 {
    let view = shooter.calculate_view_vector(pitch, yaw);
    let up = shooter.calculate_view_vector(pitch - 90.0, yaw);
    view.rotate_axis(up.normalize_or_zero(), f64::from(angle.to_radians()))
}

/// Vanilla `CrossbowItem.getProjectileShotVector`.
fn projectile_shot_vector(shooter: &SharedEntity, direction: DVec3, angle: f32) -> DVec3 {
    let normalized = direction.normalize_or_zero();
    let mut axis = normalized.cross(DVec3::Y);
    if axis.length_squared() <= 1.0e-7 {
        let (yaw, pitch) = shooter.rotation();
        axis = normalized.cross(shooter.calculate_view_vector(pitch - 90.0, yaw));
    }
    let rotated = normalized.rotate_axis(axis.normalize_or_zero(), FRAC_PI_2);
    normalized.rotate_axis(rotated.normalize_or_zero(), f64::from(angle.to_radians()))
}

/// Vanilla `CrossbowItem.createProjectile`: a firework rocket keeps its own
/// flight, anything else goes through the shared `ArrowItem.createArrow` factory
/// (which applies the weapon's `projectile_piercing`) and gets the crossbow's
/// hit sound.
fn create_projectile(
    world: &Arc<World>,
    shooter: &SharedEntity,
    weapon: &ItemStack,
    projectile_stack: &ItemStack,
    is_crit: bool,
) -> SharedEntity {
    if projectile_stack.is(&vanilla_items::FIREWORK_ROCKET) {
        let position = shooter.position();
        let rocket = FireworkRocketEntity::launched(
            &vanilla_entities::FIREWORK_ROCKET,
            next_entity_id(),
            DVec3::new(
                position.x,
                shooter.get_eye_y() - FIREWORK_SPAWN_EYE_OFFSET,
                position.z,
            ),
            Arc::downgrade(world),
            projectile_stack.clone(),
        );
        rocket.set_shot_at_angle(true);
        rocket.set_owner_entity(Some(shooter));
        return Arc::new(rocket);
    }

    let arrow = create_arrow(world, projectile_stack, shooter, Some(weapon));
    if is_crit && let Some(arrow) = arrow.downcast_ref::<ArrowEntity>() {
        arrow.set_crit_arrow(true);
    }
    if let Some(arrow) = arrow.downcast_ref::<ArrowEntity>() {
        arrow
            .abstract_arrow_base()
            .set_sound_event(&sound_events::ITEM_CROSSBOW_HIT);
    } else if let Some(arrow) = arrow.downcast_ref::<SpectralArrowEntity>() {
        arrow
            .abstract_arrow_base()
            .set_sound_event(&sound_events::ITEM_CROSSBOW_HIT);
    }
    arrow
}

/// Vanilla `CrossbowItem.getDurabilityUse`: a firework rocket costs three uses.
fn durability_use(projectile_stack: &ItemStack) -> i32 {
    if projectile_stack.is(&vanilla_items::FIREWORK_ROCKET) {
        3
    } else {
        1
    }
}

/// Vanilla `CrossbowItem.getShotPitch`: index `0` always uses `1.0`.
fn get_shot_pitch(index: usize) -> f32 {
    if index == 0 {
        1.0
    } else {
        get_random_shot_pitch(index % 2 == 1)
    }
}

/// Vanilla `CrossbowItem.getRandomShotPitch`.
fn get_random_shot_pitch(is_left: bool) -> f32 {
    let base = if is_left { 0.63 } else { 0.43 };
    1.0 / (rand::random::<f32>() * 0.5 + 1.8) + base
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::data_components::vanilla_components::CHARGED_PROJECTILES;
    use steel_registry::items::ItemRef;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::types::InteractionHand;
    use steel_utils::{ChunkPos, Identifier, WorldAabb};

    use super::*;
    use crate::behavior::ITEM_BEHAVIORS;
    use crate::behavior::init_behaviors;
    use crate::entity::Entity as _;
    use crate::entity::entities::mobs::illager::PillagerEntity;
    use crate::inventory::container::Container as _;
    use crate::player::game_mode::use_item;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };

    const FIRE: (f32, f32) = (0.0_f32, 0.0_f32);

    fn fixture(name: &'static str) -> (TestWorld, Arc<Player>) {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world(name);
        insert_ready_full_chunk(&world_fixture.world, ChunkPos::new(0, 0));
        let player =
            TestPlayerBuilder::new(Arc::clone(&world_fixture.world), "Crossbow", 4242).build();

        let player_entity: SharedEntity = Arc::<Player>::clone(&player);
        player
            .try_set_position(DVec3::new(0.5, 65.0, 0.5))
            .expect("the fixture player starts inside the loaded chunk");
        player.set_rotation(FIRE);
        world_fixture
            .world
            .try_add_entity(Arc::clone(&player_entity))
            .expect("shooter joins the world");
        (world_fixture, player)
    }

    /// Puts a crossbow in the main hand and `ammo` in the first inventory slot.
    fn equip(player: &Player, crossbow: ItemStack, ammo: Option<(ItemRef, i32)>) -> ItemStack {
        let mut inventory = player.inventory.lock();
        inventory.set_selected_item(crossbow);
        if let Some((item, count)) = ammo {
            inventory.set_item(1, ItemStack::with_count(item, count));
        }
        inventory.get_selected_item().clone()
    }

    fn held(player: &Player) -> ItemStack {
        player.inventory.lock().get_selected_item().clone()
    }

    fn ammo_count(player: &Player, slot: usize) -> i32 {
        player.inventory.lock().get_item(slot).count()
    }

    /// Drives one full charge tick: the vanilla `onUseTick` call that happens when
    /// the use timer reaches `getChargeDuration`, then stores the loaded stack
    /// back into the player's main hand (vanilla mutates the stack in hand).
    fn charge_to_full(player: &Arc<Player>, crossbow: &mut ItemStack) {
        let entity: SharedEntity = Arc::<Player>::clone(player);
        let user = LivingEntityRef::new(&entity).expect("a player is a living entity");
        let world = player.get_world();
        let remaining = USE_DURATION - get_charge_duration(crossbow);
        ITEM_BEHAVIORS
            .get_behavior(&vanilla_items::CROSSBOW)
            .on_use_tick(&world, user, crossbow, remaining);
        player.inventory.lock().set_selected_item(crossbow.clone());
    }

    fn projectiles(world: &Arc<World>) -> Vec<SharedEntity> {
        world.get_entities_in_aabb_matching(
            &WorldAabb::new(-16.0, 55.0, -16.0, 16.0, 75.0, 16.0),
            |entity| {
                entity.entity_type() == &vanilla_entities::ARROW
                    || entity.entity_type() == &vanilla_entities::SPECTRAL_ARROW
                    || entity.entity_type() == &vanilla_entities::FIREWORK_ROCKET
            },
        )
    }

    #[test]
    fn charge_duration_matches_vanilla_with_and_without_quick_charge() {
        init_vanilla_registry();
        let crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
        assert_eq!(get_charge_duration(&crossbow), 25);
        assert!((power_for_time(12, &crossbow) - 0.48).abs() < 1e-6);
        assert_eq!(power_for_time(25, &crossbow), 1.0);
        assert_eq!(power_for_time(80, &crossbow), 1.0);

        // Vanilla Quick Charge: `-0.25` charge time per level, i.e. `25 - 5 * level`.
        for (level, expected) in [(1_u32, 20), (2, 15), (3, 10)] {
            let mut crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
            crossbow.set_enchantments(
                &[(Identifier::vanilla_static("quick_charge"), level)],
                false,
            );
            assert_eq!(
                get_charge_duration(&crossbow),
                expected,
                "quick_charge {level}"
            );
            assert_eq!(power_for_time(expected, &crossbow), 1.0);
            assert!(power_for_time(expected - 1, &crossbow) < 1.0);
        }
    }

    #[test]
    fn load_sounds_follow_the_vanilla_charge_thresholds() {
        assert_eq!(
            load_sounds(0.0, 0.0),
            LoadSounds {
                start: false,
                mid: false,
                end: false
            }
        );
        assert_eq!(
            load_sounds(0.15, 0.2),
            LoadSounds {
                start: true,
                mid: false,
                end: false
            }
        );
        assert_eq!(
            load_sounds(0.45, 0.52),
            LoadSounds {
                start: false,
                mid: true,
                end: false
            }
        );
        assert_eq!(
            load_sounds(0.96, 1.0),
            LoadSounds {
                start: false,
                mid: false,
                end: true
            }
        );
        // A tick that stays inside one band repeats nothing, and a finished
        // charge does not replay the end sound.
        assert_eq!(
            load_sounds(0.3, 0.36),
            LoadSounds {
                start: false,
                mid: false,
                end: false
            }
        );
        assert_eq!(
            load_sounds(1.0, 1.0),
            LoadSounds {
                start: false,
                mid: false,
                end: false
            }
        );
    }

    #[test]
    fn quick_charge_swaps_the_start_and_mid_load_sounds() {
        init_vanilla_registry();
        let mut crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
        let defaults = get_charging_sounds(&crossbow);
        assert_eq!(
            defaults.start,
            Some(&sound_events::ITEM_CROSSBOW_LOADING_START)
        );
        assert_eq!(
            defaults.mid,
            Some(&sound_events::ITEM_CROSSBOW_LOADING_MIDDLE)
        );
        assert_eq!(defaults.end, Some(&sound_events::ITEM_CROSSBOW_LOADING_END));

        // Vanilla `quick_charge` overrides only `start`/`end`, so the mid sound
        // is absent at every level (it picks the whole record at the highest level).
        crossbow.set_enchantments(&[(Identifier::vanilla_static("quick_charge"), 1)], true);
        let level_one = get_charging_sounds(&crossbow);
        assert_eq!(
            level_one.start,
            Some(&sound_events::ITEM_CROSSBOW_QUICK_CHARGE_1)
        );
        assert_eq!(level_one.mid, None);
        assert_eq!(
            level_one.end,
            Some(&sound_events::ITEM_CROSSBOW_LOADING_END)
        );

        crossbow.set_enchantments(&[(Identifier::vanilla_static("quick_charge"), 3)], true);
        let level_three = get_charging_sounds(&crossbow);
        assert_eq!(
            level_three.start,
            Some(&sound_events::ITEM_CROSSBOW_QUICK_CHARGE_3)
        );
        assert_eq!(level_three.mid, None);
    }

    #[test]
    fn full_charge_loads_one_arrow_into_the_component() {
        let (fixture, player) = fixture("crossbow_load_arrow");
        let mut crossbow = equip(
            &player,
            ItemStack::new(&vanilla_items::CROSSBOW),
            Some((&vanilla_items::ARROW, 16)),
        );
        assert!(!is_charged(&crossbow));

        charge_to_full(&player, &mut crossbow);

        let loaded = crossbow
            .get(CHARGED_PROJECTILES)
            .expect("a full charge loads the component");
        assert_eq!(loaded.items().len(), 1);
        assert_eq!(loaded.items()[0].item().key, vanilla_items::ARROW.key);
        assert!(is_charged(&crossbow));
        assert_eq!(ammo_count(&player, 1), 15, "loading consumes one arrow");

        // Vanilla `releaseUsing` only reports the fully charged crossbow.
        let behavior = ITEM_BEHAVIORS.get_behavior(&vanilla_items::CROSSBOW);
        let charge_duration = get_charge_duration(&crossbow);
        assert!(behavior.use_on_release(&crossbow));
        assert!(behavior.release_using(
            &mut crossbow,
            &fixture.world,
            player.as_ref(),
            USE_DURATION - charge_duration
        ));
        assert!(!behavior.release_using(
            &mut crossbow,
            &fixture.world,
            player.as_ref(),
            USE_DURATION - 2
        ));
    }

    #[test]
    fn firing_a_charged_crossbow_consumes_the_loaded_state() {
        let (fixture, player) = fixture("crossbow_fire_consumes_load");
        let world = &fixture.world;
        let mut crossbow = equip(
            &player,
            ItemStack::new(&vanilla_items::CROSSBOW),
            Some((&vanilla_items::ARROW, 16)),
        );
        charge_to_full(&player, &mut crossbow);
        assert!(is_charged(&held(&player)));

        // Vanilla fires on the next right-click (`CrossbowItem.use`).
        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Consume
        );

        let fired = held(&player);
        assert!(
            fired
                .get(CHARGED_PROJECTILES)
                .is_none_or(|projectiles| projectiles.items().is_empty()),
            "the shot clears the loaded projectiles"
        );
        assert_eq!(
            fired.get_damage_value(),
            1,
            "firing an arrow costs one durability"
        );
        assert_eq!(ammo_count(&player, 1), 15);

        let arrows = projectiles(world);
        assert_eq!(arrows.len(), 1);
        let velocity = arrows[0].velocity();
        assert!(
            (velocity.length() - f64::from(ARROW_POWER)).abs() < 0.1,
            "a crossbow arrow flies at 3.15: {velocity:?}"
        );
        assert!(
            velocity.z > 3.0 && velocity.x.abs() < 0.1 && velocity.y.abs() < 0.1,
            "the arrow leaves along the shooter's look direction: {velocity:?}"
        );

        // Vanilla `CrossbowItem.createProjectile` gives crossbow arrows their own
        // hit sound.
        let arrow = arrows[0]
            .downcast_ref::<ArrowEntity>()
            .expect("an arrow entity");
        assert_eq!(
            arrow.abstract_arrow_base().sound_event().key,
            sound_events::ITEM_CROSSBOW_HIT.key
        );
    }

    #[test]
    fn uncharged_use_starts_charging_and_needs_ammo() {
        let (world_fixture, player) = fixture("crossbow_start_charging");
        equip(&player, ItemStack::new(&vanilla_items::CROSSBOW), None);
        assert_eq!(
            use_item(&player, &world_fixture.world, InteractionHand::MainHand),
            InteractionResult::Fail,
            "an ammo-less crossbow cannot be charged"
        );
        assert_eq!(player.active_item_use_hand(), None);

        equip(
            &player,
            ItemStack::new(&vanilla_items::CROSSBOW),
            Some((&vanilla_items::ARROW, 1)),
        );
        assert_eq!(
            use_item(&player, &world_fixture.world, InteractionHand::MainHand),
            InteractionResult::Consume
        );
        assert_eq!(
            player.active_item_use_hand(),
            Some(InteractionHand::MainHand)
        );
        assert_eq!(
            ITEM_BEHAVIORS
                .get_behavior(&vanilla_items::CROSSBOW)
                .get_use_animation(&held(&player)),
            ItemUseAnimation::Crossbow
        );
    }

    #[test]
    fn multishot_fires_three_arrows_with_the_vanilla_spread() {
        let (fixture, player) = fixture("crossbow_multishot_spread");
        let world = &fixture.world;
        let mut crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
        crossbow.set_enchantments(&[(Identifier::vanilla_static("multishot"), 1)], true);
        let mut crossbow = equip(&player, crossbow, Some((&vanilla_items::ARROW, 16)));

        charge_to_full(&player, &mut crossbow);
        assert_eq!(
            crossbow
                .get(CHARGED_PROJECTILES)
                .expect("multishot loads the component")
                .items()
                .len(),
            3,
            "multishot loads three projectiles"
        );
        // Vanilla `ProjectileWeaponItem.draw` only consumes ammo for the first
        // projectile; the extras are intangible copies.
        assert_eq!(ammo_count(&player, 1), 15);

        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Consume
        );

        let mut arrows = projectiles(world);
        assert_eq!(arrows.len(), 3);
        let mut x_values: Vec<f64> = arrows.iter().map(|arrow| arrow.velocity().x).collect();
        x_values.sort_by(f64::total_cmp);
        // Vanilla's `-10 / 0 / +10` degree triple around the up vector, fired at
        // yaw 0 (looking down +Z) with `projectile_spread` 10. The shot
        // uncertainty jitters each axis, hence the tolerance.
        let ten_degrees = 10.0_f64.to_radians().sin() * f64::from(ARROW_POWER);
        let tolerance = 0.08;
        assert!(
            (x_values[0] + ten_degrees).abs() < tolerance
                && x_values[1].abs() < tolerance
                && (x_values[2] - ten_degrees).abs() < tolerance,
            "multishot spreads the arrows by -10/0/+10 degrees: {x_values:?}"
        );
        for arrow in &mut arrows {
            let velocity = arrow.velocity();
            assert!(
                (velocity.length() - f64::from(ARROW_POWER)).abs() < 0.1,
                "every spread arrow keeps the crossbow power: {velocity:?}"
            );
        }
    }

    #[test]
    fn piercing_sets_the_arrow_pierce_level() {
        let (fixture, player) = fixture("crossbow_piercing");
        let world = &fixture.world;
        let mut crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
        crossbow.set_enchantments(&[(Identifier::vanilla_static("piercing"), 4)], true);
        let mut crossbow = equip(&player, crossbow, Some((&vanilla_items::ARROW, 8)));

        charge_to_full(&player, &mut crossbow);
        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Consume
        );

        let arrows = projectiles(world);
        assert_eq!(arrows.len(), 1);
        let arrow = arrows[0]
            .downcast_ref::<ArrowEntity>()
            .expect("an arrow entity");
        assert_eq!(arrow.get_pierce_level(), 4);
    }

    #[test]
    fn firework_rockets_load_and_fire_at_vanilla_power() {
        let (fixture, player) = fixture("crossbow_firework_rocket");
        let world = &fixture.world;
        let mut crossbow = equip(
            &player,
            ItemStack::new(&vanilla_items::CROSSBOW),
            Some((&vanilla_items::FIREWORK_ROCKET, 8)),
        );

        charge_to_full(&player, &mut crossbow);
        let loaded = crossbow.get(CHARGED_PROJECTILES).expect("rockets load");
        assert_eq!(loaded.items().len(), 1);
        assert_eq!(
            loaded.items()[0].item().key,
            vanilla_items::FIREWORK_ROCKET.key
        );
        assert_eq!(shooting_power(loaded), FIREWORK_POWER);

        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Consume
        );

        let rockets = projectiles(world);
        assert_eq!(rockets.len(), 1);
        assert_eq!(rockets[0].entity_type(), &vanilla_entities::FIREWORK_ROCKET);
        let velocity = rockets[0].velocity();
        assert!(
            (velocity.length() - f64::from(FIREWORK_POWER)).abs() < 0.1,
            "a fired rocket flies at 1.6: {velocity:?}"
        );
        let position = rockets[0].position();
        assert!(
            (position.y - (player.get_eye_y() - FIREWORK_SPAWN_EYE_OFFSET)).abs() < 1e-9
                && (position.x - player.position().x).abs() < 1e-9
                && (position.z - player.position().z).abs() < 1e-9,
            "the rocket spawns below the shooter's eyes: {position:?}"
        );
        assert_eq!(
            held(&player).get_damage_value(),
            3,
            "firing a rocket costs three durability"
        );
    }

    #[test]
    fn shot_crossbow_is_raised_once_per_load_and_never_for_an_empty_shot() {
        let (fixture, player) = fixture("crossbow_shot_crossbow_advancement");
        let world = &fixture.world;
        let mut crossbow = equip(
            &player,
            ItemStack::new(&vanilla_items::CROSSBOW),
            Some((&vanilla_items::ARROW, 16)),
        );

        // An unloaded crossbow has nothing to shoot, so vanilla returns before
        // `performShooting` raises the trigger.
        let mut empty = ItemStack::new(&vanilla_items::CROSSBOW);
        let shooter: SharedEntity = Arc::<Player>::clone(&player);
        perform_shooting(
            world,
            &shooter,
            &mut empty,
            ARROW_POWER,
            SHOOT_UNCERTAINTY,
            None,
        );
        assert_eq!(shot_crossbow_criteria(&player), 0);

        charge_to_full(&player, &mut crossbow);
        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Consume
        );
        assert_eq!(
            shot_crossbow_criteria(&player),
            1,
            "`adventure/ol_betsy` completes from the crossbow shot"
        );

        // A second load and shot reports the same completed criterion.
        let mut reloaded = held(&player);
        charge_to_full(&player, &mut reloaded);
        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Consume
        );
        assert_eq!(shot_crossbow_criteria(&player), 1);
    }

    /// Returns how many completed `shot_crossbow` criteria the player has.
    fn shot_crossbow_criteria(player: &Player) -> usize {
        let advancement = REGISTRY
            .advancements
            .value_by_key(&Identifier::vanilla_static("adventure/ol_betsy"))
            .expect("`adventure/ol_betsy` exists");
        player
            .advancements
            .lock()
            .progress
            .get_progress(advancement)
            .map_or(0, |progress| {
                progress
                    .get_completed_criteria()
                    .filter(|name| *name == "shot_crossbow")
                    .count()
            })
    }

    #[test]
    fn loaded_projectiles_survive_a_drop_and_relog_stack_round_trip() {
        use simdnbt::FromNbtTag as _;
        use simdnbt::borrow::read_tag;
        use std::io::Cursor;
        use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
        use text_components::TextComponent;

        let (_fixture, player) = fixture("crossbow_component_round_trip");
        let mut tipped = ItemStack::with_count(&vanilla_items::TIPPED_ARROW, 1);
        tipped.set(CUSTOM_NAME, TextComponent::plain("loaded".to_owned()));

        let mut crossbow = equip(&player, ItemStack::new(&vanilla_items::CROSSBOW), None);
        player.inventory.lock().set_item(1, tipped);
        charge_to_full(&player, &mut crossbow);
        assert!(is_charged(&crossbow));
        assert_eq!(ammo_count(&player, 1), 0, "loading consumes the arrow");

        // The loaded state must survive the persistence codec unchanged.
        let tag = crossbow.to_nbt_tag_ref();
        let mut bytes = Vec::new();
        tag.write(&mut bytes);
        let borrowed = read_tag(&mut Cursor::new(bytes.as_slice())).expect("NBT parses");
        let restored = ItemStack::from_nbt_tag(borrowed.as_tag()).expect("stack NBT should decode");

        let projectiles = restored
            .get(CHARGED_PROJECTILES)
            .expect("the loaded state persists");
        assert_eq!(projectiles.items().len(), 1);
        assert_eq!(
            projectiles.items()[0].item().key,
            vanilla_items::TIPPED_ARROW.key
        );
        assert_eq!(projectiles.items()[0].count(), 1);
        let reloaded = projectiles.items()[0].create();
        assert!(
            reloaded.get(CUSTOM_NAME).is_some(),
            "the loaded arrow keeps its component patch across the round trip"
        );
        assert_eq!(
            restored.get_damage_value(),
            0,
            "the round trip keeps the crossbow's own state"
        );
        assert!(is_charged(&restored), "a relogged crossbow is still loaded");
    }

    #[test]
    fn ammo_predicate_accepts_arrows_and_fireworks_only() {
        use crate::behavior::items::bow::is_supported_projectile;

        init_vanilla_registry();
        assert!(is_crossbow_ammo(&ItemStack::new(&vanilla_items::ARROW)));
        assert!(is_crossbow_ammo(&ItemStack::new(
            &vanilla_items::SPECTRAL_ARROW
        )));
        assert!(is_crossbow_ammo(&ItemStack::new(
            &vanilla_items::FIREWORK_ROCKET
        )));
        assert!(!is_crossbow_ammo(&ItemStack::new(&vanilla_items::STICK)));
        assert!(!is_crossbow_ammo(&ItemStack::empty()));
        // The bow keeps its arrow-only predicate.
        assert!(is_supported_projectile(&ItemStack::new(
            &vanilla_items::ARROW
        )));
        assert!(!is_crossbow_ammo(&ItemStack::new(&vanilla_items::SHIELD)));
    }

    /// A monster shooter: the fixture world plus a pillager standing next to a
    /// player. Vanilla `Monster.getProjectile` never consults an inventory, so
    /// the co-located player is what proves the mob draw stays inside the mob.
    fn mob_fixture(
        name: &'static str,
    ) -> (TestWorld, Arc<Player>, Arc<PillagerEntity>, SharedEntity) {
        let (world_fixture, player) = fixture(name);
        let world = &world_fixture.world;
        let pillager = Arc::new(PillagerEntity::new(
            &vanilla_entities::PILLAGER,
            4343,
            DVec3::new(2.5, 65.0, 0.5),
            Arc::downgrade(world),
        ));
        let body: SharedEntity = Arc::<PillagerEntity>::clone(&pillager);
        world
            .try_add_entity(Arc::clone(&body))
            .expect("the shooter joins the world");
        (world_fixture, player, pillager, body)
    }

    fn equip_mob_crossbow(pillager: &PillagerEntity, crossbow: ItemStack) {
        pillager.set_item_in_hand(InteractionHand::MainHand, crossbow);
    }

    fn held_mob_crossbow(pillager: &PillagerEntity) -> ItemStack {
        pillager.get_item_in_hand(InteractionHand::MainHand)
    }

    /// Ticks the mob's active item use through vanilla `LivingEntity.updateUsingItem`,
    /// the path `Mob.serverAiStep` drives on every mob tick.
    fn tick_mob_charge(pillager: &PillagerEntity, body: &SharedEntity, ticks: i32) {
        for _ in 0..ticks {
            pillager.updating_using_item(body);
        }
    }

    /// Charges exactly the way the crossbow goals do: one `updateUsingItem` per
    /// mob tick and a `releaseUsingItem` once `getTicksUsingItem` reaches the
    /// crossbow's charge duration. The release tick is what loads a mob's
    /// crossbow, because `CrossbowItem.useOnRelease` re-ticks the use (with the
    /// same remaining ticks) before it stops.
    fn charge_and_release(pillager: &PillagerEntity, body: &SharedEntity, ticks: i32) {
        pillager.start_using_item(InteractionHand::MainHand);
        tick_mob_charge(pillager, body, ticks);
        pillager.release_using_item(body);
        assert!(!pillager.is_using_item(), "the release ends the charge");
    }

    #[test]
    fn mob_charge_loads_the_declared_arrow_without_touching_a_player() {
        let (_fixture, player, pillager, body) = mob_fixture("mob_crossbow_load");
        // The player holds the same ammo a mob would silently steal in the buggy
        // shared-helper shape.
        player
            .inventory
            .lock()
            .set_item(1, ItemStack::with_count(&vanilla_items::ARROW, 16));
        equip_mob_crossbow(&pillager, ItemStack::new(&vanilla_items::CROSSBOW));

        let duration = get_charge_duration(&held_mob_crossbow(&pillager));
        assert_eq!(duration, 25);

        // One tick short of the vanilla duration leaves the crossbow unloaded.
        charge_and_release(&pillager, &body, duration - 1);
        assert!(
            !is_charged(&held_mob_crossbow(&pillager)),
            "24 of the vanilla 25 ticks is not a full charge"
        );

        // The full vanilla duration loads exactly one declared arrow.
        charge_and_release(&pillager, &body, duration);
        let loaded = held_mob_crossbow(&pillager);
        let projectiles = loaded
            .get(CHARGED_PROJECTILES)
            .expect("a full charge loads the component");
        assert_eq!(
            projectiles.items().len(),
            1,
            "a plain mob crossbow loads exactly one projectile"
        );
        assert_eq!(projectiles.items()[0].item().key, vanilla_items::ARROW.key);

        assert_eq!(
            ammo_count(&player, 1),
            16,
            "a mob never draws from a co-located player's inventory"
        );
        assert!(held(&player).is_empty(), "the player's hand is untouched");
    }

    #[test]
    fn mob_charge_duration_follows_quick_charge() {
        for (key, level, expected) in [
            ("mob_crossbow_charge_plain", None, 25),
            ("mob_crossbow_charge_quick_1", Some(1), 20),
            ("mob_crossbow_charge_quick_2", Some(2), 15),
            ("mob_crossbow_charge_quick_3", Some(3), 10),
        ] {
            let (_fixture, _player, pillager, body) = mob_fixture(key);
            let mut crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
            if let Some(level) = level {
                crossbow.set_enchantments(
                    &[(Identifier::vanilla_static("quick_charge"), level)],
                    false,
                );
            }
            equip_mob_crossbow(&pillager, crossbow);
            assert_eq!(
                get_charge_duration(&held_mob_crossbow(&pillager)),
                expected,
                "quick charge {level:?}"
            );

            charge_and_release(&pillager, &body, expected - 1);
            assert!(
                !is_charged(&held_mob_crossbow(&pillager)),
                "quick charge {level:?} is not full one tick early"
            );

            charge_and_release(&pillager, &body, expected);
            assert!(
                is_charged(&held_mob_crossbow(&pillager)),
                "quick charge {level:?} loads on the vanilla duration"
            );
        }
    }

    #[test]
    fn mob_loading_spends_matching_ammo_held_in_the_off_hand() {
        let (_fixture, _player, pillager, body) = mob_fixture("mob_crossbow_offhand_ammo");
        // Vanilla `Monster.getProjectile` checks `getHeldProjectile` before its
        // plain-arrow default, so a rocket in the off hand is what loads.
        pillager.set_item_in_hand(
            InteractionHand::OffHand,
            ItemStack::with_count(&vanilla_items::FIREWORK_ROCKET, 5),
        );
        equip_mob_crossbow(&pillager, ItemStack::new(&vanilla_items::CROSSBOW));

        charge_and_release(&pillager, &body, 25);

        let loaded = held_mob_crossbow(&pillager);
        let projectiles = loaded
            .get(CHARGED_PROJECTILES)
            .expect("the held rocket loads");
        assert_eq!(
            projectiles.items()[0].item().key,
            vanilla_items::FIREWORK_ROCKET.key
        );
        assert_eq!(
            pillager.get_item_in_hand(InteractionHand::OffHand).count(),
            4,
            "vanilla `useAmmo` splits the loaded rocket out of the held stack"
        );
    }

    #[test]
    fn mob_multishot_loads_intangible_copies_of_the_declared_arrow() {
        let (_fixture, _player, pillager, body) = mob_fixture("mob_crossbow_multishot");
        let mut crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
        crossbow.set_enchantments(&[(Identifier::vanilla_static("multishot"), 1)], true);
        equip_mob_crossbow(&pillager, crossbow);

        charge_and_release(&pillager, &body, 25);

        let loaded = held_mob_crossbow(&pillager);
        let projectiles = loaded
            .get(CHARGED_PROJECTILES)
            .expect("multishot loads the component");
        assert_eq!(projectiles.items().len(), 3, "multishot loads three bolts");
        assert!(
            projectiles
                .items()
                .iter()
                .all(|template| template.item().key == vanilla_items::ARROW.key)
        );
        // Vanilla's `useAmmo` marks every projectile after the first as an
        // intangible copy, so the declared default is not consumed three times.
        assert!(
            projectiles.items()[1].create().has(INTANGIBLE_PROJECTILE),
            "the copies beyond the first are intangible"
        );
    }

    #[test]
    fn mob_crossbow_fires_at_mob_arrow_power_and_clears_the_load() {
        let (fixture, player, pillager, body) = mob_fixture("mob_crossbow_fire");
        let world = &fixture.world;
        equip_mob_crossbow(&pillager, ItemStack::new(&vanilla_items::CROSSBOW));
        charge_and_release(&pillager, &body, 25);
        assert!(is_charged(&held_mob_crossbow(&pillager)));

        let target: SharedEntity = Arc::<Player>::clone(&player);
        assert!(perform_mob_crossbow_attack(
            world,
            &body,
            &target,
            MOB_ARROW_POWER,
            SHOOT_UNCERTAINTY,
        ));

        let fired = held_mob_crossbow(&pillager);
        assert!(
            fired
                .get(CHARGED_PROJECTILES)
                .is_none_or(|projectiles| projectiles.items().is_empty()),
            "the shot clears the loaded projectiles"
        );
        assert_eq!(
            fired.get_damage_value(),
            1,
            "firing an arrow costs one durability"
        );

        let arrows = projectiles(world);
        assert_eq!(arrows.len(), 1);
        let velocity = arrows[0].velocity();
        assert!(
            (velocity.length() - f64::from(MOB_ARROW_POWER)).abs() < 0.2,
            "a mob bolt flies at `MOB_ARROW_POWER` (1.6): {velocity:?}"
        );
    }

    #[test]
    fn uncharged_mob_crossbow_releases_nothing() {
        let (fixture, player, pillager, body) = mob_fixture("mob_crossbow_uncharged");
        let world = &fixture.world;
        equip_mob_crossbow(&pillager, ItemStack::new(&vanilla_items::CROSSBOW));

        let target: SharedEntity = Arc::<Player>::clone(&player);
        assert!(
            !perform_mob_crossbow_attack(world, &body, &target, MOB_ARROW_POWER, SHOOT_UNCERTAINTY,),
            "an unloaded crossbow releases no bolt"
        );
        assert!(projectiles(world).is_empty());
        assert_eq!(
            held_mob_crossbow(&pillager).get_damage_value(),
            0,
            "an empty crossbow is not damaged"
        );
    }
}

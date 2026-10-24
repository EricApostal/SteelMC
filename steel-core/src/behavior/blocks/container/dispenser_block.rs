//! Dispenser and dropper block behaviors.
//!
//! Both blocks are nine-slot container blocks that share a facing state and a
//! redstone trigger. A dispenser runs the item-specific
//! [`DispenseMethod`] table; a dropper always ejects through the default
//! behavior or pushes into the container it faces.

use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_macros::block_behavior;
use steel_math::convert_to_rotation_segment;
use steel_registry::RegistryReference;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_registry::data_components::components::PotionContents;
use steel_registry::data_components::vanilla_components::{ENTITY_DATA, POTION_CONTENTS};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::stat::custom::CustomStatRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{
    REGISTRY, level_events, sound_events, vanilla_block_entity_types, vanilla_blocks,
    vanilla_custom_stats, vanilla_entities, vanilla_game_events, vanilla_game_rules, vanilla_items,
    vanilla_particle_types, vanilla_potions,
};
use steel_utils::Downcast as _;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, WorldAabb, translations};
use text_components::TextComponent;

use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::blocks::WitherSkullBlock;
use crate::behavior::blocks::portal::FireBlock;
use crate::behavior::blocks::special::TntBlock;
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::behavior::items::BlockItem;
use crate::behavior::waxables::get_waxed_from_normal_variant;
use crate::behavior::{BLOCK_BEHAVIORS, ITEM_BEHAVIORS, InventoryAccess};
use crate::block_entity::BLOCK_ENTITIES;
use crate::block_entity::entities::insert_item_into;
use crate::entity::entities::ArmadilloEntity;
use crate::entity::entities::ItemEntity;
use crate::entity::projectile::triangle_random;
use crate::entity::{
    EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, next_entity_id, spawn_entity,
};
use crate::inventory::container::calculate_redstone_signal_from_container;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};
use crate::inventory::menu::kinds::dispenser;
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, SignalGetter as _, World};

const FACING: &EnumProperty<Direction> = &BlockStateProperties::FACING;
const TRIGGERED: &BoolProperty = &BlockStateProperties::TRIGGERED;

/// Vanilla `DispenserBlock.TRIGGER_DURATION`: ticks between the rising edge and
/// the dispense.
const TRIGGER_DURATION: i32 = 4;

/// Vanilla `DefaultDispenseItemBehavior.DEFAULT_ACCURACY`: spread applied to the
/// ejected item's velocity.
const DEFAULT_ACCURACY: i32 = 6;

/// Vanilla `DispenserBlock.getDispensePosition` places the ejected item 0.7
/// blocks in front of the dispenser's center.
const DISPENSE_POSITION_SCALE: f64 = 0.7;

/// Dispenser block behavior.
#[block_behavior]
pub struct DispenserBlock {
    block: BlockRef,
}

/// Dropper block behavior.
///
/// A dropper shares the dispenser's trigger and container but always uses the
/// default eject behavior, or pushes into the container it faces.
#[block_behavior]
pub struct DropperBlock {
    block: BlockRef,
}

impl DispenserBlock {
    /// Creates a new dispenser block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl DropperBlock {
    /// Creates a new dropper block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

/// One dispenser/dropper gathering a single item.
///
/// Mirrors vanilla `BlockSource`. It borrows the world and the block entity's
/// container reference so the dispense behaviors never hold a container lock
/// while calling into the world.
struct DispenseSource<'a> {
    world: &'a Arc<World>,
    pos: BlockPos,
    state: BlockStateId,
    container: &'a ContainerRef,
}

impl DispenseSource<'_> {
    /// The direction the dispenser faces.
    fn facing(&self) -> Direction {
        self.state.get_value(FACING)
    }

    /// The block directly in front of the dispenser.
    fn target(&self) -> BlockPos {
        self.facing().relative(self.pos)
    }

    /// Vanilla `BlockSource.center`.
    fn center(&self) -> DVec3 {
        DVec3::new(
            f64::from(self.pos.x()) + 0.5,
            f64::from(self.pos.y()) + 0.5,
            f64::from(self.pos.z()) + 0.5,
        )
    }

    /// Vanilla `DispenserBlock.getDispensePosition`.
    fn dispense_position(&self, scale: f64, offset: DVec3) -> DVec3 {
        let (dx, dy, dz) = self.facing().offset();
        self.center()
            + DVec3::new(
                scale * f64::from(dx) + offset.x,
                scale * f64::from(dy) + offset.y,
                scale * f64::from(dz) + offset.z,
            )
    }

    /// Plays a dispenser level event at the block position.
    fn level_event(&self, event_type: i32) {
        self.world.level_event(event_type, self.pos, 0, None);
    }

    /// Vanilla `DefaultDispenseItemBehavior.playDefaultAnimation`: the
    /// smoke-puff particle event carrying the facing data.
    fn play_animation(&self) {
        self.world.level_event(
            level_events::PARTICLES_SHOOT_SMOKE,
            self.pos,
            self.facing().get_3d_data_value(),
            None,
        );
    }

    /// Vanilla `BlockSource.blockEntity().insertItem`: merges the remainder back
    /// into the dispenser's own slots.
    fn insert_item(&self, stack: ItemStack) -> ItemStack {
        if stack.is_empty() {
            return stack;
        }
        let mut guard = ContainerLockGuard::lock_all(&[self.container]);
        match guard.get_mut(self.container.container_id()) {
            Some(container) => insert_item_into(container, stack),
            None => stack,
        }
    }

    /// Vanilla `DispenserBlockEntity.getRandomSlot`: a uniformly random
    /// non-empty slot, chosen by reservoir sampling.
    fn random_slot(&self) -> Option<(usize, ItemStack)> {
        let guard = ContainerLockGuard::lock_all(&[self.container]);
        let container = guard.get(self.container.container_id())?;
        let mut replace_slot = None;
        let mut replace_odds = 1;
        for slot in 0..container.get_container_size() {
            if !container.get_item(slot).is_empty() {
                if rand::random_range(0..replace_odds) == 0 {
                    replace_slot = Some(slot);
                }
                replace_odds += 1;
            }
        }
        let slot = replace_slot?;
        Some((slot, container.get_item(slot).clone()))
    }

    /// Writes a slot back after a dispense, releasing the lock before returning.
    fn set_slot(&self, slot: usize, stack: ItemStack) {
        let mut guard = ContainerLockGuard::lock_all(&[self.container]);
        guard.set_item(self.container.container_id(), slot, stack);
    }
}

/// Sound and animation side effects a vanilla `DispenseItemBehavior` emits after
/// its execute step.
enum DispenseFeedback {
    /// `DefaultDispenseItemBehavior`: always the dispense sound.
    Default,
    /// `OptionalDispenseItemBehavior`: success or failure sound.
    Optional(bool),
    /// `ProjectileDispenseBehavior`: the launch event or the item's override.
    Projectile(Option<i32>),
}

/// Vanilla `DispenserBlock.DISPENSER_REGISTRY` entry selected for an item.
///
/// Entries whose vanilla behavior needs a Steel API that does not exist yet are
/// not listed here and therefore fall back to [`DispenseMethod::Default`]. See
/// [`dispense_method`] for the list and the owner of each missing API.
enum DispenseMethod {
    /// Vanilla `DefaultDispenseItemBehavior`.
    Default,
    /// Vanilla `ProjectileDispenseBehavior`.
    Projectile,
    /// Vanilla `ArmorStand` entry.
    ArmorStand,
    /// Vanilla `FlintAndSteelDispenseItemBehavior`.
    FlintAndSteel,
    /// Vanilla bone-meal entry.
    BoneMeal,
    /// Vanilla TNT entry.
    Tnt,
    /// Vanilla carved-pumpkin entry.
    CarvedPumpkin,
    /// Vanilla wither-skeleton-skull entry.
    WitherSkull,
    /// Vanilla `ShulkerBoxDispenseBehavior`.
    ShulkerBox,
    /// Vanilla empty-bucket entry.
    EmptyBucket,
    /// Vanilla glass-bottle entry.
    GlassBottle,
    /// Vanilla brush entry.
    Brush,
    /// Vanilla honeycomb entry.
    Honeycomb,
    /// Vanilla water-potion entry.
    WaterPotion,
    /// Vanilla `SpawnEggItemBehavior`.
    SpawnEgg,
    /// Vanilla `EquipmentDispenseItemBehavior`.
    Equipment,
}

/// Matches an item against a set of vanilla item statics.
macro_rules! is_any_of {
    ($item:expr, $($candidate:expr),+ $(,)?) => {
        $(std::ptr::eq($item, &*$candidate))||+
    };
}

/// Selects the vanilla dispense method for a stack.
///
/// Vanilla entries that cannot run yet because their Steel dependency is missing
/// fall through to [`DispenseMethod::Default`]; each is reported in this slice's
/// hand-off notes rather than silently emulated:
///
/// * boats and rafts — no boat entity implementation,
/// * minecarts — only the chest minecart entity exists,
/// * filled buckets — no position-based `DispensibleContainerItem.emptyContents`,
/// * chest — no chested-horse chest slot,
/// * shears — no `Shearable` entity capability or beehive shearing,
/// * glowstone — `RespawnAnchorBlock::charge` is private to its module.
fn dispense_method(stack: &ItemStack) -> DispenseMethod {
    let item = stack.item();

    if is_any_of!(
        item,
        vanilla_items::ARROW,
        vanilla_items::TIPPED_ARROW,
        vanilla_items::SPECTRAL_ARROW,
        vanilla_items::EGG,
        vanilla_items::BLUE_EGG,
        vanilla_items::BROWN_EGG,
        vanilla_items::SNOWBALL,
        vanilla_items::EXPERIENCE_BOTTLE,
        vanilla_items::SPLASH_POTION,
        vanilla_items::LINGERING_POTION,
        vanilla_items::FIREWORK_ROCKET,
        vanilla_items::FIRE_CHARGE,
        vanilla_items::WIND_CHARGE,
    ) {
        return DispenseMethod::Projectile;
    }

    if std::ptr::eq(item, &*vanilla_items::ARMOR_STAND) {
        return DispenseMethod::ArmorStand;
    }
    if std::ptr::eq(item, &*vanilla_items::FLINT_AND_STEEL) {
        return DispenseMethod::FlintAndSteel;
    }
    if std::ptr::eq(item, &*vanilla_items::BONE_MEAL) {
        return DispenseMethod::BoneMeal;
    }
    if std::ptr::eq(item, &*vanilla_items::TNT) {
        return DispenseMethod::Tnt;
    }
    if std::ptr::eq(item, &*vanilla_items::CARVED_PUMPKIN) {
        return DispenseMethod::CarvedPumpkin;
    }
    if std::ptr::eq(item, &*vanilla_items::WITHER_SKELETON_SKULL) {
        return DispenseMethod::WitherSkull;
    }
    if is_shulker_box(item) {
        return DispenseMethod::ShulkerBox;
    }
    if std::ptr::eq(item, &*vanilla_items::BUCKET) {
        return DispenseMethod::EmptyBucket;
    }
    if std::ptr::eq(item, &*vanilla_items::GLASS_BOTTLE) {
        return DispenseMethod::GlassBottle;
    }
    if std::ptr::eq(item, &*vanilla_items::BRUSH) {
        return DispenseMethod::Brush;
    }
    if std::ptr::eq(item, &*vanilla_items::HONEYCOMB) {
        return DispenseMethod::Honeycomb;
    }
    if std::ptr::eq(item, &*vanilla_items::POTION) {
        return DispenseMethod::WaterPotion;
    }

    // Vanilla `DispenserBlock.getDefaultDispenseMethod`.
    if stack.has(steel_registry::data_components::vanilla_components::EQUIPPABLE) {
        return DispenseMethod::Equipment;
    }
    if item.block.is_none()
        && ITEM_BEHAVIORS.get_behavior(item).as_spawn_egg().is_some()
        && stack.has(ENTITY_DATA)
    {
        return DispenseMethod::SpawnEgg;
    }

    DispenseMethod::Default
}

/// Returns whether `item` is any shulker box variant.
fn is_shulker_box(item: ItemRef) -> bool {
    is_any_of!(
        item,
        vanilla_items::SHULKER_BOX,
        vanilla_items::WHITE_SHULKER_BOX,
        vanilla_items::ORANGE_SHULKER_BOX,
        vanilla_items::MAGENTA_SHULKER_BOX,
        vanilla_items::LIGHT_BLUE_SHULKER_BOX,
        vanilla_items::YELLOW_SHULKER_BOX,
        vanilla_items::LIME_SHULKER_BOX,
        vanilla_items::PINK_SHULKER_BOX,
        vanilla_items::GRAY_SHULKER_BOX,
        vanilla_items::LIGHT_GRAY_SHULKER_BOX,
        vanilla_items::CYAN_SHULKER_BOX,
        vanilla_items::PURPLE_SHULKER_BOX,
        vanilla_items::BLUE_SHULKER_BOX,
        vanilla_items::BROWN_SHULKER_BOX,
        vanilla_items::GREEN_SHULKER_BOX,
        vanilla_items::RED_SHULKER_BOX,
        vanilla_items::BLACK_SHULKER_BOX,
    )
}

/// Runs one dispense, including the behavior's sound and animation.
fn dispense(source: &DispenseSource<'_>, stack: ItemStack) -> ItemStack {
    let method = dispense_method(&stack);
    let (result, feedback) = match method {
        DispenseMethod::Default => (default_dispense(source, stack), DispenseFeedback::Default),
        DispenseMethod::Projectile => dispense_projectile(source, stack),
        DispenseMethod::ArmorStand => (
            dispense_armor_stand(source, stack),
            DispenseFeedback::Default,
        ),
        DispenseMethod::FlintAndSteel => dispense_flint_and_steel(source, stack),
        DispenseMethod::BoneMeal => dispense_bone_meal(source, stack),
        DispenseMethod::Tnt => dispense_tnt(source, stack),
        DispenseMethod::CarvedPumpkin => dispense_carved_pumpkin(source, stack),
        DispenseMethod::WitherSkull => dispense_wither_skull(source, stack),
        DispenseMethod::ShulkerBox => dispense_shulker_box(source, stack),
        DispenseMethod::EmptyBucket => dispense_empty_bucket(source, stack),
        DispenseMethod::GlassBottle => dispense_glass_bottle(source, stack),
        DispenseMethod::Brush => dispense_brush(source, stack),
        DispenseMethod::Honeycomb => dispense_honeycomb(source, stack),
        DispenseMethod::WaterPotion => dispense_water_potion(source, stack),
        DispenseMethod::SpawnEgg => (dispense_spawn_egg(source, stack), DispenseFeedback::Default),
        DispenseMethod::Equipment => (dispense_equipment(source, stack), DispenseFeedback::Default),
    };

    match feedback {
        DispenseFeedback::Default | DispenseFeedback::Optional(true) => {
            source.level_event(level_events::SOUND_DISPENSER_DISPENSE);
        }
        DispenseFeedback::Optional(false) => {
            source.level_event(level_events::SOUND_DISPENSER_FAIL);
        }
        DispenseFeedback::Projectile(override_event) => source
            .level_event(override_event.unwrap_or(level_events::SOUND_DISPENSER_PROJECTILE_LAUNCH)),
    }
    source.play_animation();
    result
}

/// Vanilla `DefaultDispenseItemBehavior.execute`: ejects one item.
fn default_dispense(source: &DispenseSource<'_>, mut stack: ItemStack) -> ItemStack {
    let direction = source.facing();
    let position = source.dispense_position(DISPENSE_POSITION_SCALE, DVec3::ZERO);
    let ejected = stack.split(1);
    spawn_item(source.world, ejected, DEFAULT_ACCURACY, direction, position);
    stack
}

/// Vanilla `DefaultDispenseItemBehavior.spawnItem`.
fn spawn_item(
    world: &Arc<World>,
    item: ItemStack,
    accuracy: i32,
    direction: Direction,
    position: DVec3,
) {
    let (step_x, _step_y, step_z) = direction.offset();
    let mut spawn_y = position.y;
    if direction.get_axis() == steel_utils::axis::Axis::Y {
        spawn_y -= 0.125;
    } else {
        spawn_y -= 0.15625;
    }

    let spread = 0.017_227_5 * f64::from(accuracy);
    let pow = rand::random::<f64>() * 0.1 + 0.2;
    let velocity = DVec3::new(
        triangle_random(f64::from(step_x) * pow, spread),
        triangle_random(0.2, spread),
        triangle_random(f64::from(step_z) * pow, spread),
    );

    let entity = ItemEntity::with_item_and_velocity(
        &vanilla_entities::ITEM,
        next_entity_id(),
        DVec3::new(position.x, spawn_y, position.z),
        item,
        velocity,
        Arc::downgrade(world),
    );
    if world.try_add_entity(Arc::new(entity)).is_err() {
        log::warn!("Failed to eject item from dispenser at {:?}", position);
    }
}

/// Vanilla `DefaultDispenseItemBehavior.consumeWithRemainder`.
fn consume_with_remainder(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
    remainder: ItemStack,
) -> ItemStack {
    stack.shrink(1);
    if stack.is_empty() {
        return remainder;
    }
    add_to_inventory_or_dispense(source, remainder);
    stack
}

/// Vanilla `DefaultDispenseItemBehavior.addToInventoryOrDispense`.
fn add_to_inventory_or_dispense(source: &DispenseSource<'_>, stack: ItemStack) {
    if stack.is_empty() {
        return;
    }
    let remainder = source.insert_item(stack);
    if remainder.is_empty() {
        return;
    }
    let direction = source.facing();
    spawn_item(
        source.world,
        remainder,
        DEFAULT_ACCURACY,
        direction,
        source.dispense_position(DISPENSE_POSITION_SCALE, DVec3::ZERO),
    );
    source.level_event(level_events::SOUND_DISPENSER_DISPENSE);
    source.play_animation();
}

/// Vanilla `ProjectileDispenseBehavior`.
fn dispense_projectile(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let direction = source.facing();
    let position = source.dispense_position(DISPENSE_POSITION_SCALE, DVec3::ZERO);
    let behavior = ITEM_BEHAVIORS.get_behavior(stack.item());
    let Some(projectile_item) = behavior.as_projectile_item() else {
        return (
            default_dispense(source, stack),
            DispenseFeedback::Projectile(None),
        );
    };
    let config = projectile_item.create_dispense_config();
    let Some(entity) = projectile_item.as_projectile(source.world, position, &stack, direction)
    else {
        return (
            stack,
            DispenseFeedback::Projectile(config.override_dispense_event),
        );
    };

    let (step_x, step_y, step_z) = direction.offset();
    crate::entity::projectile::shoot_projectile_entity(
        source.world,
        &entity,
        &mut stack,
        DVec3::new(f64::from(step_x), f64::from(step_y), f64::from(step_z)),
        config.power,
        config.uncertainty,
    );
    stack.shrink(1);
    (
        stack,
        DispenseFeedback::Projectile(config.override_dispense_event),
    )
}

/// Vanilla armor-stand entry: spawns a stand facing the dispenser's direction.
fn dispense_armor_stand(source: &DispenseSource<'_>, stack: ItemStack) -> ItemStack {
    let direction = source.facing();
    let request = EntitySpawnRequest {
        entity_type: &vanilla_entities::ARMOR_STAND,
        placement: EntitySpawnPlacement::Block {
            pos: source.target(),
            try_move_down: false,
            moved_up: false,
        },
        reason: EntitySpawnReason::Dispenser,
        finalize_spawn: false,
        play_ambient_sound: false,
        item_stack: Some(&stack),
        user_is_operator: false,
    };
    let Ok(entity) = spawn_entity(source.world, request) else {
        return stack;
    };
    entity.set_rotation((direction.to_yaw(), 0.0));

    let mut stack = stack;
    stack.shrink(1);
    stack
}

/// Vanilla `FlintAndSteelDispenseItemBehavior`.
fn dispense_flint_and_steel(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let world = source.world;
    let facing = source.facing();
    let target = source.target();
    let target_state = world.get_block_state(target);
    let mut success = true;

    if FireBlock::can_be_placed_at(world, target, facing) {
        world.set_block(
            target,
            FireBlock::get_state(world.as_ref(), target),
            UpdateFlags::UPDATE_ALL,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            target,
            &GameEventContext::new(None, None),
        );
    } else if can_light(target_state) {
        world.set_block(
            target,
            target_state.set_value(&BlockStateProperties::LIT, true),
            UpdateFlags::UPDATE_ALL,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            target,
            &GameEventContext::new(None, Some(target_state)),
        );
    } else if target_state.get_block() == &vanilla_blocks::TNT {
        if TntBlock::prime(world, target, None, &stack) {
            let _ = world.remove_block(target, false);
        } else {
            success = false;
        }
    }

    if success {
        stack.hurt_and_break(1, false);
    }
    (stack, DispenseFeedback::Optional(success))
}

/// Vanilla `CampfireBlock.canLight || CandleBlock.canLight ||
/// CandleCakeBlock.canLight`.
///
/// The item behavior keeps the same check private, so the dispense path spells
/// out the shared tag/property test it uses.
fn can_light(state: BlockStateId) -> bool {
    let Some(lit) = state.try_get_value(&BlockStateProperties::LIT) else {
        return false;
    };
    if lit {
        return false;
    }

    let block = state.get_block();
    if block.has_tag(&BlockTag::CAMPFIRES) || block.has_tag(&BlockTag::CANDLES) {
        return state.try_get_value(&BlockStateProperties::WATERLOGGED) == Some(false);
    }
    block.has_tag(&BlockTag::CANDLE_CAKES)
}

/// Vanilla bone-meal entry: grows the targeted crop, then a water plant.
fn dispense_bone_meal(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let target = source.target();
    let success = grow_crop(source.world, target, &mut stack);
    if success {
        source.world.level_event(
            level_events::PARTICLES_AND_SOUND_PLANT_GROWTH,
            target,
            15,
            None,
        );
    }
    (stack, DispenseFeedback::Optional(success))
}

/// Vanilla `BoneMealItem.growCrop`, including the one-item consumption.
fn grow_crop(world: &Arc<World>, pos: BlockPos, stack: &mut ItemStack) -> bool {
    let state = world.get_block_state(pos);
    let Some(behavior) = BLOCK_BEHAVIORS.get_behavior_for_state(state) else {
        return false;
    };
    let Some(bonemealable) = behavior.as_bonemealable() else {
        return false;
    };
    if !bonemealable.is_valid_bonemeal_target(state, world.as_ref(), pos) {
        return false;
    }

    let mut rng = rand::rng();
    if bonemealable.is_bonemeal_success(state, world, &mut rng, pos) {
        bonemealable.perform_bonemeal(state, world, &mut rng, pos);
    }
    stack.shrink(1);
    true
}

/// Vanilla TNT entry.
fn dispense_tnt(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    if !source
        .world
        .get_game_rule(&vanilla_game_rules::TNT_EXPLODES)
    {
        return (stack, DispenseFeedback::Optional(false));
    }

    let target = source.target();
    if !TntBlock::prime(source.world, target, None, &stack) {
        return (stack, DispenseFeedback::Optional(false));
    }

    source.world.game_event(
        &vanilla_game_events::ENTITY_PLACE,
        target,
        &GameEventContext::new(None, None),
    );
    stack.shrink(1);
    (stack, DispenseFeedback::Optional(true))
}

/// Vanilla carved-pumpkin entry: places the pumpkin to form a golem, otherwise
/// equips it.
fn dispense_carved_pumpkin(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let target = source.target();
    if source.world.get_block_state(target).is_air()
        && crate::world::village::can_spawn_golem(source.world.as_ref(), target)
    {
        source.world.set_block(
            target,
            vanilla_blocks::CARVED_PUMPKIN.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        source.world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            target,
            &GameEventContext::new(None, None),
        );
        stack.shrink(1);
        (stack, DispenseFeedback::Optional(true))
    } else {
        let equipped = dispense_equipment(source, stack);
        (equipped, DispenseFeedback::Optional(false))
    }
}

/// Vanilla wither-skeleton-skull entry: places the skull when the target can
/// complete a wither build pattern, then runs the pattern check.
fn dispense_wither_skull(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let direction = source.facing();
    let target = source.target();
    if source.world.get_block_state(target).is_air()
        && WitherSkullBlock::can_spawn_mob(source.world, target, &stack)
    {
        let rotation = convert_to_rotation_segment(direction.to_yaw());
        source.world.set_block(
            target,
            vanilla_blocks::WITHER_SKELETON_SKULL
                .default_state()
                .set_value(&BlockStateProperties::ROTATION_16, rotation),
            UpdateFlags::UPDATE_ALL,
        );
        source.world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            target,
            &GameEventContext::new(None, None),
        );
        WitherSkullBlock::check_spawn(source.world, target);
        stack.shrink(1);
        (stack, DispenseFeedback::Optional(true))
    } else {
        let equipped = dispense_equipment(source, stack);
        (equipped, DispenseFeedback::Optional(false))
    }
}

/// Vanilla `ShulkerBoxDispenseBehavior`.
fn dispense_shulker_box(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let Some(block) = stack.item().block else {
        return (stack, DispenseFeedback::Optional(false));
    };

    let facing = source.facing();
    let target = source.target();
    let clicked_face = if source.world.get_block_state(target.below()).is_air() {
        facing
    } else {
        Direction::Up
    };
    let context =
        BlockPlaceContext::directional(source.world, target, facing, &mut stack, clicked_face);
    let success = BlockItem::new(block).place(context).consumes_action();
    (stack, DispenseFeedback::Optional(success))
}

/// Vanilla empty-bucket entry: picks up the fluid source in front.
fn dispense_empty_bucket(
    source: &DispenseSource<'_>,
    stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let target = source.target();
    let state = source.world.get_block_state(target);
    let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
    let Some(pickup) = behavior.pickup_block(source.world, target, state, None) else {
        return (default_dispense(source, stack), DispenseFeedback::Default);
    };

    source.world.game_event(
        &vanilla_game_events::FLUID_PICKUP,
        target,
        &GameEventContext::new(None, None),
    );
    let result = consume_with_remainder(source, stack, pickup.filled_bucket);
    (result, DispenseFeedback::Default)
}

/// Vanilla glass-bottle entry for water.
///
/// The beehive branch (`BeehiveBlock.releaseBeesAndResetHoneyLevel`) is not
/// reachable yet, so a glass bottle in front of a full beehive falls through to
/// the default eject.
fn dispense_glass_bottle(
    source: &DispenseSource<'_>,
    stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let target = source.target();
    if !source
        .world
        .get_block_state(target)
        .get_fluid_state()
        .is_water()
    {
        return (default_dispense(source, stack), DispenseFeedback::Default);
    }

    source.world.game_event(
        &vanilla_game_events::FLUID_PICKUP,
        source.pos,
        &GameEventContext::new(None, None),
    );
    let mut potion = ItemStack::new(&vanilla_items::POTION);
    potion.set(
        POTION_CONTENTS,
        PotionContents::empty().with_potion(RegistryReference::new(&vanilla_potions::WATER)),
    );
    let result = consume_with_remainder(source, stack, potion);
    (result, DispenseFeedback::Optional(true))
}

/// Vanilla brush entry: brushes scutes off the armadillos in front.
fn dispense_brush(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let target = source.target();
    let area = block_area(target);
    let armadillos = source.world.get_entities_in_aabb_matching(&area, |entity| {
        entity.downcast_ref::<ArmadilloEntity>().is_some()
    });
    if armadillos.is_empty() {
        return (stack, DispenseFeedback::Optional(false));
    }

    for entity in armadillos {
        let Some(armadillo) = entity.downcast_ref::<ArmadilloEntity>() else {
            continue;
        };
        if armadillo.brush_off_scute(&stack) {
            stack.hurt_and_break(16, false);
            return (stack, DispenseFeedback::Optional(true));
        }
    }
    (stack, DispenseFeedback::Optional(false))
}

/// Vanilla honeycomb entry: waxes the targeted block.
fn dispense_honeycomb(
    source: &DispenseSource<'_>,
    mut stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let target = source.target();
    let state = source.world.get_block_state(target);
    let Some(waxed_block) = get_waxed_from_normal_variant(state.get_block()) else {
        return (default_dispense(source, stack), DispenseFeedback::Default);
    };

    let waxed_state = REGISTRY.blocks.copy_matching_properties(state, waxed_block);
    source
        .world
        .set_block(target, waxed_state, UpdateFlags::UPDATE_ALL);
    source
        .world
        .level_event(level_events::PARTICLES_WAX_ON, target, 0, None);
    source
        .world
        .play_block_sound(&sound_events::ITEM_HONEYCOMB_WAX_ON, target, 1.0, 1.0, None);
    stack.shrink(1);
    (stack, DispenseFeedback::Optional(true))
}

/// Vanilla water-potion entry: converts the targeted mud-convertible block.
fn dispense_water_potion(
    source: &DispenseSource<'_>,
    stack: ItemStack,
) -> (ItemStack, DispenseFeedback) {
    let contents = stack.get_or_default(POTION_CONTENTS, PotionContents::empty());
    if !contents.is(&vanilla_potions::WATER) {
        return (default_dispense(source, stack), DispenseFeedback::Default);
    }

    let target = source.target();
    if !source
        .world
        .get_block_state(target)
        .get_block()
        .has_tag(&BlockTag::CONVERTIBLE_TO_MUD)
    {
        return (default_dispense(source, stack), DispenseFeedback::Default);
    }

    for _ in 0..5 {
        source.world.send_particles(
            steel_registry::particle_type::ParticleData::simple(&vanilla_particle_types::SPLASH),
            DVec3::new(
                f64::from(source.pos.x()) + rand::random::<f64>(),
                f64::from(source.pos.y()) + 1.0,
                f64::from(source.pos.z()) + rand::random::<f64>(),
            ),
            1,
            DVec3::ZERO,
            1.0,
        );
    }
    source
        .world
        .play_block_sound(&sound_events::ITEM_BOTTLE_EMPTY, source.pos, 1.0, 1.0, None);
    source.world.game_event(
        &vanilla_game_events::FLUID_PLACE,
        source.pos,
        &GameEventContext::new(None, None),
    );
    source.world.set_block(
        target,
        vanilla_blocks::MUD.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    let result =
        consume_with_remainder(source, stack, ItemStack::new(&vanilla_items::GLASS_BOTTLE));
    (result, DispenseFeedback::Default)
}

/// Vanilla `SpawnEggItemBehavior`.
fn dispense_spawn_egg(source: &DispenseSource<'_>, mut stack: ItemStack) -> ItemStack {
    let Some(entity_data) = stack.get(ENTITY_DATA) else {
        return stack;
    };
    let direction = source.facing();
    let request = EntitySpawnRequest {
        entity_type: entity_data.entity_type(),
        placement: EntitySpawnPlacement::Block {
            pos: source.target(),
            try_move_down: direction != Direction::Up,
            moved_up: false,
        },
        reason: EntitySpawnReason::Dispenser,
        finalize_spawn: true,
        play_ambient_sound: true,
        item_stack: Some(&stack),
        user_is_operator: false,
    };
    if spawn_entity(source.world, request).is_err() {
        return stack;
    }

    stack.shrink(1);
    source.world.game_event(
        &vanilla_game_events::ENTITY_PLACE,
        source.pos,
        &GameEventContext::new(None, None),
    );
    stack
}

/// Vanilla `EquipmentDispenseItemBehavior.dispenseEquipment`.
fn dispense_equipment(source: &DispenseSource<'_>, mut stack: ItemStack) -> ItemStack {
    let area = block_area(source.target());
    let entities = source.world.get_entities_in_aabb_matching(&area, |entity| {
        entity
            .as_living_entity()
            .is_some_and(|living| living.can_equip_with_dispenser(&stack))
    });
    let Some(entity) = entities.first() else {
        return default_dispense(source, stack);
    };
    let Some(living) = entity.as_living_entity() else {
        return default_dispense(source, stack);
    };
    let Some(equippable) = stack.get_equippable() else {
        return default_dispense(source, stack);
    };

    let slot = equippable.slot;
    let mut equipped = Some(stack.split(1));
    living.with_equipment_slot_mut(slot, &mut |item| {
        if let Some(stack) = equipped.take() {
            *item = stack;
        }
    });
    if let Some(mob) = entity.as_mob() {
        mob.set_guaranteed_drop(slot);
        mob.set_persistence_required();
    }
    stack
}

/// The vanilla `new AABB(pos)` area covering one block.
fn block_area(pos: BlockPos) -> WorldAabb {
    WorldAabb::new(
        f64::from(pos.x()),
        f64::from(pos.y()),
        f64::from(pos.z()),
        f64::from(pos.x()) + 1.0,
        f64::from(pos.y()) + 1.0,
        f64::from(pos.z()) + 1.0,
    )
}

/// Shared dispenser/dropper trigger: a rising edge schedules a dispense.
fn trigger_on_neighbor_change(
    block: BlockRef,
    state: BlockStateId,
    world: &Arc<World>,
    pos: BlockPos,
) {
    let should_trigger = world.has_neighbor_signal(pos) || world.has_neighbor_signal(pos.above());
    let triggered = state.get_value(TRIGGERED);
    if should_trigger && !triggered {
        world.schedule_block_tick_default(pos, block, TRIGGER_DURATION);
        world.set_block(
            pos,
            state.set_value(TRIGGERED, true),
            UpdateFlags::UPDATE_CLIENTS,
        );
    } else if !should_trigger && triggered {
        world.set_block(
            pos,
            state.set_value(TRIGGERED, false),
            UpdateFlags::UPDATE_CLIENTS,
        );
    }
}

/// Opens the shared dispenser menu and awards the inspect stat.
fn open_dispenser_menu(
    world: &Arc<World>,
    pos: BlockPos,
    player: &Player,
    title: TextComponent,
    stat: CustomStatRef,
) -> InteractionResult {
    let Some(block_entity) = world.get_block_entity(pos) else {
        return InteractionResult::Success;
    };
    let Some(container) = ContainerRef::from_block_entity(block_entity) else {
        return InteractionResult::Success;
    };

    let inventory = Arc::clone(&player.inventory);
    player.open_menu(title, move |context| {
        dispenser(inventory, context.container_id, container)
    });
    player.award_custom_stat(stat);
    InteractionResult::Success
}

/// Vanilla `AbstractContainerMenu.getRedstoneSignalFromBlockEntity`.
fn analog_output(world: &dyn LevelReader, pos: BlockPos) -> i32 {
    let Some(container) = world
        .get_block_entity(pos)
        .and_then(ContainerRef::from_block_entity)
    else {
        return 0;
    };
    let guard = ContainerLockGuard::lock_all(&[&container]);
    guard
        .get(container.container_id())
        .map_or(0, calculate_redstone_signal_from_container)
}

/// Resolves the block entity at `pos` as a container, warning when the block
/// entity type does not match the block that is dispensing.
fn dispense_container(
    world: &Arc<World>,
    pos: BlockPos,
    expected: steel_registry::block_entity_type::BlockEntityTypeRef,
) -> Option<ContainerRef> {
    let Some(block_entity) = world.get_block_entity(pos) else {
        log::warn!("Ignoring dispensing attempt without a block entity at {pos:?}");
        return None;
    };
    if block_entity.get_type() != expected {
        log::warn!("Ignoring dispensing attempt for a mismatched block entity at {pos:?}");
        return None;
    }
    ContainerRef::from_block_entity(block_entity)
}

impl BlockBehavior for DispenserBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.get_nearest_looking_direction().opposite()),
        )
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        open_dispenser_menu(
            world,
            pos,
            player,
            TextComponent::translated(translations::CONTAINER_DISPENSER.msg()),
            &vanilla_custom_stats::INSPECT_DISPENSER,
        )
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::DISPENSER,
            level,
            pos,
            state,
        ))
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        trigger_on_neighbor_change(self.block, state, world, pos);
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.dispense_from(world, state, pos);
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        analog_output(world, pos)
    }
}

impl DispenserBlock {
    /// Vanilla `DispenserBlock.dispenseFrom`.
    fn dispense_from(&self, world: &Arc<World>, state: BlockStateId, pos: BlockPos) {
        let Some(container) =
            dispense_container(world, pos, &vanilla_block_entity_types::DISPENSER)
        else {
            return;
        };
        let source = DispenseSource {
            world,
            pos,
            state,
            container: &container,
        };

        let Some((slot, stack)) = source.random_slot() else {
            source.level_event(level_events::SOUND_DISPENSER_FAIL);
            world.game_event(
                &vanilla_game_events::BLOCK_ACTIVATE,
                pos,
                &GameEventContext::new(None, Some(state)),
            );
            return;
        };

        let remaining = dispense(&source, stack);
        source.set_slot(slot, remaining);
    }
}

impl BlockBehavior for DropperBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.get_nearest_looking_direction().opposite()),
        )
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        open_dispenser_menu(
            world,
            pos,
            player,
            TextComponent::translated(translations::CONTAINER_DROPPER.msg()),
            &vanilla_custom_stats::INSPECT_DROPPER,
        )
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::DROPPER,
            level,
            pos,
            state,
        ))
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        trigger_on_neighbor_change(self.block, state, world, pos);
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.dispense_from(world, state, pos);
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        analog_output(world, pos)
    }
}

impl DropperBlock {
    /// Vanilla `DropperBlock.dispenseFrom`.
    fn dispense_from(&self, world: &Arc<World>, state: BlockStateId, pos: BlockPos) {
        let Some(container) = dispense_container(world, pos, &vanilla_block_entity_types::DROPPER)
        else {
            return;
        };
        let source = DispenseSource {
            world,
            pos,
            state,
            container: &container,
        };

        let Some((slot, stack)) = source.random_slot() else {
            source.level_event(level_events::SOUND_DISPENSER_FAIL);
            return;
        };

        let direction = source.facing();
        let remaining = match crate::block_entity::entities::hopper_container_at(
            world,
            direction.relative(pos),
        ) {
            Some(into) => push_into_container(&container, &into, stack, direction),
            None => dispense(&source, stack),
        };
        source.set_slot(slot, remaining);
    }
}

/// Vanilla `DropperBlock` push: inserts one item into the faced container and
/// returns the dropper's remaining stack.
fn push_into_container(
    from: &ContainerRef,
    into: &ContainerRef,
    stack: ItemStack,
    direction: Direction,
) -> ItemStack {
    let mut guard = ContainerLockGuard::lock_all(&[from, into]);
    let Some([from_container, into_container]) =
        guard.get_disjoint_mut([from.container_id(), into.container_id()])
    else {
        return stack;
    };

    let leftover = crate::block_entity::entities::hopper_add_item(
        Some(from_container),
        into_container,
        stack.copy_with_count(1),
        Some(direction.opposite()),
    );
    if leftover.is_empty() {
        let mut remaining = stack;
        remaining.shrink(1);
        remaining
    } else {
        stack
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::*;

    fn stack_of(item: ItemRef) -> ItemStack {
        ItemStack::new(item)
    }

    #[test]
    fn projectile_items_use_the_projectile_behavior() {
        init_vanilla_registry();
        for item in [
            &*vanilla_items::ARROW,
            &*vanilla_items::SNOWBALL,
            &*vanilla_items::WIND_CHARGE,
        ] {
            assert!(matches!(
                dispense_method(&stack_of(item)),
                DispenseMethod::Projectile
            ));
        }
    }

    #[test]
    fn special_items_select_their_vanilla_behaviors() {
        init_vanilla_registry();
        assert!(matches!(
            dispense_method(&stack_of(&vanilla_items::FLINT_AND_STEEL)),
            DispenseMethod::FlintAndSteel
        ));
        assert!(matches!(
            dispense_method(&stack_of(&vanilla_items::BONE_MEAL)),
            DispenseMethod::BoneMeal
        ));
        assert!(matches!(
            dispense_method(&stack_of(&vanilla_items::WHITE_SHULKER_BOX)),
            DispenseMethod::ShulkerBox
        ));
        assert!(matches!(
            dispense_method(&stack_of(&vanilla_items::BUCKET)),
            DispenseMethod::EmptyBucket
        ));
    }

    #[test]
    fn plain_items_fall_back_to_the_default_behavior() {
        init_vanilla_registry();
        assert!(matches!(
            dispense_method(&stack_of(&vanilla_items::STONE)),
            DispenseMethod::Default
        ));
    }
}

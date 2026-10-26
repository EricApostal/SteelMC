//! Vanilla `/loot` command.
//!
//! Mirrors `net.minecraft.server.commands.LootCommand`: every drop target
//! (`replace`, `insert`, `give`, `spawn`) wraps the same four loot sources
//! (`fish`, `loot`, `kill`, `mine`), and each target reports through vanilla's
//! `CommandResponseTracker` semantics.

use std::fmt;

use glam::DVec3;
use steel_protocol::packets::game::{
    ArgumentType as ProtocolArgumentType, SuggestionType as ProtocolSuggestionType,
};
use steel_registry::{
    REGISTRY, RegistryExt as _,
    blocks::block_state_ext::BlockStateExt as _,
    data_components::vanilla_components::{CUSTOM_NAME, ITEM_NAME},
    equipment::EquipmentSlot,
    item_stack::ItemStack,
    loot_table::{DamageSourceInfo, LootContext, LootTableRef},
    vanilla_damage_types,
};
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey, Identifier, translations};
use text_components::{TextComponent, translation::TranslatedMessage};

use super::super::{
    brigadier::{
        ArgumentType, CommandNodeBuilder, CommandSyntaxError, CommandSyntaxErrorKind, StringReader,
        SuggestionsBuilder,
    },
    execution::{
        CommandArgumentSource, CommandSource, SteelArgumentParser, SteelArgumentSuggestionContext,
        SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument, literal,
    },
    registration::CommandRegistration,
};
use super::execute::loaded_block_position;
use crate::{
    behavior::BlockLootContext,
    command::incorrectly_typed_argument,
    entity::{Entity, SharedEntity, entity_loot_ref, loot_equipment},
    inventory::{
        container::Container,
        lock::{ContainerId, ContainerLockGuard, ContainerRef},
    },
    player::Player,
    world::LevelReader as _,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("loot"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("loot")
        .then(
            literal("replace")
                .then(
                    literal("entity").then(
                        argument("entities", SteelArgumentType::entities()).then(
                            argument("slot", slot_argument())
                                .then(sources(Target::ReplaceEntity {
                                    explicit_count: false,
                                }))
                                .then(argument("count", ArgumentType::integer(0, i32::MAX)).then(
                                    sources(Target::ReplaceEntity {
                                        explicit_count: true,
                                    }),
                                )),
                        ),
                    ),
                )
                .then(
                    literal("block").then(
                        argument("targetPos", SteelArgumentType::block_pos()).then(
                            argument("slot", slot_argument())
                                .then(sources(Target::ReplaceBlock {
                                    explicit_count: false,
                                }))
                                .then(argument("count", ArgumentType::integer(0, i32::MAX)).then(
                                    sources(Target::ReplaceBlock {
                                        explicit_count: true,
                                    }),
                                )),
                        ),
                    ),
                ),
        )
        .then(literal("insert").then(
            argument("targetPos", SteelArgumentType::block_pos()).then(sources(Target::Insert)),
        ))
        .then(
            literal("give").then(
                argument("players", SteelArgumentType::players()).then(sources(Target::Give)),
            ),
        )
        .then(literal("spawn").then(
            argument("targetPos", SteelArgumentType::vec3(true)).then(sources(Target::Spawn)),
        ))
}

/// Vanilla's four loot sources, shared by every drop target.
fn sources(target: Target) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    let fish =
        literal("fish").then(
            argument("loot_table", loot_table_argument()).then(
                argument("pos", SteelArgumentType::block_pos())
                    .executes(move |context| drop_fishing_loot(context, target, ToolKind::None))
                    .then(argument("tool", SteelArgumentType::item_stack()).executes(
                        move |context| drop_fishing_loot(context, target, ToolKind::Argument),
                    ))
                    .then(literal("mainhand").executes(move |context| {
                        drop_fishing_loot(context, target, ToolKind::MainHand)
                    }))
                    .then(literal("offhand").executes(move |context| {
                        drop_fishing_loot(context, target, ToolKind::OffHand)
                    })),
            ),
        );
    let loot = literal("loot").then(
        argument("loot_table", loot_table_argument())
            .executes(move |context| drop_chest_loot(context, target)),
    );
    let kill = literal("kill").then(
        argument("target", SteelArgumentType::entity())
            .executes(move |context| drop_kill_loot(context, target)),
    );
    let mine = literal("mine").then(
        argument("pos", SteelArgumentType::block_pos())
            .executes(move |context| drop_block_loot(context, target, ToolKind::None))
            .then(
                argument("tool", SteelArgumentType::item_stack())
                    .executes(move |context| drop_block_loot(context, target, ToolKind::Argument)),
            )
            .then(
                literal("mainhand")
                    .executes(move |context| drop_block_loot(context, target, ToolKind::MainHand)),
            )
            .then(
                literal("offhand")
                    .executes(move |context| drop_block_loot(context, target, ToolKind::OffHand)),
            ),
    );

    fish.then(loot).then(kill).then(mine)
}

/// Where rolled drops are delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// Drop the items into the world at a position.
    Spawn,
    /// Add the items to players' inventories.
    Give,
    /// Distribute the items into a block container.
    Insert,
    /// Replace block-container slots.
    ReplaceBlock { explicit_count: bool },
    /// Replace entity slots.
    ReplaceEntity { explicit_count: bool },
}

/// Which tool the loot context receives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolKind {
    /// Vanilla's `ItemStack.EMPTY` tool.
    None,
    /// An explicit `tool` item argument.
    Argument,
    /// The source entity's main hand.
    MainHand,
    /// The source entity's off hand.
    OffHand,
}

/// Vanilla `CommandResponseTracker<ItemStack>` restricted to the `NON_ZERO`
/// element set, which is the only set `/loot` dispatches.
struct ResponseTracker {
    total_value: i32,
    only_non_zero_element: Option<ItemStack>,
    non_zero_element_count: i32,
}

impl ResponseTracker {
    const fn new() -> Self {
        Self {
            total_value: 0,
            only_non_zero_element: None,
            non_zero_element_count: 0,
        }
    }

    /// Vanilla `CommandResponseTracker.track(element)`: one unit of value per
    /// tracked stack, regardless of the stack's own count.
    fn track(&mut self, stack: &ItemStack) {
        self.total_value += 1;
        self.non_zero_element_count += 1;
        self.only_non_zero_element = if self.non_zero_element_count == 1 {
            Some(stack.clone())
        } else {
            None
        };
    }
}

fn drop_fishing_loot(
    context: &SteelCommandContext<CommandSource>,
    target: Target,
    tool_kind: ToolKind,
) -> Result<i32, CommandSyntaxError> {
    let loot_table = loot_table(context)?;
    let pos = loaded_block_position(context, "pos")?;
    let tool = tool_for(context, tool_kind)?;
    let origin = block_center(pos);
    let source = context.source();

    let equipment = source
        .entity()
        .and_then(|entity| loot_equipment(entity.as_ref()));
    let mut rng = rand::rng();
    let mut params = LootContext::new(&mut rng)
        .with_origin(origin.x, origin.y, origin.z)
        .with_tool(&tool);
    if let Some(entity) = source.entity() {
        params = params.with_this_entity(entity_loot_ref(entity.as_ref(), equipment.as_ref()));
    }

    let drops = loot_table.get_random_items(&mut params);
    deliver(context, target, drops, None)
}

fn drop_chest_loot(
    context: &SteelCommandContext<CommandSource>,
    target: Target,
) -> Result<i32, CommandSyntaxError> {
    let loot_table = loot_table(context)?;
    let source = context.source();
    let origin = source.position();

    let equipment = source
        .entity()
        .and_then(|entity| loot_equipment(entity.as_ref()));
    let mut rng = rand::rng();
    let mut params = LootContext::new(&mut rng).with_origin(origin.x, origin.y, origin.z);
    if let Some(entity) = source.entity() {
        params = params.with_this_entity(entity_loot_ref(entity.as_ref(), equipment.as_ref()));
    }

    let drops = loot_table.get_random_items(&mut params);
    deliver(context, target, drops, None)
}

fn drop_kill_loot(
    context: &SteelCommandContext<CommandSource>,
    target: Target,
) -> Result<i32, CommandSyntaxError> {
    let victim = context.entity("target")?;
    let Some(loot_table) = entity_loot_table(victim.as_ref()) else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DROP_NO_LOOT_TABLE_ENTITY
                .message([victim.display_name()])
                .component(),
        ));
    };

    let source = context.source();
    let origin = source.position();
    let attacker = source.entity().map(AsRef::as_ref);
    let attacker_is_player = attacker.is_some_and(|attacker| attacker.as_player().is_some());
    let attacker_equipment = attacker.and_then(loot_equipment);
    let victim_equipment = loot_equipment(victim.as_ref());
    let this_entity = entity_loot_ref(victim.as_ref(), victim_equipment.as_ref());
    let attacker_ref =
        attacker.map(|attacker| entity_loot_ref(attacker, attacker_equipment.as_ref()));
    let damage_source = DamageSourceInfo {
        damage_type: Some(&vanilla_damage_types::MAGIC.key),
        tags: &[],
        is_direct: true,
    };

    let mut rng = rand::rng();
    let mut params = LootContext::new(&mut rng)
        .with_origin(origin.x, origin.y, origin.z)
        .with_game_time(source.world().game_time())
        .with_killed_by_player(attacker_is_player)
        .with_this_entity(this_entity)
        .with_damage_source(damage_source);
    if let Some(attacker_ref) = attacker_ref {
        params = params
            .with_killer_entity(attacker_ref)
            .with_direct_killer_entity(attacker_ref);
        if attacker_is_player {
            params = params.with_last_damage_player(attacker_ref);
        }
    }

    let drops = loot_table.get_random_items(&mut params);
    deliver(context, target, drops, Some(&loot_table.key))
}

fn drop_block_loot(
    context: &SteelCommandContext<CommandSource>,
    target: Target,
    tool_kind: ToolKind,
) -> Result<i32, CommandSyntaxError> {
    let pos = loaded_block_position(context, "pos")?;
    let tool = tool_for(context, tool_kind)?;
    let source = context.source();
    let world = source.world();
    let state = world.get_block_state(pos);
    let block = state.get_block();
    let loot_key = Identifier::vanilla(format!("blocks/{}", block.key.path));
    if REGISTRY.loot_tables.by_key(&loot_key).is_none() {
        let name = TextComponent::translated(TranslatedMessage {
            key: format!("block.{}", block.key).into(),
            fallback: None,
            args: None,
        });
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DROP_NO_LOOT_TABLE_BLOCK
                .message([name])
                .component(),
        ));
    }

    let block_entity = world.get_block_entity(pos);
    let mut loot_context = BlockLootContext::new(world, pos)
        .with_tool(&tool)
        .with_block_entity(block_entity.as_deref());
    if let Some(entity) = source.entity() {
        loot_context = loot_context.with_entity(Some(entity.as_ref()));
    }

    let drops = loot_context.get_drops(state);
    deliver(context, target, drops, Some(&loot_key))
}

/// Rolls are done; deliver them to the configured target and report vanilla's
/// success message.
fn deliver(
    context: &SteelCommandContext<CommandSource>,
    target: Target,
    drops: Vec<ItemStack>,
    loot_table: Option<&Identifier>,
) -> Result<i32, CommandSyntaxError> {
    let mut tracker = ResponseTracker::new();
    match target {
        Target::Spawn => drop_in_world(context, &drops, &mut tracker),
        Target::Give => player_give(context, &drops, &mut tracker)?,
        Target::Insert => block_distribute(context, &drops, &mut tracker)?,
        Target::ReplaceBlock { explicit_count } => {
            block_replace(context, &drops, explicit_count, &mut tracker)?;
        }
        Target::ReplaceEntity { explicit_count } => {
            entity_replace(context, &drops, explicit_count, &mut tracker)?;
        }
    }
    Ok(report(context, &tracker, loot_table))
}

fn report(
    context: &SteelCommandContext<CommandSource>,
    tracker: &ResponseTracker,
    loot_table: Option<&Identifier>,
) -> i32 {
    let message = match (&tracker.only_non_zero_element, loot_table) {
        (Some(stack), None) => translations::COMMANDS_DROP_SUCCESS_SINGLE
            .message([
                TextComponent::from(stack.count().to_string()),
                item_display_name(stack),
            ])
            .component(),
        (Some(stack), Some(loot_table)) => translations::COMMANDS_DROP_SUCCESS_SINGLE_WITH_TABLE
            .message([
                TextComponent::from(stack.count().to_string()),
                item_display_name(stack),
                TextComponent::plain(loot_table.to_string()),
            ])
            .component(),
        (None, None) => translations::COMMANDS_DROP_SUCCESS_MULTIPLE
            .message([TextComponent::from(
                tracker.non_zero_element_count.to_string(),
            )])
            .component(),
        (None, Some(loot_table)) => translations::COMMANDS_DROP_SUCCESS_MULTIPLE_WITH_TABLE
            .message([
                TextComponent::from(tracker.non_zero_element_count.to_string()),
                TextComponent::plain(loot_table.to_string()),
            ])
            .component(),
    };
    context.source().send_success(&message, false);
    tracker.total_value
}

/// Vanilla `dropInWorld`.
fn drop_in_world(
    context: &SteelCommandContext<CommandSource>,
    drops: &[ItemStack],
    tracker: &mut ResponseTracker,
) {
    let pos = context.coordinates("targetPos").map_or_else(
        |_| context.source().position(),
        |pos| pos.position(context.source()),
    );
    let world = context.source().world();
    for drop in drops {
        if let Some(item_entity) = world.spawn_item(pos, drop.clone()) {
            item_entity.set_default_pickup_delay();
        }
        tracker.track(drop);
    }
}

/// Vanilla `playerGive`.
fn player_give(
    context: &SteelCommandContext<CommandSource>,
    drops: &[ItemStack],
    tracker: &mut ResponseTracker,
) -> Result<(), CommandSyntaxError> {
    let players = context.players("players")?;
    for drop in drops {
        for player in &players {
            let mut copy = drop.clone();
            let added = player.inventory.lock().add(&mut copy);
            if added {
                tracker.track(drop);
                player.broadcast_inventory_changes();
            }
        }
    }
    Ok(())
}

/// Vanilla `blockDistribute`.
fn block_distribute(
    context: &SteelCommandContext<CommandSource>,
    drops: &[ItemStack],
    tracker: &mut ResponseTracker,
) -> Result<(), CommandSyntaxError> {
    let pos = loaded_block_position(context, "targetPos")?;
    let container = block_container(context.source(), pos)?;
    let id = container.container_id();
    let mut guard = ContainerLockGuard::lock_all(&[&container]);
    for drop in drops {
        if distribute_to_container(&mut guard, id, drop.clone()) {
            tracker.track(drop);
        }
    }
    Ok(())
}

/// Vanilla `distributeToContainer`.
fn distribute_to_container(
    guard: &mut ContainerLockGuard,
    id: ContainerId,
    mut stack: ItemStack,
) -> bool {
    let size = guard.get(id).map_or(0, Container::get_container_size);
    let mut changed = false;
    for slot in 0..size {
        if stack.is_empty() {
            break;
        }
        let Some((can_place, current)) = guard.get(id).map(|container| {
            (
                container.can_place_item(slot, &stack),
                container.get_item(slot).clone(),
            )
        }) else {
            break;
        };
        if !can_place {
            continue;
        }
        if current.is_empty() {
            guard.set_item(id, slot, stack.clone());
            changed = true;
            break;
        }
        if can_merge_items(&current, &stack) {
            let space = stack.max_stack_size() - current.count();
            let count = stack.count().min(space);
            stack.shrink(count);
            let mut merged = current;
            merged.grow(count);
            guard.set_item(id, slot, merged);
            changed = true;
        }
    }
    changed
}

/// Vanilla `canMergeItems`.
fn can_merge_items(existing: &ItemStack, candidate: &ItemStack) -> bool {
    existing.count() <= existing.max_stack_size()
        && ItemStack::is_same_item_same_components(existing, candidate)
}

/// Vanilla `blockReplace`.
fn block_replace(
    context: &SteelCommandContext<CommandSource>,
    drops: &[ItemStack],
    explicit_count: bool,
    tracker: &mut ResponseTracker,
) -> Result<(), CommandSyntaxError> {
    let pos = loaded_block_position(context, "targetPos")?;
    let start_slot = slot_value(context)?;
    let slot_count = slot_count(context, drops.len(), explicit_count)?;
    let container = block_container(context.source(), pos)?;
    let id = container.container_id();
    let mut guard = ContainerLockGuard::lock_all(&[&container]);
    let max_slot = guard.get(id).map_or(0, Container::get_container_size);

    if usize::try_from(start_slot)
        .ok()
        .is_none_or(|slot| slot >= max_slot)
    {
        return Err(slot_error(start_slot));
    }

    for offset in 0..slot_count {
        let Some(slot) = start_slot
            .checked_add(offset)
            .and_then(|slot| usize::try_from(slot).ok())
        else {
            break;
        };
        let to_add = drop_at(drops, offset);
        let can_place = guard.get(id).is_some_and(|container| {
            slot < container.get_container_size() && container.can_place_item(slot, &to_add)
        });
        if can_place {
            guard.set_item(id, slot, to_add.clone());
            tracker.track(&to_add);
        }
    }
    Ok(())
}

/// Vanilla `entityReplace` + `setSlots`.
fn entity_replace(
    context: &SteelCommandContext<CommandSource>,
    drops: &[ItemStack],
    explicit_count: bool,
    tracker: &mut ResponseTracker,
) -> Result<(), CommandSyntaxError> {
    let entities = context.entities("entities")?;
    let start_slot = slot_value(context)?;
    let slot_count = slot_count(context, drops.len(), explicit_count)?;

    for entity in &entities {
        set_slots(entity, drops, start_slot, slot_count, tracker);
        if let Some(player) = entity.as_player() {
            player.broadcast_inventory_changes();
        }
    }
    Ok(())
}

/// Vanilla `setSlots`: a null slot access means the entity has no such slot.
fn set_slots(
    entity: &SharedEntity,
    drops: &[ItemStack],
    start_slot: i32,
    count: i32,
    tracker: &mut ResponseTracker,
) {
    for offset in 0..count {
        let item = drop_at(drops, offset);
        let Some(slot) = start_slot.checked_add(offset) else {
            break;
        };
        if set_entity_slot(entity.as_ref(), slot, item.clone()) {
            tracker.track(&item);
        }
    }
}

/// Returns the `drops` entry at `offset`, or the empty stack vanilla pads with.
fn drop_at(drops: &[ItemStack], offset: i32) -> ItemStack {
    usize::try_from(offset)
        .ok()
        .and_then(|offset| drops.get(offset))
        .cloned()
        .unwrap_or_else(ItemStack::empty)
}

/// Mirrors vanilla `Entity.getSlot` for the slot indices Steel can address.
///
/// Vanilla resolves player slots 0-35, 200-226, 499 and 500-503 plus
/// `LivingEntity` equipment slots 98-106. Steel's player inventory and ender
/// chest containers cover 0-35 and 200-226, and equipment covers 98-106. The
/// carried item (499), the crafting grid (500-503), mob inventory carriers
/// (300+) and horse inventory (500-514) have no Steel accessor yet, so those
/// indices report "no slot" exactly like a vanilla entity without slot access.
fn set_entity_slot(entity: &dyn Entity, slot: i32, item: ItemStack) -> bool {
    if let Some(player) = entity.as_player()
        && set_player_slot(player, slot, &item)
    {
        return true;
    }
    let Some(living) = entity.as_living_entity() else {
        return false;
    };
    let Some(equipment) = equipment_slot(slot) else {
        return false;
    };
    if !equipment_accepts(equipment, &item) {
        return false;
    }
    let mut set = |current: &mut ItemStack| current.clone_from(&item);
    living.with_equipment_slot_mut(equipment, &mut set);
    true
}

fn set_player_slot(player: &Player, slot: i32, item: &ItemStack) -> bool {
    let Ok(index) = usize::try_from(slot) else {
        return false;
    };
    match index {
        0..=35 => {
            let mut inventory = player.inventory.lock();
            Container::set_item(&mut *inventory, index, item.clone());
            true
        }
        200..=226 => {
            let mut ender_chest = player.ender_chest_inventory.lock();
            let index = index - 200;
            if index < ender_chest.get_container_size() {
                ender_chest.set_item(index, item.clone());
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Vanilla `LivingEntity.getEquipmentSlot(int)`.
const fn equipment_slot(slot: i32) -> Option<EquipmentSlot> {
    match slot {
        98 => Some(EquipmentSlot::MainHand),
        99 => Some(EquipmentSlot::OffHand),
        100 => Some(EquipmentSlot::Feet),
        101 => Some(EquipmentSlot::Legs),
        102 => Some(EquipmentSlot::Chest),
        103 => Some(EquipmentSlot::Head),
        105 => Some(EquipmentSlot::Body),
        106 => Some(EquipmentSlot::Saddle),
        _ => None,
    }
}

/// Vanilla `createEquipmentSlotAccess` predicate: armor, body and saddle slots
/// only accept empty stacks or items equippable in that slot.
fn equipment_accepts(slot: EquipmentSlot, item: &ItemStack) -> bool {
    match slot {
        EquipmentSlot::Head | EquipmentSlot::MainHand | EquipmentSlot::OffHand => true,
        _ => item.is_empty() || item.is_equippable_in_slot(slot),
    }
}

fn slot_count(
    context: &SteelCommandContext<CommandSource>,
    drops: usize,
    explicit_count: bool,
) -> Result<i32, CommandSyntaxError> {
    if explicit_count {
        return context.integer("count");
    }
    i32::try_from(drops)
        .map_err(|_| CommandSyntaxError::dynamic("Drop count exceeds the command result range"))
}

fn block_container(
    source: &CommandSource,
    pos: BlockPos,
) -> Result<ContainerRef, CommandSyntaxError> {
    source
        .world()
        .get_block_entity(pos)
        .and_then(ContainerRef::from_block_entity)
        .ok_or_else(|| {
            CommandSyntaxError::dynamic(
                translations::COMMANDS_ITEM_TARGET_NOT_A_CONTAINER
                    .message([
                        TextComponent::from(pos.x().to_string()),
                        TextComponent::from(pos.y().to_string()),
                        TextComponent::from(pos.z().to_string()),
                    ])
                    .component(),
            )
        })
}

fn slot_error(slot: i32) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_ITEM_TARGET_NO_SUCH_SLOT
            .message([TextComponent::from(slot.to_string())])
            .component(),
    )
}

fn block_center(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    )
}

/// Vanilla `LootCommand.getSourceHandItem`.
fn source_hand_item(
    source: &CommandSource,
    slot: EquipmentSlot,
) -> Result<ItemStack, CommandSyntaxError> {
    let Some(entity) = source.entity() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_ENTITY,
        )));
    };
    let Some(living) = entity.as_living_entity() else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DROP_NO_HELD_ITEMS.msg().component(),
        ));
    };
    let mut item = ItemStack::empty();
    living.with_equipment_slot(slot, &mut |current| item = current.clone());
    Ok(item)
}

fn tool_for(
    context: &SteelCommandContext<CommandSource>,
    tool_kind: ToolKind,
) -> Result<ItemStack, CommandSyntaxError> {
    match tool_kind {
        ToolKind::None => Ok(ItemStack::empty()),
        ToolKind::Argument => Ok(context.item_stack("tool")?.copy_with_count(1)),
        ToolKind::MainHand => source_hand_item(context.source(), EquipmentSlot::MainHand),
        ToolKind::OffHand => source_hand_item(context.source(), EquipmentSlot::OffHand),
    }
}

/// Vanilla `Mob.getLootTable`: a mob's custom death loot table overrides the
/// entity type's default table.
fn entity_loot_table(entity: &dyn Entity) -> Option<LootTableRef> {
    if let Some(mob) = entity.as_mob()
        && mob.has_custom_death_loot_table()
    {
        return mob.custom_death_loot_table();
    }
    let key = Identifier::vanilla(format!("entities/{}", entity.entity_type().key.path));
    REGISTRY.loot_tables.by_key(&key)
}

/// Vanilla `ItemStack.getDisplayName`.
fn item_display_name(stack: &ItemStack) -> TextComponent {
    stack
        .get(CUSTOM_NAME)
        .or_else(|| stack.get(ITEM_NAME))
        .cloned()
        .unwrap_or_else(|| TextComponent::plain(stack.item().key.to_string()))
}

fn loot_table(
    context: &SteelCommandContext<CommandSource>,
) -> Result<LootTableRef, CommandSyntaxError> {
    context
        .argument("loot_table")?
        .downcast_ref::<LootTableValue>()
        .map(|value| value.0)
        .ok_or_else(|| incorrectly_typed_argument("loot_table"))
}

fn slot_value(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    context
        .argument("slot")?
        .downcast_ref::<SlotValue>()
        .map(|value| value.0)
        .ok_or_else(|| incorrectly_typed_argument("slot"))
}

fn loot_table_argument() -> SteelArgumentType {
    SteelArgumentType::new(LootTableParser)
}

fn slot_argument() -> SteelArgumentType {
    SteelArgumentType::new(SlotParser)
}

/// A resolved `minecraft:loot_table` command argument.
#[derive(Clone, Copy)]
struct LootTableValue(LootTableRef);

// SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
unsafe impl DowncastType for LootTableValue {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:command/value/loot_table");
}

impl fmt::Debug for LootTableValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("LootTableValue")
            .field(&self.0.key)
            .finish()
    }
}

/// Vanilla `ResourceOrIdArgument.LootTableArgument` restricted to registry ids.
///
/// Vanilla also accepts inline SNBT loot tables; Steel's loot tables are
/// generated statics without a codec, so inline values are rejected as a parse
/// failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LootTableParser;

// SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
unsafe impl DowncastType for LootTableParser {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:command/parser/loot_table");
}

impl SteelArgumentParser for LootTableParser {
    type Value = LootTableValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        if reader.peek() == Some('{') {
            let raw = reader.read_unquoted_token().to_owned();
            return Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
                translations::ARGUMENT_RESOURCE_OR_ID_FAILED_TO_PARSE
                    .message([TextComponent::plain(raw)])
                    .component(),
            ))));
        }
        let raw = read_identifier(reader).to_owned();
        let key = raw.parse::<Identifier>().map_err(|_| {
            reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
                translations::ARGUMENT_RESOURCE_OR_ID_FAILED_TO_PARSE
                    .message([TextComponent::plain(raw.clone())])
                    .component(),
            )))
        })?;
        REGISTRY.loot_tables.by_key(&key).map_or_else(
            || {
                Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
                    translations::ARGUMENT_RESOURCE_OR_ID_NO_SUCH_ELEMENT
                        .message([
                            TextComponent::plain(key.to_string()),
                            TextComponent::plain(LOOT_TABLE_REGISTRY.to_owned()),
                        ])
                        .component(),
                ))))
            },
            |table| Ok(LootTableValue(table)),
        )
    }

    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        let prefix = builder.remaining().to_owned();
        for (_, table) in REGISTRY.loot_tables.iter() {
            let key = table.key.to_string();
            if key.starts_with(&prefix) {
                builder.suggest(key);
            }
        }
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (
            ProtocolArgumentType::LootTable,
            Some(ProtocolSuggestionType::AskServer),
        )
    }
}

/// The registry feeding the `minecraft:loot_table` argument.
const LOOT_TABLE_REGISTRY: &str = "minecraft:loot_table";

/// A resolved `minecraft:item_slot` command argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SlotValue(i32);

// SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
unsafe impl DowncastType for SlotValue {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:command/value/slot");
}

/// Vanilla `SlotArgument`: parses a named `SlotRanges` entry that must resolve
/// to exactly one slot index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SlotParser;

// SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
unsafe impl DowncastType for SlotParser {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:command/parser/slot");
}

impl SteelArgumentParser for SlotParser {
    type Value = SlotValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        let name = reader.read_unquoted_token().to_owned();
        match slot_range(&name) {
            Some(SlotRange::Single(slot)) => Ok(SlotValue(slot)),
            Some(SlotRange::Multiple) => {
                Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
                    translations::SLOT_ONLY_SINGLE_ALLOWED
                        .message([TextComponent::plain(name)])
                        .component(),
                ))))
            }
            None => Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
                translations::SLOT_UNKNOWN
                    .message([TextComponent::plain(name)])
                    .component(),
            )))),
        }
    }

    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        let prefix = builder.remaining().to_owned();
        for name in single_slot_names() {
            if name.starts_with(&prefix) {
                builder.suggest(name);
            }
        }
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (ProtocolArgumentType::ItemSlot, None)
    }
}

/// One vanilla `SlotRanges` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotRange {
    /// A range that resolves to exactly one slot index.
    Single(i32),
    /// A range with several slot indices, rejected by `SlotArgument`.
    Multiple,
}

/// The numbered vanilla `SlotRanges` prefixes: name prefix, first slot, size.
const SLOT_RANGE_PREFIXES: [(&str, i32, i32); 7] = [
    ("container.", 0, 54),
    ("hotbar.", 0, 9),
    ("inventory.", 9, 27),
    ("enderchest.", 200, 27),
    ("mob.inventory.", 300, 8),
    ("horse.", 500, 15),
    ("player.crafting.", 500, 4),
];

/// Vanilla `SlotRanges.nameToIds`.
fn slot_range(name: &str) -> Option<SlotRange> {
    match name {
        "contents" => return Some(SlotRange::Single(0)),
        "weapon" | "weapon.mainhand" => return Some(SlotRange::Single(98)),
        "weapon.offhand" => return Some(SlotRange::Single(99)),
        "armor.head" => return Some(SlotRange::Single(103)),
        "armor.chest" => return Some(SlotRange::Single(102)),
        "armor.legs" => return Some(SlotRange::Single(101)),
        "armor.feet" => return Some(SlotRange::Single(100)),
        "armor.body" => return Some(SlotRange::Single(105)),
        "saddle" => return Some(SlotRange::Single(106)),
        "horse.chest" | "player.cursor" => return Some(SlotRange::Single(499)),
        _ => {}
    }
    for (prefix, first, size) in SLOT_RANGE_PREFIXES {
        let Some(suffix) = name.strip_prefix(prefix) else {
            continue;
        };
        if suffix == "*" {
            return Some(SlotRange::Multiple);
        }
        return suffix
            .parse::<i32>()
            .ok()
            .filter(|index| *index >= 0 && *index < size)
            .map(|index| SlotRange::Single(first + index));
    }
    None
}

/// Vanilla `SlotRanges.singleSlotNames`.
fn single_slot_names() -> Vec<String> {
    let mut names = vec!["contents".to_owned()];
    for (prefix, _, size) in SLOT_RANGE_PREFIXES {
        names.extend((0..size).map(|index| format!("{prefix}{index}")));
    }
    for name in [
        "weapon",
        "weapon.mainhand",
        "weapon.offhand",
        "armor.head",
        "armor.chest",
        "armor.legs",
        "armor.feet",
        "armor.body",
        "saddle",
        "horse.chest",
        "player.cursor",
    ] {
        names.push(name.to_owned());
    }
    names
}

/// Reads a namespaced identifier, matching vanilla `Identifier.read`.
fn read_identifier<'input>(reader: &mut StringReader<'input>) -> &'input str {
    let start = reader.read_so_far().len();
    while reader.peek().is_some_and(is_allowed_in_identifier) {
        reader.skip();
    }
    &reader.read_so_far()[start..]
}

const fn is_allowed_in_identifier(character: char) -> bool {
    character.is_ascii_digit()
        || character.is_ascii_lowercase()
        || matches!(character, '_' | ':' | '/' | '.' | '-')
}

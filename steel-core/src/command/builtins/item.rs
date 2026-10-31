//! Vanilla `/item` command.
//!
//! Mirrors `net.minecraft.server.commands.item.{ItemCommands, ItemAccessor,
//! EntityItemAccessor, BlockItemAccessor}` and the `ArgProvider` scaffolding
//! that builds the four subtrees: `replace`/`fill`/`override` each take a
//! target, a slot source, and either a literal item or a source slot source;
//! `modify` applies an `item_modifier` to the selected slots.
//!
//! Command syntax, exactly as vanilla builds it:
//!
//! ```text
//! item replace entity <targets> <slots> with <item> [<count>]
//! item replace entity <targets> <slots> from entity <source> <sourceSlots> [<modifier>]
//! item replace entity <targets> <slots> from block <source> <sourceSlots> [<modifier>]
//! item replace block <target> <slots> ... (same two branches)
//! item fill|override ... (same, with the cycling / fallback item provider)
//! item modify entity <targets> <slots> <modifier>
//! item modify block <target> <slots> <modifier>
//! ```
//!
//! Intentional divergence: vanilla pushes a visited entry for the applied
//! `LootItemFunction` so a self-referencing `item_modifier` cannot recurse
//! forever. Steel's registry-side `LootContext` has no visited-element stack,
//! so a cyclic modifier reference would recurse; vanilla's own modifiers are
//! acyclic and Steel's registry is generated from them.

use std::{cell::Cell, rc::Rc, sync::Arc};

use steel_registry::{
    data_components::vanilla_components::{CUSTOM_NAME, ITEM_NAME},
    item_stack::ItemStack,
    loot_table::LootContext,
};
use steel_utils::{BlockPos, Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, ItemModifierRef, SlotSourceArg, SlotSourceName, SteelArgumentType,
        SteelCommandContext, SteelCommandRuntime, argument, literal,
    },
    registration::CommandRegistration,
};
use super::execute::loaded_block_position;
use crate::{
    entity::{SharedEntity, entity_loot_ref, loot_equipment},
    inventory::lock::{ContainerLockGuard, ContainerRef},
    loot::slot::{
        AnySelector, ArrayItemProvider, ContainerSlots, CycledItemProvider, EntitySlots,
        ItemProvider, NonEmptySelector, SlotAccess, SlotCollection, SlotProvider,
        SlotSourceContext, TrackingSelector, provide_slot_range, provide_slot_source,
    },
    world::{LevelReader as _, World},
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("item"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("item")
        .then(set_items_branch("replace", Distributor::Replace))
        .then(set_items_branch("fill", Distributor::Fill))
        .then(set_items_branch("override", Distributor::Override))
        .then(modify_branch())
}

/// Which target an `item` subcommand acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetKind {
    /// Vanilla `EntityItemAccessor`.
    Entity,
    /// Vanilla `BlockItemAccessor`.
    Block,
}

/// Which source the `from` branch copies out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceKind {
    /// Vanilla's `entity` source provider.
    Entity,
    /// Vanilla's `block` source provider.
    Block,
}

/// Whether a slot list is being read (`from`) or written (`with`/`modify`),
/// which selects vanilla's `source`/`target` error messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotRole {
    Target,
    Source,
}

/// Vanilla `ItemDistributor`: how a collection of stacks feeds the slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Distributor {
    /// Vanilla `replaceItemProvider` (`ItemProvider.of`).
    Replace,
    /// Vanilla `fillItemProvider` (`ItemProvider.cycle`).
    Fill,
    /// Vanilla `overrideItemProvider` (`of(...).orElseProvide(EMPTY)`).
    Override,
}

impl Distributor {
    /// Vanilla `ItemDistributor.apply`.
    fn item_provider(self, stacks: Vec<ItemStack>) -> Box<dyn ItemProvider> {
        match self {
            Self::Replace => Box::new(ArrayItemProvider::new(stacks)),
            Self::Fill => Box::new(CycledItemProvider::new(stacks)),
            Self::Override => {
                let provider: Box<dyn ItemProvider> = Box::new(ArrayItemProvider::new(stacks));
                provider.or_else_provide(ItemStack::empty())
            }
        }
    }
}

/// Vanilla `wrapSetItems` for one `replace`/`fill`/`override` subtree.
fn set_items_branch(
    name: &'static str,
    distributor: Distributor,
) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal(name)
        .then(
            literal("entity").then(
                argument("targets", SteelArgumentType::entities())
                    .then(set_items_slots(TargetKind::Entity, distributor)),
            ),
        )
        .then(
            literal("block").then(
                argument("target", SteelArgumentType::block_pos())
                    .then(set_items_slots(TargetKind::Block, distributor)),
            ),
        )
}

/// The `<slots>` node shared by every set-items operation.
fn set_items_slots(
    target: TargetKind,
    distributor: Distributor,
) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    argument("slots", SteelArgumentType::slot_source())
        .then(copy_branch(target, distributor, SourceKind::Entity))
        .then(copy_branch(target, distributor, SourceKind::Block))
        .then(with_branch(target, distributor))
}

/// Vanilla's `from <provider>` branch of `wrapSetItems`.
fn copy_branch(
    target: TargetKind,
    distributor: Distributor,
    source_kind: SourceKind,
) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    // Vanilla `ArgProvider.wrap`: the source provider's argument node carries
    // the `<sourceSlots>` subtree.
    let source_slots = argument("sourceSlots", SteelArgumentType::slot_source())
        .executes(move |context| copy_items(context, target, source_kind, distributor, false))
        .then(
            argument("modifier", SteelArgumentType::item_modifier()).executes(move |context| {
                copy_items(context, target, source_kind, distributor, true)
            }),
        );
    let provider = match source_kind {
        SourceKind::Entity => literal("entity")
            .then(argument("source", SteelArgumentType::entities()).then(source_slots)),
        SourceKind::Block => literal("block")
            .then(argument("source", SteelArgumentType::block_pos()).then(source_slots)),
    };
    literal("from").then(provider)
}

/// Vanilla's `with <item> [<count>]` branch of `wrapSetItems`.
fn with_branch(
    target: TargetKind,
    distributor: Distributor,
) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("with").then(
        argument("item", SteelArgumentType::item_stack())
            .executes(move |context| set_literal_item(context, target, distributor, 1))
            .then(
                argument("count", ArgumentType::integer(1, 99)).executes(move |context| {
                    let count = context.integer("count")?;
                    set_literal_item(context, target, distributor, count)
                }),
            ),
    )
}

/// Vanilla's `modify` subtree.
fn modify_branch() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("modify")
        .then(
            literal("entity").then(
                argument("targets", SteelArgumentType::entities())
                    .then(modify_slots_node(TargetKind::Entity)),
            ),
        )
        .then(
            literal("block").then(
                argument("target", SteelArgumentType::block_pos())
                    .then(modify_slots_node(TargetKind::Block)),
            ),
        )
}

fn modify_slots_node(target: TargetKind) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    argument("slots", SteelArgumentType::slot_source()).then(
        argument("modifier", SteelArgumentType::item_modifier())
            .executes(move |context| modify_items(context, target)),
    )
}

/// Vanilla `ItemCommands.setItem`: replace the selected target slots with
/// `count` copies of the `item` argument.
fn set_literal_item(
    context: &SteelCommandContext<CommandSource>,
    target: TargetKind,
    distributor: Distributor,
    count: i32,
) -> Result<i32, CommandSyntaxError> {
    let prototype = context.item_stack("item")?.copy_with_count(count);
    let slots = context.slot_source("slots")?;
    let items = vec![prototype.clone()];
    match target {
        TargetKind::Entity => set_items_with_provider(
            context,
            &entity_accessor(context)?,
            slots,
            distributor,
            items,
            Some(&prototype),
        ),
        TargetKind::Block => set_items_with_provider(
            context,
            &block_accessor(context)?,
            slots,
            distributor,
            items,
            Some(&prototype),
        ),
    }
}

/// Vanilla `ItemCommands.getItems` + `setItems(Collection)`: copy the source
/// slots into the target slots through the distributor.
fn copy_items(
    context: &SteelCommandContext<CommandSource>,
    target: TargetKind,
    source_kind: SourceKind,
    distributor: Distributor,
    with_modifier: bool,
) -> Result<i32, CommandSyntaxError> {
    let slots = context.slot_source("slots")?;
    let source_slots = context.slot_source("sourceSlots")?;
    let modifier = if with_modifier {
        Some(context.item_modifier("modifier")?)
    } else {
        None
    };
    let items = match source_kind {
        SourceKind::Entity => {
            let accessor = entity_accessor_from(context, "source")?;
            source_items(context, &accessor, source_slots, modifier)?
        }
        SourceKind::Block => {
            let accessor = block_accessor_from(context, "source")?;
            source_items(context, &accessor, source_slots, modifier)?
        }
    };
    match target {
        TargetKind::Entity => set_items_with_provider(
            context,
            &entity_accessor(context)?,
            slots,
            distributor,
            items,
            None,
        ),
        TargetKind::Block => set_items_with_provider(
            context,
            &block_accessor(context)?,
            slots,
            distributor,
            items,
            None,
        ),
    }
}

/// Vanilla `ItemCommands.modifyItems`.
fn modify_items(
    context: &SteelCommandContext<CommandSource>,
    target: TargetKind,
) -> Result<i32, CommandSyntaxError> {
    let slots = context.slot_source("slots")?;
    let modifier = context.item_modifier("modifier")?;

    let selected = Rc::new(Cell::new(0usize));
    let mut selector = TrackingSelector::tracking(NonEmptySelector, &selected);

    let equipment = context
        .source()
        .entity()
        .and_then(|entity| loot_equipment(entity.as_ref()));
    let this_entity = context
        .source()
        .entity()
        .map(|entity| entity_loot_ref(entity.as_ref(), equipment.as_ref()));
    let position = context.source().position();
    let mut rng = rand::rng();
    let mut loot_context =
        LootContext::new(&mut rng).with_origin(position.x, position.y, position.z);
    if let Some(this_entity) = this_entity {
        loot_context = loot_context.with_this_entity(this_entity);
    }
    let mut counting = CountingModifier {
        modifier,
        context: &mut loot_context,
        updated: 0,
    };

    let mut tracker = ResponseTracker::new();
    match target {
        TargetKind::Entity => {
            let accessor = entity_accessor(context)?;
            accessor.set_items(context.source(), slots, &mut |element, collection| {
                let before = counting.updated;
                collection.modify_slots(&mut |slot| counting.accept(slot), &mut selector);
                let changed = counting.updated - before;
                tracker.track(element, changed);
                changed
            })?;
            finish_modify(context, &accessor, slots, &selected, &tracker)
        }
        TargetKind::Block => {
            let accessor = block_accessor(context)?;
            accessor.set_items(context.source(), slots, &mut |element, collection| {
                let before = counting.updated;
                collection.modify_slots(&mut |slot| counting.accept(slot), &mut selector);
                let changed = counting.updated - before;
                tracker.track(element, changed);
                changed
            })?;
            finish_modify(context, &accessor, slots, &selected, &tracker)
        }
    }
}

/// Vanilla `ItemCommands.setItems(source, accessor, slots, provider, knownItem)`.
fn set_items_with_provider(
    context: &SteelCommandContext<CommandSource>,
    accessor: &impl ItemAccessor,
    slots: SlotSourceArg,
    distributor: Distributor,
    items: Vec<ItemStack>,
    known_item: Option<&ItemStack>,
) -> Result<i32, CommandSyntaxError> {
    let selected = Rc::new(Cell::new(0usize));
    let mut selector = TrackingSelector::tracking(AnySelector, &selected);
    let mut provider = distributor.item_provider(items);

    let mut tracker = ResponseTracker::new();
    accessor.set_items(context.source(), slots, &mut |element, collection| {
        provider.restart();
        let replaced = collection.replace_slot_items(provider.as_mut(), &mut selector);
        let changed = i32::try_from(replaced).unwrap_or(i32::MAX);
        tracker.track(element, changed);
        changed
    })?;

    if selected.get() == 0 {
        return Err(inapplicable_slot_error(slots.name(), SlotRole::Target));
    }
    accessor.replace_success(context.source(), &tracker, known_item)
}

/// Vanilla `ItemCommands.getItems`: the source slots' item copies, with the
/// optional modifier applied to each copy.
fn source_items(
    context: &SteelCommandContext<CommandSource>,
    accessor: &impl ItemAccessor,
    slots: SlotSourceArg,
    modifier: Option<ItemModifierRef>,
) -> Result<Vec<ItemStack>, CommandSyntaxError> {
    let mut items = accessor.get_slots(context.source(), slots)?;
    if let Some(modifier) = modifier {
        let equipment = context
            .source()
            .entity()
            .and_then(|entity| loot_equipment(entity.as_ref()));
        let this_entity = context
            .source()
            .entity()
            .map(|entity| entity_loot_ref(entity.as_ref(), equipment.as_ref()));
        let position = context.source().position();
        let mut rng = rand::rng();
        let mut loot_context =
            LootContext::new(&mut rng).with_origin(position.x, position.y, position.z);
        if let Some(this_entity) = this_entity {
            loot_context = loot_context.with_this_entity(this_entity);
        }
        for item in &mut items {
            modifier.function.apply(item, &mut loot_context);
            clamp_to_max_stack_size(item);
        }
    }
    if items.is_empty() {
        return Err(inapplicable_slot_error(slots.name(), SlotRole::Source));
    }
    Ok(items)
}

/// Vanilla `modifyItems`' tail: the inapplicable-slot check and the success
/// feedback.
fn finish_modify(
    context: &SteelCommandContext<CommandSource>,
    accessor: &impl ItemAccessor,
    slots: SlotSourceArg,
    selected: &Rc<Cell<usize>>,
    tracker: &ResponseTracker,
) -> Result<i32, CommandSyntaxError> {
    if selected.get() == 0 {
        return Err(inapplicable_slot_error(slots.name(), SlotRole::Target));
    }
    accessor.modify_success(context.source(), tracker)
}

/// Vanilla `ItemCommands.applyModifier`'s `ItemStack.limitSize`.
fn clamp_to_max_stack_size(stack: &mut ItemStack) {
    let max_stack_size = stack.max_stack_size();
    if stack.count() > max_stack_size {
        stack.set_count(max_stack_size);
    }
}

/// Vanilla `CountingModifier`: applies the modifier to a copy and writes it
/// back, counting the slots that accepted the result.
struct CountingModifier<'c, 'a, R: rand::Rng> {
    modifier: ItemModifierRef,
    context: &'c mut LootContext<'a, R>,
    updated: i32,
}

impl<R: rand::Rng> CountingModifier<'_, '_, R> {
    /// Vanilla `CountingModifier.accept`.
    fn accept(&mut self, slot: &dyn SlotAccess) {
        let mut stack = slot.get();
        self.modifier.function.apply(&mut stack, self.context);
        clamp_to_max_stack_size(&mut stack);
        if slot.set(stack) {
            self.updated += 1;
        }
    }
}

/// The tracked element of a `/item` response, mirroring vanilla's two
/// `ItemAccessor` type parameters.
#[derive(Clone)]
enum ItemElement {
    /// One target entity.
    Entity(SharedEntity),
    /// The target block position.
    Block(BlockPos),
}

/// Vanilla `ItemAccessor`: how a command target hands out (and writes back)
/// item slots.
trait ItemAccessor {
    /// Vanilla `ItemAccessor.setItems`.
    fn set_items(
        &self,
        source: &CommandSource,
        slots: SlotSourceArg,
        setter: &mut dyn FnMut(ItemElement, &dyn SlotCollection) -> i32,
    ) -> Result<(), CommandSyntaxError>;

    /// Vanilla `ItemAccessor.getSlots`, reduced to the item copies `getItems`
    /// consumes.
    fn get_slots(
        &self,
        source: &CommandSource,
        slots: SlotSourceArg,
    ) -> Result<Vec<ItemStack>, CommandSyntaxError>;

    /// Vanilla `ItemAccessor.getReplaceSuccess`.
    fn replace_success(
        &self,
        source: &CommandSource,
        tracker: &ResponseTracker,
        known_item: Option<&ItemStack>,
    ) -> Result<i32, CommandSyntaxError>;

    /// Vanilla `ItemAccessor.getModifySuccess`.
    fn modify_success(
        &self,
        source: &CommandSource,
        tracker: &ResponseTracker,
    ) -> Result<i32, CommandSyntaxError>;
}

/// Vanilla `CommandResponseTracker<Element>` restricted to the `NON_ZERO`
/// element set, which is the set every `/item` operation reads.
struct ResponseTracker {
    total_value: i32,
    only_non_zero_element: Option<ItemElement>,
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

    /// Vanilla `CommandResponseTracker.track(element, value)`.
    fn track(&mut self, element: ItemElement, value: i32) {
        self.total_value += value;
        if value != 0 {
            self.non_zero_element_count += 1;
            self.only_non_zero_element = if self.non_zero_element_count == 1 {
                Some(element)
            } else {
                None
            };
        }
    }

    /// Vanilla `CommandResponseTracker.Messages.throwIfZero`.
    fn require_changes(&self, known_item: Option<&ItemStack>) -> Result<(), CommandSyntaxError> {
        if self.non_zero_element_count != 0 {
            return Ok(());
        }
        Err(match known_item {
            Some(item) => CommandSyntaxError::dynamic(
                translations::COMMANDS_ITEM_TARGET_FAILED_KNOWN_ITEM
                    .message([item_display_name(item)])
                    .component(),
            ),
            None => CommandSyntaxError::dynamic(
                translations::COMMANDS_ITEM_TARGET_FAILED
                    .message([] as [TextComponent; 0])
                    .component(),
            ),
        })
    }
}

/// Vanilla `EntityItemAccessor`.
struct EntityItemAccessor {
    entities: Vec<SharedEntity>,
}

impl EntityItemAccessor {
    const fn new(entities: Vec<SharedEntity>) -> Self {
        Self { entities }
    }
}

impl ItemAccessor for EntityItemAccessor {
    fn set_items(
        &self,
        source: &CommandSource,
        slots: SlotSourceArg,
        setter: &mut dyn FnMut(ItemElement, &dyn SlotCollection) -> i32,
    ) -> Result<(), CommandSyntaxError> {
        let source_entity = source.entity().map(AsRef::as_ref);
        let source_provider = source_entity.map(EntitySlots::new);
        for entity in &self.entities {
            let provider = EntitySlots::new(entity.as_ref());
            let collection = slots_from_provider(
                &provider,
                source_provider
                    .as_ref()
                    .map(|source| source as &dyn SlotProvider),
                slots,
            );
            let changed = setter(ItemElement::Entity(Arc::clone(entity)), collection.as_ref());
            // Vanilla `EntityItemAccessor.setItems`: a changed server player
            // pushes the new inventory to its open container menu.
            if changed > 0
                && let Some(player) = entity.as_player()
            {
                player.broadcast_inventory_changes();
            }
        }
        Ok(())
    }

    fn get_slots(
        &self,
        source: &CommandSource,
        slots: SlotSourceArg,
    ) -> Result<Vec<ItemStack>, CommandSyntaxError> {
        let source_entity = source.entity().map(AsRef::as_ref);
        let source_provider = source_entity.map(EntitySlots::new);
        let mut copies = Vec::new();
        for entity in &self.entities {
            let provider = EntitySlots::new(entity.as_ref());
            copies.extend(
                slots_from_provider(
                    &provider,
                    source_provider
                        .as_ref()
                        .map(|source| source as &dyn SlotProvider),
                    slots,
                )
                .item_copies(),
            );
        }
        Ok(copies)
    }

    fn replace_success(
        &self,
        source: &CommandSource,
        tracker: &ResponseTracker,
        known_item: Option<&ItemStack>,
    ) -> Result<i32, CommandSyntaxError> {
        tracker.require_changes(known_item)?;
        let message = match (known_item, tracker.non_zero_element_count) {
            (Some(item), 1) => translations::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_SINGLE_KNOWN_ITEM
                .message([
                    slot_count(tracker.total_value),
                    single_entity_name(tracker),
                    item_display_name(item),
                ])
                .component(),
            (Some(item), _) => {
                translations::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_MULTIPLE_KNOWN_ITEM
                    .message([
                        slot_count(tracker.non_zero_element_count),
                        item_display_name(item),
                    ])
                    .component()
            }
            (None, 1) => translations::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_SINGLE
                .message([slot_count(tracker.total_value), single_entity_name(tracker)])
                .component(),
            (None, _) => translations::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_MULTIPLE
                .message([slot_count(tracker.non_zero_element_count)])
                .component(),
        };
        source.send_success(&message, true);
        Ok(tracker.total_value)
    }

    fn modify_success(
        &self,
        source: &CommandSource,
        tracker: &ResponseTracker,
    ) -> Result<i32, CommandSyntaxError> {
        tracker.require_changes(None)?;
        let message = if tracker.non_zero_element_count == 1 {
            translations::COMMANDS_ITEM_ENTITY_MODIFY_SUCCESS_SINGLE
                .message([slot_count(tracker.total_value), single_entity_name(tracker)])
                .component()
        } else {
            translations::COMMANDS_ITEM_ENTITY_MODIFY_SUCCESS_MULTIPLE
                .message([slot_count(tracker.non_zero_element_count)])
                .component()
        };
        source.send_success(&message, true);
        Ok(tracker.total_value)
    }
}

/// Vanilla `BlockItemAccessor`.
struct BlockItemAccessor {
    pos: BlockPos,
}

impl BlockItemAccessor {
    const fn new(pos: BlockPos) -> Self {
        Self { pos }
    }

    /// Vanilla `BlockItemAccessor.getContainer`.
    fn container(
        &self,
        world: &Arc<World>,
        role: SlotRole,
    ) -> Result<ContainerRef, CommandSyntaxError> {
        world
            .get_block_entity(self.pos)
            .and_then(ContainerRef::from_block_entity)
            .ok_or_else(|| not_a_container_error(self.pos, role))
    }
}

impl ItemAccessor for BlockItemAccessor {
    fn set_items(
        &self,
        source: &CommandSource,
        slots: SlotSourceArg,
        setter: &mut dyn FnMut(ItemElement, &dyn SlotCollection) -> i32,
    ) -> Result<(), CommandSyntaxError> {
        let container = self.container(source.world(), SlotRole::Target)?;
        let id = container.container_id();
        let mut guard = ContainerLockGuard::lock_all(&[&container]);
        let container_slots = ContainerSlots::new(&mut guard, id);
        let source_provider = source
            .entity()
            .map(|entity| EntitySlots::new(entity.as_ref()));
        let collection = slots_from_provider(
            &container_slots,
            source_provider
                .as_ref()
                .map(|source| source as &dyn SlotProvider),
            slots,
        );
        setter(ItemElement::Block(self.pos), collection.as_ref());
        Ok(())
    }

    fn get_slots(
        &self,
        source: &CommandSource,
        slots: SlotSourceArg,
    ) -> Result<Vec<ItemStack>, CommandSyntaxError> {
        let container = self.container(source.world(), SlotRole::Source)?;
        let id = container.container_id();
        let mut guard = ContainerLockGuard::lock_all(&[&container]);
        let container_slots = ContainerSlots::new(&mut guard, id);
        let source_provider = source
            .entity()
            .map(|entity| EntitySlots::new(entity.as_ref()));
        let collection = slots_from_provider(
            &container_slots,
            source_provider
                .as_ref()
                .map(|source| source as &dyn SlotProvider),
            slots,
        );
        Ok(collection.item_copies())
    }

    fn replace_success(
        &self,
        source: &CommandSource,
        tracker: &ResponseTracker,
        known_item: Option<&ItemStack>,
    ) -> Result<i32, CommandSyntaxError> {
        tracker.require_changes(known_item)?;
        let message = match known_item {
            Some(item) => translations::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS_KNOWN_ITEM
                .message([
                    slot_count(tracker.total_value),
                    self.pos_component(tracker, 0),
                    self.pos_component(tracker, 1),
                    self.pos_component(tracker, 2),
                    item_display_name(item),
                ])
                .component(),
            None => translations::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS
                .message([
                    slot_count(tracker.total_value),
                    self.pos_component(tracker, 0),
                    self.pos_component(tracker, 1),
                    self.pos_component(tracker, 2),
                ])
                .component(),
        };
        source.send_success(&message, true);
        Ok(tracker.total_value)
    }

    fn modify_success(
        &self,
        source: &CommandSource,
        tracker: &ResponseTracker,
    ) -> Result<i32, CommandSyntaxError> {
        tracker.require_changes(None)?;
        let message = translations::COMMANDS_ITEM_BLOCK_MODIFY_SUCCESS
            .message([
                slot_count(tracker.total_value),
                self.pos_component(tracker, 0),
                self.pos_component(tracker, 1),
                self.pos_component(tracker, 2),
            ])
            .component();
        source.send_success(&message, true);
        Ok(tracker.total_value)
    }
}

impl BlockItemAccessor {
    /// Vanilla's block success messages read the tracked `BlockPos` element.
    const fn reported_pos(&self, tracker: &ResponseTracker) -> BlockPos {
        match tracker.only_non_zero_element {
            Some(ItemElement::Block(pos)) => pos,
            Some(ItemElement::Entity(_)) | None => self.pos,
        }
    }

    /// One coordinate of the reported block position.
    fn pos_component(&self, tracker: &ResponseTracker, axis: usize) -> TextComponent {
        let pos = self.reported_pos(tracker);
        let coordinate = match axis {
            0 => pos.x(),
            1 => pos.y(),
            _ => pos.z(),
        };
        TextComponent::from(coordinate.to_string())
    }
}

fn entity_accessor(
    context: &SteelCommandContext<CommandSource>,
) -> Result<EntityItemAccessor, CommandSyntaxError> {
    Ok(EntityItemAccessor::new(context.entities("targets")?))
}

fn entity_accessor_from(
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<EntityItemAccessor, CommandSyntaxError> {
    Ok(EntityItemAccessor::new(context.entities(name)?))
}

fn block_accessor(
    context: &SteelCommandContext<CommandSource>,
) -> Result<BlockItemAccessor, CommandSyntaxError> {
    Ok(BlockItemAccessor::new(loaded_block_position(
        context, "target",
    )?))
}

fn block_accessor_from(
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<BlockItemAccessor, CommandSyntaxError> {
    Ok(BlockItemAccessor::new(loaded_block_position(
        context, name,
    )?))
}

/// Vanilla `ExecuteCommand.countItems`: the total count of source stacks whose
/// item predicate matches, using the same `ItemAccessor` slot readers as `/item`.
pub(super) fn count_matching_items(
    context: &SteelCommandContext<CommandSource>,
    source_kind: ItemSourceKind,
    slots_name: &str,
    predicate_name: &str,
) -> Result<i32, CommandSyntaxError> {
    let slots = context.slot_source(slots_name)?;
    let predicate = context.item_predicate(predicate_name)?;
    let source = context.source();
    let stacks = match source_kind {
        ItemSourceKind::Entity => {
            EntityItemAccessor::new(context.entities("source")?).get_slots(source, slots)?
        }
        ItemSourceKind::Block => BlockItemAccessor::new(loaded_block_position(context, "source")?)
            .get_slots(source, slots)?,
    };
    Ok(stacks
        .iter()
        .filter(|stack| predicate.matches(stack))
        .fold(0, |total, stack| total.wrapping_add(stack.count())))
}

/// Which `SOURCE_PROVIDERS` entry `execute if items` reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ItemSourceKind {
    Entity,
    Block,
}

/// Vanilla `ItemCommands.getSlotsFromProvider`: builds the slot-source context
/// (container = the target provider, this-entity = the command source entity)
/// and provides the slots.
fn slots_from_provider<'a>(
    provider: &'a dyn SlotProvider,
    source_entity: Option<&'a dyn SlotProvider>,
    source: SlotSourceArg,
) -> Box<dyn SlotCollection + 'a> {
    let context = SlotSourceContext {
        container: Some(provider),
        this_entity: source_entity,
        block_entity: None,
    };
    match source {
        SlotSourceArg::Literal(range) => provide_slot_range(range, &context),
        SlotSourceArg::Named(entry) => provide_slot_source(&entry.source, &context),
    }
}

/// Vanilla `ItemCommands.ERROR_TARGET_INAPPLICABLE_SLOT` /
/// `ERROR_SOURCE_INAPPLICABLE_SLOT`.
fn inapplicable_slot_error(name: SlotSourceName, role: SlotRole) -> CommandSyntaxError {
    let name = TextComponent::plain(name.to_string());
    let message = match role {
        SlotRole::Target => &translations::COMMANDS_ITEM_TARGET_NO_SUCH_SLOT,
        SlotRole::Source => &translations::COMMANDS_ITEM_SOURCE_NO_SUCH_SLOT,
    }
    .message([name])
    .component();
    CommandSyntaxError::dynamic(message)
}

/// Vanilla `ERROR_TARGET_NOT_A_CONTAINER` / `ERROR_SOURCE_NOT_A_CONTAINER`.
fn not_a_container_error(pos: BlockPos, role: SlotRole) -> CommandSyntaxError {
    let coordinates = [
        TextComponent::from(pos.x().to_string()),
        TextComponent::from(pos.y().to_string()),
        TextComponent::from(pos.z().to_string()),
    ];
    let message = match role {
        SlotRole::Target => &translations::COMMANDS_ITEM_TARGET_NOT_A_CONTAINER,
        SlotRole::Source => &translations::COMMANDS_ITEM_SOURCE_NOT_A_CONTAINER,
    }
    .message(coordinates)
    .component();
    CommandSyntaxError::dynamic(message)
}

fn slot_count(count: i32) -> TextComponent {
    TextComponent::from(count.to_string())
}

fn single_entity_name(tracker: &ResponseTracker) -> TextComponent {
    match tracker.only_non_zero_element.as_ref() {
        Some(ItemElement::Entity(entity)) => entity.display_name(),
        Some(ItemElement::Block(_)) | None => TextComponent::from(""),
    }
}

/// Vanilla `ItemStack.getDisplayName`.
fn item_display_name(stack: &ItemStack) -> TextComponent {
    stack
        .get(CUSTOM_NAME)
        .or_else(|| stack.get(ITEM_NAME))
        .cloned()
        .unwrap_or_else(|| TextComponent::plain(stack.item().key.to_string()))
}
#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::super::create_dispatcher;
    use super::*;
    use crate::command::brigadier::{CommandDispatcher, NodeId};

    type Dispatcher = CommandDispatcher<CommandSource, SteelCommandRuntime>;

    fn child(dispatcher: &Dispatcher, parent: NodeId, name: &str) -> NodeId {
        let Some(children) = dispatcher.children(parent) else {
            panic!("parent node should exist");
        };
        let Some(child) = children.iter().copied().find(|child| {
            dispatcher
                .node(*child)
                .is_some_and(|node| node.name() == name)
        }) else {
            panic!("child {name} should exist");
        };
        child
    }

    fn item_root() -> (Dispatcher, NodeId) {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let root = child(&dispatcher, dispatcher.root(), "item");
        (dispatcher, root)
    }

    fn argument_of(dispatcher: &Dispatcher, node: NodeId) -> Option<&SteelArgumentType> {
        dispatcher.node(node).and_then(|node| node.argument_type())
    }

    /// Vanilla `ItemCommands` builds `replace`, `fill` and `override` from the
    /// same `wrapSetItems` template; the target providers are `entity` and
    /// `block`, in that order.
    #[test]
    fn set_items_subtrees_have_entity_and_block_targets() {
        let (dispatcher, item) = item_root();
        for name in ["replace", "fill", "override"] {
            let subtree = child(&dispatcher, item, name);
            let entity = child(&dispatcher, subtree, "entity");
            let targets = child(&dispatcher, entity, "targets");
            assert_eq!(
                argument_of(&dispatcher, targets),
                Some(&SteelArgumentType::entities())
            );
            let slots = child(&dispatcher, targets, "slots");
            assert_eq!(
                argument_of(&dispatcher, slots),
                Some(&SteelArgumentType::slot_source())
            );

            let block = child(&dispatcher, subtree, "block");
            let target = child(&dispatcher, block, "target");
            assert_eq!(
                argument_of(&dispatcher, target),
                Some(&SteelArgumentType::block_pos())
            );
            let block_slots = child(&dispatcher, target, "slots");
            assert_eq!(
                argument_of(&dispatcher, block_slots),
                Some(&SteelArgumentType::slot_source())
            );
        }
    }

    /// Vanilla `wrapSetItems` attaches `from entity`, `from block` and `with`
    /// to the `<slots>` node, and `count` only under `with`.
    #[test]
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the private CommandNode type cannot be named from this module"
    )]
    fn replace_slots_node_offers_from_and_with() {
        let (dispatcher, item) = item_root();
        let replace = child(&dispatcher, item, "replace");
        let entity = child(&dispatcher, replace, "entity");
        let targets = child(&dispatcher, entity, "targets");
        let slots = child(&dispatcher, targets, "slots");

        let with = child(&dispatcher, slots, "with");
        let item_argument = child(&dispatcher, with, "item");
        assert_eq!(
            argument_of(&dispatcher, item_argument),
            Some(&SteelArgumentType::item_stack())
        );
        assert!(
            dispatcher
                .node(item_argument)
                .is_some_and(|node| node.is_executable())
        );
        let count = child(&dispatcher, item_argument, "count");
        assert_eq!(
            argument_of(&dispatcher, count),
            Some(&SteelArgumentType::from(ArgumentType::integer(1, 99)))
        );

        let from = child(&dispatcher, slots, "from");
        let source_entity = child(&dispatcher, from, "entity");
        let source = child(&dispatcher, source_entity, "source");
        assert_eq!(
            argument_of(&dispatcher, source),
            Some(&SteelArgumentType::entities())
        );
        let source_slots = child(&dispatcher, source, "sourceSlots");
        assert_eq!(
            argument_of(&dispatcher, source_slots),
            Some(&SteelArgumentType::slot_source())
        );
        assert!(
            dispatcher
                .node(source_slots)
                .is_some_and(|node| node.is_executable())
        );
        let modifier = child(&dispatcher, source_slots, "modifier");
        assert_eq!(
            argument_of(&dispatcher, modifier),
            Some(&SteelArgumentType::item_modifier())
        );

        let source_block = child(&dispatcher, from, "block");
        let source_pos = child(&dispatcher, source_block, "source");
        assert_eq!(
            argument_of(&dispatcher, source_pos),
            Some(&SteelArgumentType::block_pos())
        );
    }

    /// Vanilla `modify` takes a slot source and an `item_modifier` on both
    /// target providers.
    #[test]
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the private CommandNode type cannot be named from this module"
    )]
    fn modify_takes_slot_source_and_item_modifier() {
        let (dispatcher, item) = item_root();
        let modify = child(&dispatcher, item, "modify");
        // `entity` and `block` use different argument names, so walk both.
        let entity = child(&dispatcher, modify, "entity");
        let targets = child(&dispatcher, entity, "targets");
        let entity_slots = child(&dispatcher, targets, "slots");
        assert_eq!(
            argument_of(&dispatcher, entity_slots),
            Some(&SteelArgumentType::slot_source())
        );
        let entity_modifier = child(&dispatcher, entity_slots, "modifier");
        assert_eq!(
            argument_of(&dispatcher, entity_modifier),
            Some(&SteelArgumentType::item_modifier())
        );
        assert!(
            dispatcher
                .node(entity_modifier)
                .is_some_and(|node| node.is_executable())
        );

        let block = child(&dispatcher, modify, "block");
        let target = child(&dispatcher, block, "target");
        let block_slots = child(&dispatcher, target, "slots");
        assert_eq!(
            argument_of(&dispatcher, block_slots),
            Some(&SteelArgumentType::slot_source())
        );
        let block_modifier = child(&dispatcher, block_slots, "modifier");
        assert_eq!(
            argument_of(&dispatcher, block_modifier),
            Some(&SteelArgumentType::item_modifier())
        );
    }
}

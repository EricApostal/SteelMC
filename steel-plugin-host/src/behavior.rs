//! Rust-side adapters for extension-provided block and item behaviours.

use std::sync::{Arc, Weak};

use steel_abi::{
    STEEL_HAND_MAIN, STEEL_INTERACTION_CONSUME, STEEL_INTERACTION_FAIL, STEEL_INTERACTION_PASS,
    STEEL_INTERACTION_SUCCESS, STEEL_INTERACTION_SUCCESS_SERVER,
    STEEL_INTERACTION_SUCCESS_WITHOUT_ITEM, STEEL_INTERACTION_TRY_EMPTY_HAND, STEEL_REF_BLOCK,
    SteelBlockHooks, SteelBlockState, SteelInteractionResult, SteelItemHooks, SteelPlaceContext,
};
use steel_core::behavior::BlockEntityCreation;
use steel_core::behavior::{
    BlockBehavior, BlockPlaceContext, DefaultBlockBehavior, DefaultItemBehavior, InteractionResult,
    ItemBehavior, UseItemContext, UseOnContext,
};
use steel_core::block_entity::{BLOCK_ENTITIES, SharedBlockEntity};
use steel_core::entity::Entity;
use steel_core::player::Player;
use steel_core::world::World;
use steel_registry::REGISTRY;
use steel_registry::RegistryEntry;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::properties::Direction;
use steel_registry::item_stack::ItemStack;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, BlockStateId as StateId};

use crate::state::{HOST, world_id};

fn direction_index(direction: Direction) -> u32 {
    match direction {
        Direction::Down => 0,
        Direction::Up => 1,
        Direction::North => 2,
        Direction::South => 3,
        Direction::West => 4,
        Direction::East => 5,
    }
}

fn hand_value(hand: InteractionHand) -> u32 {
    match hand {
        InteractionHand::MainHand => STEEL_HAND_MAIN,
        InteractionHand::OffHand => 1,
    }
}

fn interaction(result: SteelInteractionResult) -> InteractionResult {
    if !result.is_handled() {
        return InteractionResult::TryEmptyHandInteraction;
    }
    match result.kind {
        STEEL_INTERACTION_SUCCESS => InteractionResult::Success,
        STEEL_INTERACTION_SUCCESS_WITHOUT_ITEM => InteractionResult::SuccessWithoutItem,
        STEEL_INTERACTION_SUCCESS_SERVER => InteractionResult::SuccessServer,
        STEEL_INTERACTION_CONSUME => InteractionResult::Consume,
        STEEL_INTERACTION_FAIL => InteractionResult::Fail,
        STEEL_INTERACTION_PASS => InteractionResult::Pass,
        STEEL_INTERACTION_TRY_EMPTY_HAND => InteractionResult::TryEmptyHandInteraction,
        _ => InteractionResult::Pass,
    }
}

fn state(value: BlockStateId) -> SteelBlockState {
    SteelBlockState(u32::from(value.0))
}

fn entity_ref(world: &Arc<World>, player: &Player) -> steel_abi::SteelEntityRef {
    steel_abi::SteelEntityRef {
        world: steel_abi::SteelWorldRef(world_id(world)),
        entity: player.id(),
    }
}

fn place_context(context: &BlockPlaceContext<'_>) -> SteelPlaceContext {
    let world = context.world;
    let player = context.player();
    let item_stack = context.with_item(|stack| HOST.owned.lock().insert_item_stack(stack.clone()));
    let hit = context.hit_pos();
    let place = context.place_pos();
    SteelPlaceContext {
        world: steel_abi::SteelWorldRef(world_id(world)),
        hit_x: hit.x(),
        hit_y: hit.y(),
        hit_z: hit.z(),
        place_x: place.x(),
        place_y: place.y(),
        place_z: place.z(),
        clicked_face: direction_index(context.clicked_face()),
        inside: u8::from(context.is_inside()),
        replaces_clicked_block: u8::from(context.replaces_clicked_block()),
        player: player.map_or(steel_abi::SteelEntityRef::INVALID, |player| {
            entity_ref(world, player)
        }),
        player_present: u8::from(player.is_some()),
        item_stack,
    }
}

/// Block behaviour forwarded to an extension hook table. Hooks that are not
/// installed fall through to Steel's default behaviour.
pub struct ForeignBlockBehavior {
    block: BlockRef,
    default: DefaultBlockBehavior,
    hooks: SteelBlockHooks,
}

// SAFETY: hook tables are immutable after registration and the extension
// contract requires thread-safe user data for the lifetime of the library.
unsafe impl Send for ForeignBlockBehavior {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for ForeignBlockBehavior {}

impl ForeignBlockBehavior {
    #[must_use]
    pub fn new(block: BlockRef, hooks: SteelBlockHooks) -> Self {
        Self {
            block,
            default: DefaultBlockBehavior::new(block),
            hooks,
        }
    }
}

impl BlockBehavior for ForeignBlockBehavior {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let Some(hook) = self.hooks.get_state_for_placement else {
            return self.default.get_state_for_placement(context);
        };
        let abi_context = place_context(context);
        let mut out = SteelBlockState(0);
        // SAFETY: the hook table belongs to a loaded extension and the callback
        // only observes the context for the duration of this call.
        let present = unsafe { hook(self.hooks.user_data, &abi_context, &mut out) };
        if present != 0 {
            u16::try_from(out.0).ok().map(StateId)
        } else {
            None
        }
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        moved_by_piston: bool,
    ) {
        if let Some(hook) = self.hooks.on_place {
            // SAFETY: see `get_state_for_placement`.
            unsafe {
                hook(
                    self.hooks.user_data,
                    steel_abi::SteelWorldRef(world_id(world)),
                    pos.x(),
                    pos.y(),
                    pos.z(),
                    SteelBlockState(u32::from(state.0)),
                    SteelBlockState(u32::from(old_state.0)),
                    u8::from(moved_by_piston),
                );
            }
        }
        self.default
            .on_place(state, world, pos, old_state, moved_by_piston);
    }

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        hit_result: &steel_core::behavior::BlockHitResult,
        inv: &mut steel_core::behavior::InventoryAccess,
    ) -> InteractionResult {
        let Some(hook) = self.hooks.use_item_on else {
            return self
                .default
                .use_item_on(state, world, pos, player, hand, hit_result, inv);
        };
        // SAFETY: see `get_state_for_placement`.
        let result = unsafe {
            hook(
                self.hooks.user_data,
                steel_abi::SteelWorldRef(world_id(world)),
                pos.x(),
                pos.y(),
                pos.z(),
                SteelBlockState(u32::from(state.0)),
                entity_ref(world, player),
                hand_value(hand),
                direction_index(hit_result.direction),
            )
        };
        if result.is_handled() {
            interaction(result)
        } else {
            self.default
                .use_item_on(state, world, pos, player, hand, hit_result, inv)
        }
    }

    fn player_destroy(
        &self,
        world: &Arc<World>,
        player: &Player,
        pos: BlockPos,
        state: BlockStateId,
        block_entity: Option<&SharedBlockEntity>,
        tool: &ItemStack,
    ) {
        if let Some(hook) = self.hooks.player_destroy {
            let tool = HOST.owned.lock().insert_item_stack(tool.clone());
            // SAFETY: see `get_state_for_placement`.
            unsafe {
                hook(
                    self.hooks.user_data,
                    steel_abi::SteelWorldRef(world_id(world)),
                    entity_ref(world, player),
                    pos.x(),
                    pos.y(),
                    pos.z(),
                    SteelBlockState(u32::from(state.0)),
                    tool,
                );
            }
        }
        self.default
            .player_destroy(world, player, pos, state, block_entity, tool);
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if let Some(hook) = self.hooks.random_tick {
            // SAFETY: see `get_state_for_placement`.
            unsafe {
                hook(
                    self.hooks.user_data,
                    steel_abi::SteelWorldRef(world_id(world)),
                    pos.x(),
                    pos.y(),
                    pos.z(),
                    SteelBlockState(u32::from(state.0)),
                );
            }
        }
        self.default.random_tick(state, world, pos);
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if let Some(hook) = self.hooks.scheduled_tick {
            // SAFETY: see `get_state_for_placement`.
            unsafe {
                hook(
                    self.hooks.user_data,
                    steel_abi::SteelWorldRef(world_id(world)),
                    pos.x(),
                    pos.y(),
                    pos.z(),
                    SteelBlockState(u32::from(state.0)),
                );
            }
        }
        self.default.tick(state, world, pos);
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        source_block: BlockRef,
        moved_by_piston: bool,
    ) {
        if let Some(hook) = self.hooks.neighbor_changed {
            // SAFETY: see `get_state_for_placement`.
            unsafe {
                hook(
                    self.hooks.user_data,
                    steel_abi::SteelWorldRef(world_id(world)),
                    pos.x(),
                    pos.y(),
                    pos.z(),
                    SteelBlockState(u32::from(state.0)),
                    steel_abi::SteelRef::registry(STEEL_REF_BLOCK, source_block.id() as u32),
                    u8::from(moved_by_piston),
                );
            }
        }
        self.default
            .handle_neighbor_changed(state, world, pos, source_block, moved_by_piston);
    }

    fn can_be_replaced(&self, state: BlockStateId, context: &BlockPlaceContext<'_>) -> bool {
        let Some(hook) = self.hooks.can_be_replaced else {
            return self.default.can_be_replaced(state, context);
        };
        let place = context.place_pos();
        // SAFETY: see `get_state_for_placement`.
        let result = unsafe {
            hook(
                self.hooks.user_data,
                steel_abi::SteelWorldRef(world_id(context.world)),
                context.hit_pos().x(),
                context.hit_pos().y(),
                context.hit_pos().z(),
                SteelBlockState(u32::from(state.0)),
                place.x(),
                place.y(),
                place.z(),
            )
        };
        result != 0
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        let _block_id = self.block.id();
        let Some(type_id) = crate::adapter::block_entity_type_for_block(self.block) else {
            return BlockEntityCreation::Unimplemented;
        };
        let Some(kind) = REGISTRY
            .block_entity_types
            .iter()
            .find_map(|(id, kind)| (id == type_id).then_some(kind))
        else {
            return BlockEntityCreation::Unimplemented;
        };
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(kind, level, pos, state))
    }
}

/// Item behaviour forwarded to an extension hook table.
pub struct ForeignItemBehavior {
    default: DefaultItemBehavior,
    hooks: SteelItemHooks,
}

// SAFETY: see `ForeignBlockBehavior`.
unsafe impl Send for ForeignItemBehavior {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for ForeignItemBehavior {}

impl ForeignItemBehavior {
    #[must_use]
    pub fn new(hooks: SteelItemHooks) -> Self {
        Self {
            default: DefaultItemBehavior,
            hooks,
        }
    }
}

impl ItemBehavior for ForeignItemBehavior {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let Some(hook) = self.hooks.use_item_on else {
            return self.default.use_on(context);
        };
        let stack = context
            .inv
            .with_item(|stack| HOST.owned.lock().insert_item_stack(stack.clone()));
        // SAFETY: see `ForeignBlockBehavior::get_state_for_placement`.
        let result = unsafe {
            hook(
                self.hooks.user_data,
                steel_abi::SteelWorldRef(world_id(context.world)),
                entity_ref(context.world, context.player),
                hand_value(context.hand),
                context.hit_result.block_pos.x(),
                context.hit_result.block_pos.y(),
                context.hit_result.block_pos.z(),
                state(context.world.get_block_state(context.hit_result.block_pos)),
                stack,
            )
        };
        if result.is_handled() {
            interaction(result)
        } else {
            self.default.use_on(context)
        }
    }

    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let Some(hook) = self.hooks.use_item else {
            return self.default.use_item(context);
        };
        let stack = context
            .inv
            .with_item(|stack| HOST.owned.lock().insert_item_stack(stack.clone()));
        // SAFETY: see `ForeignBlockBehavior::get_state_for_placement`.
        let result = unsafe {
            hook(
                self.hooks.user_data,
                steel_abi::SteelWorldRef(world_id(context.world)),
                entity_ref(context.world, context.player),
                hand_value(context.hand),
                stack,
            )
        };
        if result.is_handled() {
            interaction(result)
        } else {
            self.default.use_item(context)
        }
    }
}

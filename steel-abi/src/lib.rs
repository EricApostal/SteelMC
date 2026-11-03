//! Stable C ABI shared by the Steel server and out-of-process language extensions.
//!
//! The ABI deliberately contains no Rust types. Every value is either a plain
//! `#[repr(C)]` record or an opaque handle that the host resolves. Version 1 of
//! the interface is append-only: add function pointers at the end of
//! [`SteelInterface`] and gate use on [`STEEL_ABI_VERSION`].
//!
//! The generated `steel_api.json`, C header, and language bindings are produced
//! from this crate's source by `steel-plugin-gen`; do not hand-edit those files.
//!
//! # Threading and lifetimes
//!
//! Callbacks run on the thread that emitted them. Block, item, entity, and
//! block-entity callbacks run on the owning world's tick thread, and different
//! worlds tick concurrently, so extension state must be thread-safe. Event
//! listeners run on the emitting thread as well. World mutation through
//! `world_set_block`/`world_destroy_block` is only valid from that world's tick
//! callbacks; other threads must schedule work onto the server job system.
//!
//! Borrowed views (`SteelStrView`, context pointers) are valid only for the
//! duration of the call. Owned handles (`SteelRef` with `ITEM_STACK` or `NBT`)
//! stay valid until `item_stack_free`/`nbt_free`; nested lookups use a
//! generation tag and return an error or empty value after free.

#![allow(
    missing_docs,
    reason = "the generated JSON and C header are the public documentation surface"
)]
#![allow(
    clippy::absolute_paths,
    reason = "ABI definitions spell core:: paths explicitly to show they need no std"
)]

use core::ffi::c_void;

/// Version of the extension ABI implemented by this crate.
pub const STEEL_ABI_VERSION: u32 = 1;

/// Extension initialisation levels. They are called in order and torn down in
/// reverse order.
pub const STEEL_LEVEL_CONTENT: u32 = 0;
/// Behaviours, listeners, commands, and permissions may be registered.
pub const STEEL_LEVEL_BEHAVIOR: u32 = 1;
/// Worlds exist; world-scoped callbacks may be installed.
pub const STEEL_LEVEL_RUNTIME: u32 = 2;

/// Successful status.
pub const STEEL_OK: i32 = 0;
/// A required argument was null or malformed.
pub const STEEL_ERR_INVALID_ARGUMENT: i32 = 1;
/// The requested registration is not allowed in the current phase.
pub const STEEL_ERR_WRONG_PHASE: i32 = 2;
/// The identifier was not found in the registry.
pub const STEEL_ERR_NOT_FOUND: i32 = 3;
/// The host rejected the registration because a name was already used.
pub const STEEL_ERR_ALREADY_EXISTS: i32 = 4;
/// The operation is not implemented by this ABI version.
pub const STEEL_ERR_UNSUPPORTED: i32 = 5;
/// An internal host error occurred. Call `last_error` for details.
pub const STEEL_ERR_INTERNAL: i32 = 6;

/// A borrowed UTF-8 byte slice. It is only valid for the duration of the call
/// that produced it unless the function documentation says otherwise.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct SteelStrView {
    pub ptr: *const u8,
    pub len: usize,
}

impl SteelStrView {
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    /// Creates a view over a string slice.
    #[must_use]
    pub const fn from_str(value: &str) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }

    /// Borrows the view as UTF-8.
    ///
    /// # Safety
    ///
    /// `ptr` must point to `len` readable bytes that remain valid for `'a`.
    #[must_use]
    pub unsafe fn as_str<'a>(&self) -> Option<&'a str> {
        if self.len == 0 {
            return Some("");
        }
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees `ptr`/`len` describe readable memory.
        core::str::from_utf8(unsafe { core::slice::from_raw_parts(self.ptr, self.len) }).ok()
    }

    /// Returns whether the view is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl core::fmt::Debug for SteelStrView {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // SAFETY: debug formatting is best-effort; null views are empty.
        match unsafe { self.as_str() } {
            Some(value) => formatter.debug_tuple("SteelStrView").field(&value).finish(),
            None => formatter.write_str("SteelStrView(<invalid>)"),
        }
    }
}

/// Reference kinds used by [`SteelRef`].
pub const STEEL_REF_NONE: u32 = 0;
pub const STEEL_REF_BLOCK: u32 = 1;
pub const STEEL_REF_ITEM: u32 = 2;
pub const STEEL_REF_FLUID: u32 = 3;
pub const STEEL_REF_ENTITY_TYPE: u32 = 4;
pub const STEEL_REF_BLOCK_ENTITY_TYPE: u32 = 5;
pub const STEEL_REF_ITEM_STACK: u32 = 6;
pub const STEEL_REF_NBT: u32 = 7;
pub const STEEL_REF_TEXT: u32 = 8;

/// A handle to a registry entry or an owned host object.
///
/// Registry references use `generation == 0` and `index == registry id`.
/// Owned objects (`ITEM_STACK`, `NBT`) use the generation to reject stale
/// handles after the object has been freed.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SteelRef {
    pub kind: u32,
    pub index: u32,
    pub generation: u32,
}

impl SteelRef {
    pub const NONE: Self = Self {
        kind: STEEL_REF_NONE,
        index: 0,
        generation: 0,
    };

    #[must_use]
    pub const fn registry(kind: u32, index: u32) -> Self {
        Self {
            kind,
            index,
            generation: 0,
        }
    }

    #[must_use]
    pub const fn owned(kind: u32, index: u32, generation: u32) -> Self {
        Self {
            kind,
            index,
            generation,
        }
    }

    #[must_use]
    pub const fn is_none(self) -> bool {
        self.kind == STEEL_REF_NONE
    }
}

impl Default for SteelRef {
    fn default() -> Self {
        Self::NONE
    }
}

impl core::fmt::Debug for SteelRef {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SteelRef")
            .field("kind", &self.kind)
            .field("index", &self.index)
            .field("generation", &self.generation)
            .finish()
    }
}

/// Identifier of a loaded world in the host's world table.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SteelWorldRef(pub u32);

impl SteelWorldRef {
    pub const INVALID: Self = Self(u32::MAX);
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }
}

impl Default for SteelWorldRef {
    fn default() -> Self {
        Self::INVALID
    }
}

/// A session-local entity reference. `entity` is the vanilla runtime entity id.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SteelEntityRef {
    pub world: SteelWorldRef,
    pub entity: i32,
}

impl SteelEntityRef {
    pub const INVALID: Self = Self {
        world: SteelWorldRef::INVALID,
        entity: -1,
    };
}

/// A block-entity reference identified by its position.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SteelBlockEntityRef {
    pub world: SteelWorldRef,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SteelOptEntityRef {
    pub present: u8,
    pub value: SteelEntityRef,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SteelOptBlockEntityRef {
    pub present: u8,
    pub value: SteelBlockEntityRef,
}

/// A global vanilla block-state id.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SteelBlockState(pub u32);

/// A block-local axis-aligned box.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SteelAabb {
    pub min_x: f32,
    pub min_y: f32,
    pub min_z: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub max_z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SteelVec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Property kinds used by [`SteelPropertyDesc`].
pub const STEEL_PROPERTY_BOOL: u32 = 0;
pub const STEEL_PROPERTY_INT: u32 = 1;
pub const STEEL_PROPERTY_ENUM: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelPropertyDesc {
    pub kind: u32,
    pub name: SteelStrView,
    /// Inclusive minimum for [`STEEL_PROPERTY_INT`]; ignored otherwise.
    pub min: i32,
    /// Inclusive maximum for [`STEEL_PROPERTY_INT`]; ignored otherwise.
    pub max: i32,
    /// Ordered values for [`STEEL_PROPERTY_ENUM`].
    pub values: *const SteelStrView,
    pub value_count: usize,
}

/// Per-state shape override. `state_offset` is block-local.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelShapeOverride {
    pub state_offset: u32,
    pub boxes: *const SteelAabb,
    pub box_count: usize,
}

/// Shape data for one shape channel. `boxes` is the default shape; overrides
/// replace individual state offsets.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SteelShapeTable {
    pub boxes: *const SteelAabb,
    pub box_count: usize,
    pub overrides: *const SteelShapeOverride,
    pub override_count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelLight {
    pub emission: u8,
    pub dampening: u8,
    pub use_shape_for_occlusion: u8,
}

impl Default for SteelLight {
    fn default() -> Self {
        Self {
            emission: 0,
            dampening: 15,
            use_shape_for_occlusion: 0,
        }
    }
}

/// Block flags accepted by [`SteelBlockDesc::flags`].
pub const STEEL_BLOCK_HAS_COLLISION: u32 = 1 << 0;
pub const STEEL_BLOCK_CAN_OCCLUDE: u32 = 1 << 1;
pub const STEEL_BLOCK_IS_AIR: u32 = 1 << 2;
pub const STEEL_BLOCK_REPLACEABLE: u32 = 1 << 3;
pub const STEEL_BLOCK_RANDOMLY_TICKING: u32 = 1 << 4;
pub const STEEL_BLOCK_LIQUID: u32 = 1 << 5;
pub const STEEL_BLOCK_REQUIRES_CORRECT_TOOL: u32 = 1 << 6;
pub const STEEL_BLOCK_FORCE_SOLID_OFF: u32 = 1 << 7;
pub const STEEL_BLOCK_FORCE_SOLID_ON: u32 = 1 << 8;
pub const STEEL_BLOCK_IGNITED_BY_LAVA: u32 = 1 << 9;

/// Push reactions accepted by [`SteelBlockDesc::push_reaction`].
pub const STEEL_PUSH_NORMAL: u32 = 0;
pub const STEEL_PUSH_DESTROY: u32 = 1;
pub const STEEL_PUSH_BLOCK: u32 = 2;
pub const STEEL_PUSH_IGNORE: u32 = 3;
pub const STEEL_PUSH_POPPED: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelBlockDesc {
    pub struct_size: u32,
    pub key: SteelStrView,
    pub flags: u32,
    pub push_reaction: u32,
    pub destroy_time: f32,
    pub explosion_resistance: f32,
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    pub bounce_restitution: f32,
    pub fall_distance_reduction: f32,
    pub max_horizontal_offset: f32,
    pub max_vertical_offset: f32,
    pub properties: *const SteelPropertyDesc,
    pub property_count: usize,
    pub collision: SteelShapeTable,
    pub support: SteelShapeTable,
    pub outline: SteelShapeTable,
    pub occlusion: SteelShapeTable,
    pub interaction: SteelShapeTable,
    pub visual: SteelShapeTable,
    pub light: SteelLight,
    pub suffocating: u8,
    pub redstone_conductor: u8,
    pub randomly_ticking: u8,
    pub default_state_offset: u32,
}

impl Default for SteelBlockDesc {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            key: SteelStrView::EMPTY,
            flags: STEEL_BLOCK_HAS_COLLISION | STEEL_BLOCK_CAN_OCCLUDE,
            push_reaction: STEEL_PUSH_NORMAL,
            destroy_time: 0.0,
            explosion_resistance: 0.0,
            friction: 0.6,
            speed_factor: 1.0,
            jump_factor: 1.0,
            bounce_restitution: 0.0,
            fall_distance_reduction: 0.0,
            max_horizontal_offset: 0.25,
            max_vertical_offset: 0.2,
            properties: core::ptr::null(),
            property_count: 0,
            collision: SteelShapeTable::default(),
            support: SteelShapeTable::default(),
            outline: SteelShapeTable::default(),
            occlusion: SteelShapeTable::default(),
            interaction: SteelShapeTable::default(),
            visual: SteelShapeTable::default(),
            light: SteelLight::default(),
            suffocating: 1,
            redstone_conductor: 1,
            randomly_ticking: 0,
            default_state_offset: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelItemDesc {
    pub struct_size: u32,
    pub key: SteelStrView,
    /// Optional block item target. [`SteelRef::NONE`] creates a plain item.
    pub block: SteelRef,
    /// Stack size stored in the item's `max_stack_size` component.
    pub max_stack_size: i32,
}

impl Default for SteelItemDesc {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            key: SteelStrView::EMPTY,
            block: SteelRef::NONE,
            max_stack_size: 64,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelBlockEntityDesc {
    pub struct_size: u32,
    pub key: SteelStrView,
    pub valid_blocks: *const SteelRef,
    pub valid_block_count: usize,
}

impl Default for SteelBlockEntityDesc {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            key: SteelStrView::EMPTY,
            valid_blocks: core::ptr::null(),
            valid_block_count: 0,
        }
    }
}

/// Entity flags accepted by [`SteelEntityDesc::flags`].
pub const STEEL_ENTITY_FIRE_IMMUNE: u32 = 1 << 0;
pub const STEEL_ENTITY_SUMMONABLE: u32 = 1 << 1;
pub const STEEL_ENTITY_CAN_SERIALIZE: u32 = 1 << 2;
pub const STEEL_ENTITY_ALLOWED_IN_PEACEFUL: u32 = 1 << 3;
pub const STEEL_ENTITY_CAN_SPAWN_FAR_FROM_PLAYER: u32 = 1 << 4;
pub const STEEL_ENTITY_TRACK_DELTAS: u32 = 1 << 5;

/// Mob categories accepted by [`SteelEntityDesc::mob_category`].
pub const STEEL_MOB_CATEGORY_MONSTER: u32 = 0;
pub const STEEL_MOB_CATEGORY_CREATURE: u32 = 1;
pub const STEEL_MOB_CATEGORY_AMBIENT: u32 = 2;
pub const STEEL_MOB_CATEGORY_AXOLOTLS: u32 = 3;
pub const STEEL_MOB_CATEGORY_UNDERGROUND_WATER_CREATURE: u32 = 4;
pub const STEEL_MOB_CATEGORY_WATER_CREATURE: u32 = 5;
pub const STEEL_MOB_CATEGORY_WATER_AMBIENT: u32 = 6;
pub const STEEL_MOB_CATEGORY_MISC: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelEntityDesc {
    pub struct_size: u32,
    pub key: SteelStrView,
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub client_tracking_range: i32,
    pub update_interval: i32,
    pub mob_category: u32,
    pub flags: u32,
}

impl Default for SteelEntityDesc {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            key: SteelStrView::EMPTY,
            width: 0.6,
            height: 1.8,
            eye_height: 1.62,
            client_tracking_range: 10,
            update_interval: 3,
            mob_category: STEEL_MOB_CATEGORY_MISC,
            flags: STEEL_ENTITY_SUMMONABLE | STEEL_ENTITY_CAN_SERIALIZE,
        }
    }
}

/// Placement information exposed to block behaviours.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelPlaceContext {
    pub world: SteelWorldRef,
    pub hit_x: i32,
    pub hit_y: i32,
    pub hit_z: i32,
    pub place_x: i32,
    pub place_y: i32,
    pub place_z: i32,
    pub clicked_face: u32,
    pub inside: u8,
    pub replaces_clicked_block: u8,
    pub player: SteelEntityRef,
    pub player_present: u8,
    pub item_stack: SteelRef,
}

/// Hand values.
pub const STEEL_HAND_MAIN: u32 = 0;
pub const STEEL_HAND_OFF: u32 = 1;

/// Interaction result kinds. `STEEL_INTERACTION_NOT_HANDLED` tells the host to
/// fall through to the next handler.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SteelInteractionResult {
    pub kind: u32,
}

pub const STEEL_INTERACTION_NOT_HANDLED: u32 = 0xFFFF_FFFF;
pub const STEEL_INTERACTION_SUCCESS: u32 = 0;
pub const STEEL_INTERACTION_SUCCESS_WITHOUT_ITEM: u32 = 1;
pub const STEEL_INTERACTION_SUCCESS_SERVER: u32 = 2;
pub const STEEL_INTERACTION_CONSUME: u32 = 3;
pub const STEEL_INTERACTION_FAIL: u32 = 4;
pub const STEEL_INTERACTION_PASS: u32 = 5;
pub const STEEL_INTERACTION_TRY_EMPTY_HAND: u32 = 6;

impl SteelInteractionResult {
    pub const NOT_HANDLED: Self = Self {
        kind: STEEL_INTERACTION_NOT_HANDLED,
    };

    #[must_use]
    pub const fn is_handled(self) -> bool {
        self.kind != STEEL_INTERACTION_NOT_HANDLED
    }
}

pub type SteelBlockGetStateForPlacementFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    context: *const SteelPlaceContext,
    out: *mut SteelBlockState,
) -> u8;
pub type SteelBlockOnPlaceFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
    old_state: SteelBlockState,
    moved_by_piston: u8,
);
pub type SteelBlockUseItemOnFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
    player: SteelEntityRef,
    hand: u32,
    clicked_face: u32,
) -> SteelInteractionResult;
pub type SteelBlockPlayerDestroyFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    player: SteelEntityRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
    tool: SteelRef,
);
pub type SteelBlockRandomTickFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
);
pub type SteelBlockScheduledTickFn = SteelBlockRandomTickFn;
pub type SteelBlockNeighborChangedFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
    source_block: SteelRef,
    moved_by_piston: u8,
);
pub type SteelBlockCanBeReplacedFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
    place_x: i32,
    place_y: i32,
    place_z: i32,
) -> u8;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SteelBlockHooks {
    pub struct_size: u32,
    pub user_data: *mut c_void,
    pub get_state_for_placement: Option<SteelBlockGetStateForPlacementFn>,
    pub on_place: Option<SteelBlockOnPlaceFn>,
    pub use_item_on: Option<SteelBlockUseItemOnFn>,
    pub player_destroy: Option<SteelBlockPlayerDestroyFn>,
    pub random_tick: Option<SteelBlockRandomTickFn>,
    pub scheduled_tick: Option<SteelBlockScheduledTickFn>,
    pub neighbor_changed: Option<SteelBlockNeighborChangedFn>,
    pub can_be_replaced: Option<SteelBlockCanBeReplacedFn>,
}

// SAFETY: hook tables are immutable after registration and user data is
// required by the extension contract to be thread-safe.
unsafe impl Send for SteelBlockHooks {}

// SAFETY: see the `Send` implementation above; the table is immutable.
unsafe impl Sync for SteelBlockHooks {}

impl Default for SteelBlockHooks {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            user_data: core::ptr::null_mut(),
            get_state_for_placement: None,
            on_place: None,
            use_item_on: None,
            player_destroy: None,
            random_tick: None,
            scheduled_tick: None,
            neighbor_changed: None,
            can_be_replaced: None,
        }
    }
}

pub type SteelItemUseFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    entity: SteelEntityRef,
    hand: u32,
    stack: SteelRef,
) -> SteelInteractionResult;
pub type SteelItemUseOnFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    entity: SteelEntityRef,
    hand: u32,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
    stack: SteelRef,
) -> SteelInteractionResult;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SteelItemHooks {
    pub struct_size: u32,
    pub user_data: *mut c_void,
    pub use_item: Option<SteelItemUseFn>,
    pub use_item_on: Option<SteelItemUseOnFn>,
}

// SAFETY: see `SteelBlockHooks`.
unsafe impl Send for SteelItemHooks {}

// SAFETY: see the `Send` implementation above; the table is immutable.
unsafe impl Sync for SteelItemHooks {}

impl Default for SteelItemHooks {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            user_data: core::ptr::null_mut(),
            use_item: None,
            use_item_on: None,
        }
    }
}

pub type SteelEntityTickFn = unsafe extern "C" fn(user_data: *mut c_void, entity: SteelEntityRef);
pub type SteelEntitySaveFn =
    unsafe extern "C" fn(user_data: *mut c_void, entity: SteelEntityRef, nbt: SteelRef);
pub type SteelEntityLoadFn =
    unsafe extern "C" fn(user_data: *mut c_void, entity: SteelEntityRef, nbt: SteelRef);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SteelEntityHooks {
    pub struct_size: u32,
    pub user_data: *mut c_void,
    pub tick: Option<SteelEntityTickFn>,
    pub save: Option<SteelEntitySaveFn>,
    pub load: Option<SteelEntityLoadFn>,
}

// SAFETY: see `SteelBlockHooks`.
unsafe impl Send for SteelEntityHooks {}

// SAFETY: see the `Send` implementation above; the table is immutable.
unsafe impl Sync for SteelEntityHooks {}

impl Default for SteelEntityHooks {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            user_data: core::ptr::null_mut(),
            tick: None,
            save: None,
            load: None,
        }
    }
}

pub type SteelBlockEntityTickFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
);
pub type SteelBlockEntitySaveFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    nbt: SteelRef,
);
pub type SteelBlockEntityLoadFn = SteelBlockEntitySaveFn;
pub type SteelBlockEntityStateChangedFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SteelBlockEntityHooks {
    pub struct_size: u32,
    pub user_data: *mut c_void,
    pub tick: Option<SteelBlockEntityTickFn>,
    pub save: Option<SteelBlockEntitySaveFn>,
    pub load: Option<SteelBlockEntityLoadFn>,
    pub state_changed: Option<SteelBlockEntityStateChangedFn>,
}

// SAFETY: see `SteelBlockHooks`.
unsafe impl Send for SteelBlockEntityHooks {}

// SAFETY: see the `Send` implementation above; the table is immutable.
unsafe impl Sync for SteelBlockEntityHooks {}

impl Default for SteelBlockEntityHooks {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            user_data: core::ptr::null_mut(),
            tick: None,
            save: None,
            load: None,
            state_changed: None,
        }
    }
}

/// Event kinds delivered to extension listeners.
pub const STEEL_EVENT_BLOCK_BREAK: u32 = 1;
pub const STEEL_EVENT_BLOCK_PLACE: u32 = 2;
pub const STEEL_EVENT_PLAYER_JOIN: u32 = 3;
pub const STEEL_EVENT_PLAYER_LEAVE: u32 = 4;
pub const STEEL_EVENT_CHAT: u32 = 5;

/// `flags` bit set when a previous listener cancelled the event.
pub const STEEL_EVENT_CANCELLED: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SteelEvent {
    pub kind: u32,
    pub flags: u32,
    pub world: SteelWorldRef,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub block_state: SteelBlockState,
    pub entity: SteelEntityRef,
    /// Chat text. Only valid for the duration of the callback.
    pub text: SteelStrView,
}

/// Listener return value: `0` continues, `1` cancels the event.
pub type SteelEventListenerFn =
    unsafe extern "C" fn(event: *mut SteelEvent, user_data: *mut c_void) -> u32;

pub type SteelApiVersionFn = unsafe extern "C" fn() -> u32;
pub type SteelLogFn = unsafe extern "C" fn(level: u32, message: SteelStrView);
pub type SteelLastErrorFn = unsafe extern "C" fn() -> SteelStrView;
pub type SteelBlockByKeyFn = unsafe extern "C" fn(key: SteelStrView) -> SteelRef;
pub type SteelItemByKeyFn = SteelBlockByKeyFn;
pub type SteelBlockDefaultStateFn = unsafe extern "C" fn(block: SteelRef) -> SteelBlockState;
pub type SteelStateBlockFn = unsafe extern "C" fn(state: SteelBlockState) -> SteelRef;
pub type SteelStateWithBoolFn =
    unsafe extern "C" fn(state: SteelBlockState, name: SteelStrView, value: u8) -> SteelBlockState;
pub type SteelStateWithIntFn =
    unsafe extern "C" fn(state: SteelBlockState, name: SteelStrView, value: i32) -> SteelBlockState;
pub type SteelStateWithEnumFn = unsafe extern "C" fn(
    state: SteelBlockState,
    name: SteelStrView,
    value: SteelStrView,
) -> SteelBlockState;
pub type SteelStateGetBoolFn =
    unsafe extern "C" fn(state: SteelBlockState, name: SteelStrView, out: *mut u8) -> i32;
pub type SteelStateGetIntFn =
    unsafe extern "C" fn(state: SteelBlockState, name: SteelStrView, out: *mut i32) -> i32;
pub type SteelStateGetEnumFn =
    unsafe extern "C" fn(state: SteelBlockState, name: SteelStrView, out: *mut SteelStrView) -> i32;

pub type SteelWorldGetBlockFn =
    unsafe extern "C" fn(world: SteelWorldRef, x: i32, y: i32, z: i32) -> SteelBlockState;
pub type SteelWorldSetBlockFn = unsafe extern "C" fn(
    world: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    state: SteelBlockState,
    flags: u32,
) -> u8;
pub type SteelWorldDestroyBlockFn =
    unsafe extern "C" fn(world: SteelWorldRef, x: i32, y: i32, z: i32, drop_items: u8) -> u8;
pub type SteelWorldGetEntityFn =
    unsafe extern "C" fn(world: SteelWorldRef, entity_id: i32) -> SteelOptEntityRef;
pub type SteelWorldGetBlockEntityFn =
    unsafe extern "C" fn(world: SteelWorldRef, x: i32, y: i32, z: i32) -> SteelOptBlockEntityRef;
pub type SteelWorldSpawnEntityFn = unsafe extern "C" fn(
    world: SteelWorldRef,
    entity_type: SteelRef,
    position: SteelVec3,
    yaw: f32,
    pitch: f32,
    out: *mut SteelOptEntityRef,
) -> i32;
pub type SteelWorldPlaySoundFn = unsafe extern "C" fn(
    world: SteelWorldRef,
    sound_key: SteelStrView,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
);
pub type SteelWorldBroadcastPayloadFn = unsafe extern "C" fn(
    world: SteelWorldRef,
    channel: SteelStrView,
    data: *const u8,
    len: usize,
) -> i32;
pub type SteelPlayerSendPayloadFn = unsafe extern "C" fn(
    player: SteelEntityRef,
    channel: SteelStrView,
    data: *const u8,
    len: usize,
) -> i32;
pub type SteelEntityGetPositionFn =
    unsafe extern "C" fn(entity: SteelEntityRef, out: *mut SteelVec3) -> i32;
pub type SteelEntityTeleportFn =
    unsafe extern "C" fn(entity: SteelEntityRef, position: SteelVec3, yaw: f32, pitch: f32) -> i32;

pub type SteelItemStackNewFn = unsafe extern "C" fn(item: SteelRef, count: i32) -> SteelRef;
pub type SteelItemStackCloneFn = unsafe extern "C" fn(stack: SteelRef) -> SteelRef;
pub type SteelItemStackFreeFn = unsafe extern "C" fn(stack: SteelRef);
pub type SteelItemStackCountFn = unsafe extern "C" fn(stack: SteelRef) -> i32;
pub type SteelItemStackSetCountFn = unsafe extern "C" fn(stack: SteelRef, count: i32) -> i32;
pub type SteelItemStackItemFn = unsafe extern "C" fn(stack: SteelRef) -> SteelRef;

pub type SteelNbtNewFn = unsafe extern "C" fn() -> SteelRef;
pub type SteelNbtCloneFn = unsafe extern "C" fn(nbt: SteelRef) -> SteelRef;
pub type SteelNbtFreeFn = unsafe extern "C" fn(nbt: SteelRef);
pub type SteelNbtSetIntFn =
    unsafe extern "C" fn(nbt: SteelRef, key: SteelStrView, value: i64) -> i32;
pub type SteelNbtGetIntFn =
    unsafe extern "C" fn(nbt: SteelRef, key: SteelStrView, default_value: i64) -> i64;
pub type SteelNbtSetStringFn =
    unsafe extern "C" fn(nbt: SteelRef, key: SteelStrView, value: SteelStrView) -> i32;
pub type SteelNbtGetStringFn =
    unsafe extern "C" fn(nbt: SteelRef, key: SteelStrView) -> SteelStrView;
pub type SteelNbtContainsFn = unsafe extern "C" fn(nbt: SteelRef, key: SteelStrView) -> u8;
pub type SteelNbtRemoveFn = unsafe extern "C" fn(nbt: SteelRef, key: SteelStrView) -> i32;

pub type SteelRegisterBlockFn =
    unsafe extern "C" fn(desc: *const SteelBlockDesc, hooks: *const SteelBlockHooks) -> i32;
pub type SteelRegisterItemFn =
    unsafe extern "C" fn(desc: *const SteelItemDesc, hooks: *const SteelItemHooks) -> i32;
pub type SteelRegisterBlockEntityTypeFn = unsafe extern "C" fn(
    desc: *const SteelBlockEntityDesc,
    hooks: *const SteelBlockEntityHooks,
) -> i32;
pub type SteelRegisterEntityTypeFn =
    unsafe extern "C" fn(desc: *const SteelEntityDesc, hooks: *const SteelEntityHooks) -> i32;
pub type SteelRegisterEventListenerFn = unsafe extern "C" fn(
    kind: u32,
    priority: i32,
    callback: Option<SteelEventListenerFn>,
    user_data: *mut c_void,
) -> i32;

/// Signature of a command handler registered through
/// [`SteelInterface::register_command`].
///
/// `player` is meaningful only when `has_player` is nonzero. `command` is the
/// registered command name without its leading slash and `args` is the raw
/// trailing input, empty when the command was invoked without arguments.
/// A nonzero return value counts as a successful execution.
pub type SteelCommandHandlerFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    player: SteelEntityRef,
    has_player: u8,
    command: SteelStrView,
    args: SteelStrView,
) -> i32;

/// Returns the path of the extension library whose entry point or `initialize`
/// call is currently running. The view is valid for the call and empty when no
/// extension is being initialized on this thread.
pub type SteelExtensionPathFn = unsafe extern "C" fn() -> SteelStrView;
/// Registers a command with the server. `name` is the unqualified literal, the
/// `permission` is optional (empty means default access), and the handler
/// receives the invoking player when `has_player` is nonzero. Commands are
/// queued and become available on the next server start.
pub type SteelRegisterCommandFn = unsafe extern "C" fn(
    name: SteelStrView,
    description: SteelStrView,
    permission: SteelStrView,
    handler: Option<SteelCommandHandlerFn>,
    user_data: *mut c_void,
) -> i32;
/// Writes the player name; the view is valid until the next host call on the
/// calling thread. Returns [`STEEL_ERR_INVALID_ARGUMENT`] for non-players.
pub type SteelEntityGetNameFn =
    unsafe extern "C" fn(entity: SteelEntityRef, out: *mut SteelStrView) -> i32;
/// Writes the player UUID in hyphenated form, with the same lifetime and
/// player-only rules as [`SteelEntityGetNameFn`].
pub type SteelEntityGetUuidFn = SteelEntityGetNameFn;
/// Sends a plain chat message to a player.
pub type SteelPlayerSendMessageFn =
    unsafe extern "C" fn(entity: SteelEntityRef, message: SteelStrView) -> i32;
/// Writes the current health of a living entity.
pub type SteelEntityGetHealthFn =
    unsafe extern "C" fn(entity: SteelEntityRef, out: *mut f32) -> i32;
/// Sets the health of a living entity, clamped by the entity's maximum health.
pub type SteelEntitySetHealthFn = unsafe extern "C" fn(entity: SteelEntityRef, health: f32) -> i32;
/// Writes the world spawn position at the centre of its spawn block.
pub type SteelWorldGetSpawnFn =
    unsafe extern "C" fn(world: SteelWorldRef, out: *mut SteelVec3) -> i32;
/// Returns the number of players in a world.
pub type SteelWorldPlayerCountFn = unsafe extern "C" fn(world: SteelWorldRef) -> u32;
/// Writes the player at `index` in world iteration order.
pub type SteelWorldPlayerAtFn =
    unsafe extern "C" fn(world: SteelWorldRef, index: u32, out: *mut SteelOptEntityRef) -> i32;
/// Writes the first player in `world` whose profile name matches `name`.
pub type SteelWorldFindPlayerFn = unsafe extern "C" fn(
    world: SteelWorldRef,
    name: SteelStrView,
    out: *mut SteelOptEntityRef,
) -> i32;

/// Reference kinds used by class instances.
pub const STEEL_REF_WORLD: u32 = 9;
pub const STEEL_REF_BLOCK_STATE: u32 = 10;
pub const STEEL_REF_EXTENSION: u32 = 11;
pub const STEEL_REF_ENTITY: u32 = 12;

/// Typed MethodBind thunk. `instance` is the receiver; `args` points at one
/// pointer per argument, and `ret` points at the return slot.
pub type SteelMethodThunk = unsafe extern "C" fn(
    user_data: *mut c_void,
    instance: SteelRef,
    args: *const *const c_void,
    ret: *mut c_void,
);

/// Opaque MethodBind handle returned by `classdb_get_method_bind`.
#[repr(C)]
pub struct SteelMethodBind {
    pub _unused: u8,
}

/// Native or extension method metadata registered with the host ClassDB.
#[repr(C)]
pub struct SteelMethodBindDesc {
    pub class_name: SteelStrView,
    pub method_name: SteelStrView,
    pub hash: u32,
    pub thunk: Option<SteelMethodThunk>,
    pub user_data: *mut c_void,
}

// SAFETY: method bind descriptors are immutable registration data. Extension
// user data is required by the extension contract to remain valid and
// thread-safe for the process lifetime.
unsafe impl Send for SteelMethodBindDesc {}
unsafe impl Sync for SteelMethodBindDesc {}

pub type SteelClassdbGetMethodBindFn = unsafe extern "C" fn(
    class_name: SteelStrView,
    method_name: SteelStrView,
    hash: u32,
) -> *const SteelMethodBind;

pub type SteelMethodBindPtrcallFn = unsafe extern "C" fn(
    bind: *const SteelMethodBind,
    instance: SteelRef,
    args: *const *const c_void,
    ret: *mut c_void,
) -> i32;

/// Constructs an extension-class instance and writes its handle to `ret`.
pub type SteelClassConstructFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    args: *const *const c_void,
    ret: *mut SteelRef,
) -> i32;

/// Method descriptor supplied by an extension class.
#[repr(C)]
pub struct SteelExtensionMethodDesc {
    pub method_name: SteelStrView,
    pub hash: u32,
    pub thunk: Option<SteelMethodThunk>,
    pub user_data: *mut c_void,
}

// SAFETY: extension metadata is immutable after registration and user data is
// required to be thread-safe by the extension contract.
unsafe impl Send for SteelExtensionMethodDesc {}
unsafe impl Sync for SteelExtensionMethodDesc {}

/// Class descriptor supplied by an extension.
#[repr(C)]
pub struct SteelExtensionClassDesc {
    pub struct_size: u32,
    pub class_name: SteelStrView,
    pub inherits: SteelStrView,
    pub construct: Option<SteelClassConstructFn>,
    pub construct_user_data: *mut c_void,
    pub methods: *const SteelExtensionMethodDesc,
    pub method_count: usize,
}

// SAFETY: see `SteelExtensionMethodDesc`.
unsafe impl Send for SteelExtensionClassDesc {}
unsafe impl Sync for SteelExtensionClassDesc {}

pub type SteelRegisterExtensionClassFn =
    unsafe extern "C" fn(desc: *const SteelExtensionClassDesc) -> i32;

pub type SteelClassdbConstructFn = unsafe extern "C" fn(
    class_name: SteelStrView,
    args: *const *const c_void,
    ret: *mut SteelRef,
) -> i32;

/// Host function table passed to an extension's entry point.
///
/// Callers must check [`Self::api_version`] and may only read fields covered by
/// [`Self::struct_size`]. New functions are appended at the end.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SteelInterface {
    pub struct_size: u32,
    pub api_version: u32,
    pub log: Option<SteelLogFn>,
    pub last_error: Option<SteelLastErrorFn>,
    pub block_by_key: Option<SteelBlockByKeyFn>,
    pub item_by_key: Option<SteelItemByKeyFn>,
    pub block_default_state: Option<SteelBlockDefaultStateFn>,
    pub state_block: Option<SteelStateBlockFn>,
    pub state_with_bool: Option<SteelStateWithBoolFn>,
    pub state_with_int: Option<SteelStateWithIntFn>,
    pub state_with_enum: Option<SteelStateWithEnumFn>,
    pub state_get_bool: Option<SteelStateGetBoolFn>,
    pub state_get_int: Option<SteelStateGetIntFn>,
    pub state_get_enum: Option<SteelStateGetEnumFn>,
    pub world_get_block: Option<SteelWorldGetBlockFn>,
    pub world_set_block: Option<SteelWorldSetBlockFn>,
    pub world_destroy_block: Option<SteelWorldDestroyBlockFn>,
    pub world_get_entity: Option<SteelWorldGetEntityFn>,
    pub world_get_block_entity: Option<SteelWorldGetBlockEntityFn>,
    pub world_spawn_entity: Option<SteelWorldSpawnEntityFn>,
    pub world_play_sound: Option<SteelWorldPlaySoundFn>,
    pub world_broadcast_payload: Option<SteelWorldBroadcastPayloadFn>,
    pub player_send_payload: Option<SteelPlayerSendPayloadFn>,
    pub entity_get_position: Option<SteelEntityGetPositionFn>,
    pub entity_teleport: Option<SteelEntityTeleportFn>,
    pub item_stack_new: Option<SteelItemStackNewFn>,
    pub item_stack_clone: Option<SteelItemStackCloneFn>,
    pub item_stack_free: Option<SteelItemStackFreeFn>,
    pub item_stack_count: Option<SteelItemStackCountFn>,
    pub item_stack_set_count: Option<SteelItemStackSetCountFn>,
    pub item_stack_item: Option<SteelItemStackItemFn>,
    pub nbt_new: Option<SteelNbtNewFn>,
    pub nbt_clone: Option<SteelNbtCloneFn>,
    pub nbt_free: Option<SteelNbtFreeFn>,
    pub nbt_set_int: Option<SteelNbtSetIntFn>,
    pub nbt_get_int: Option<SteelNbtGetIntFn>,
    pub nbt_set_string: Option<SteelNbtSetStringFn>,
    pub nbt_get_string: Option<SteelNbtGetStringFn>,
    pub nbt_contains: Option<SteelNbtContainsFn>,
    pub nbt_remove: Option<SteelNbtRemoveFn>,
    pub register_block: Option<SteelRegisterBlockFn>,
    pub register_item: Option<SteelRegisterItemFn>,
    pub register_block_entity_type: Option<SteelRegisterBlockEntityTypeFn>,
    pub register_entity_type: Option<SteelRegisterEntityTypeFn>,
    pub register_event_listener: Option<SteelRegisterEventListenerFn>,
    pub extension_path: Option<SteelExtensionPathFn>,
    pub register_command: Option<SteelRegisterCommandFn>,
    pub entity_get_name: Option<SteelEntityGetNameFn>,
    pub entity_get_uuid: Option<SteelEntityGetUuidFn>,
    pub player_send_message: Option<SteelPlayerSendMessageFn>,
    pub entity_get_health: Option<SteelEntityGetHealthFn>,
    pub entity_set_health: Option<SteelEntitySetHealthFn>,
    pub world_get_spawn: Option<SteelWorldGetSpawnFn>,
    pub world_player_count: Option<SteelWorldPlayerCountFn>,
    pub world_player_at: Option<SteelWorldPlayerAtFn>,
    pub world_find_player: Option<SteelWorldFindPlayerFn>,
    pub classdb_get_method_bind: Option<SteelClassdbGetMethodBindFn>,
    pub method_bind_ptrcall: Option<SteelMethodBindPtrcallFn>,
    pub register_extension_class: Option<SteelRegisterExtensionClassFn>,
    pub classdb_construct: Option<SteelClassdbConstructFn>,
}

impl Default for SteelInterface {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            api_version: STEEL_ABI_VERSION,
            log: None,
            last_error: None,
            block_by_key: None,
            item_by_key: None,
            block_default_state: None,
            state_block: None,
            state_with_bool: None,
            state_with_int: None,
            state_with_enum: None,
            state_get_bool: None,
            state_get_int: None,
            state_get_enum: None,
            world_get_block: None,
            world_set_block: None,
            world_destroy_block: None,
            world_get_entity: None,
            world_get_block_entity: None,
            world_spawn_entity: None,
            world_play_sound: None,
            world_broadcast_payload: None,
            player_send_payload: None,
            entity_get_position: None,
            entity_teleport: None,
            item_stack_new: None,
            item_stack_clone: None,
            item_stack_free: None,
            item_stack_count: None,
            item_stack_set_count: None,
            item_stack_item: None,
            nbt_new: None,
            nbt_clone: None,
            nbt_free: None,
            nbt_set_int: None,
            nbt_get_int: None,
            nbt_set_string: None,
            nbt_get_string: None,
            nbt_contains: None,
            nbt_remove: None,
            register_block: None,
            register_item: None,
            register_block_entity_type: None,
            register_entity_type: None,
            register_event_listener: None,
            extension_path: None,
            register_command: None,
            entity_get_name: None,
            entity_get_uuid: None,
            player_send_message: None,
            entity_get_health: None,
            entity_set_health: None,
            world_get_spawn: None,
            world_player_count: None,
            world_player_at: None,
            world_find_player: None,
            classdb_get_method_bind: None,
            method_bind_ptrcall: None,
            register_extension_class: None,
            classdb_construct: None,
        }
    }
}

// SAFETY: the interface is published once as an immutable table of function
// pointers; every entry is `Send + Sync` and the table is never mutated.
unsafe impl Sync for SteelInterface {}
// SAFETY: see the `Sync` implementation above; all fields are plain function
// pointers or integers.
unsafe impl Send for SteelInterface {}

pub type SteelInitializeFn = unsafe extern "C" fn(level: u32, user_data: *mut c_void) -> i32;
pub type SteelDeinitializeFn = unsafe extern "C" fn(level: u32, user_data: *mut c_void);

/// Extension description returned by [`SteelExtensionEntryFn`].
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SteelExtension {
    pub struct_size: u32,
    pub api_version: u32,
    pub id: SteelStrView,
    pub version: SteelStrView,
    pub api_min: u32,
    pub api_max: u32,
    pub initialize: Option<SteelInitializeFn>,
    pub deinitialize: Option<SteelDeinitializeFn>,
    pub user_data: *mut c_void,
}

// SAFETY: extension manifests are immutable static data.
unsafe impl Send for SteelExtension {}

// SAFETY: see the `Send` implementation above; the manifest is immutable.
unsafe impl Sync for SteelExtension {}

/// Symbol every extension must export. The returned pointer must remain valid
/// for the lifetime of the loaded library.
pub type SteelExtensionEntryFn =
    unsafe extern "C" fn(interface: *const SteelInterface) -> *const SteelExtension;

/// Returns the ABI version. Exported from the ABI crate so language bindings can
/// run a conformance check against the compiled library.
#[unsafe(no_mangle)]
pub extern "C" fn steel_abi_version() -> u32 {
    STEEL_ABI_VERSION
}

/// Conformance helper used by binding tests.
#[unsafe(no_mangle)]
pub extern "C" fn steel_abi_test_add(left: i32, right: i32) -> i32 {
    left.wrapping_add(right)
}

/// Conformance helper that echoes the length of a borrowed string view.
#[unsafe(no_mangle)]
pub extern "C" fn steel_abi_test_len(view: SteelStrView) -> usize {
    view.len
}

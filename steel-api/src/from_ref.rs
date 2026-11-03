//! Conversion from the generic instance reference to typed receivers.

use steel_abi::{
    STEEL_REF_BLOCK, STEEL_REF_BLOCK_STATE, STEEL_REF_ENTITY, STEEL_REF_ITEM, STEEL_REF_WORLD,
    SteelEntityRef, SteelRef, SteelWorldRef,
};

#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct BlockId(pub u32);
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct BlockStateId(pub u32);
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct ItemId(pub u32);

pub trait FromRef: Sized {
    fn from_ref(reference: SteelRef) -> Option<Self>;
}

impl FromRef for SteelRef {
    fn from_ref(reference: SteelRef) -> Option<Self> {
        Some(reference)
    }
}

impl FromRef for () {
    fn from_ref(_reference: SteelRef) -> Option<Self> {
        Some(())
    }
}

impl FromRef for SteelWorldRef {
    fn from_ref(reference: SteelRef) -> Option<Self> {
        (reference.kind == STEEL_REF_WORLD).then_some(SteelWorldRef(reference.index))
    }
}

impl FromRef for SteelEntityRef {
    fn from_ref(reference: SteelRef) -> Option<Self> {
        (reference.kind == STEEL_REF_ENTITY).then_some(SteelEntityRef {
            world: SteelWorldRef(reference.index),
            entity: i32::try_from(reference.generation).ok()?,
        })
    }
}

impl FromRef for BlockId {
    fn from_ref(reference: SteelRef) -> Option<Self> {
        (reference.kind == STEEL_REF_BLOCK).then_some(Self(reference.index))
    }
}

impl FromRef for BlockStateId {
    fn from_ref(reference: SteelRef) -> Option<Self> {
        (reference.kind == STEEL_REF_BLOCK_STATE).then_some(Self(reference.index))
    }
}

impl FromRef for ItemId {
    fn from_ref(reference: SteelRef) -> Option<Self> {
        (reference.kind == STEEL_REF_ITEM).then_some(Self(reference.index))
    }
}

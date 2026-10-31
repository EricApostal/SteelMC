use steel_registry::blocks::BlockRef;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks::{
    BARRIER, CARVED_PUMPKIN, JACK_O_LANTERN, MANGROVE_LEAVES, MELON, PUMPKIN,
};
use steel_utils::Direction;

pub fn is_excluded_for_connection(block: BlockRef) -> bool {
    block.has_tag(&BlockTag::LEAVES)
        || block == &BARRIER
        || block == &CARVED_PUMPKIN
        || block == &JACK_O_LANTERN
        || block == &MELON
        || block == &PUMPKIN
        || block.has_tag(&BlockTag::SHULKER_BOXES)
        || block == &MANGROVE_LEAVES
}

pub(crate) const fn multiface_face_property(direction: Direction) -> &'static BoolProperty {
    match direction {
        Direction::Up => &BlockStateProperties::UP,
        Direction::Down => &BlockStateProperties::DOWN,
        Direction::North => &BlockStateProperties::NORTH,
        Direction::South => &BlockStateProperties::SOUTH,
        Direction::East => &BlockStateProperties::EAST,
        Direction::West => &BlockStateProperties::WEST,
    }
}

/// Vanilla `RandomSource.nextInt(int)`: rejection-sampled bounded draw.
///
/// The block-behavior RNG arrives as an object-safe `&mut dyn Rng`, which only exposes
/// `next_u32`, so the vanilla algorithm is applied directly.
pub(crate) fn next_int_bounded(rng: &mut dyn rand::Rng, bound: i32) -> i32 {
    let mut r = rng.next_u32() as i32;
    let m = bound - 1;
    if bound & m == 0 {
        return r & m;
    }
    let mut u = (r as u32 >> 1) as i32;
    loop {
        r = u % bound;
        if u + m - r >= 0 {
            return r;
        }
        u = (rng.next_u32() >> 1) as i32;
    }
}

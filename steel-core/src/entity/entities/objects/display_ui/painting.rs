//! Vanilla painting entity: registry variant, facing, survival, drops.
//!
//! Vanilla reference: `Painting`
//! (`world/entity/decoration/painting/Painting.java`) on `HangingEntity`
//! (`world/entity/decoration/HangingEntity.java`).
//!
//! The synced variant reuses the generated
//! [`PaintingEntityData::painting_variant`] field: a `RegistryReference` into
//! [`PaintingVariantRegistry`](steel_registry::painting_variant::PaintingVariantRegistry),
//! so no variant list is hardcoded here. Placement ([`PaintingEntity::create_attached`])
//! reads the `placeable` painting-variant tag from the registry.
//!
//! Reported parity gaps:
//! - Vanilla's supporting-block test accepts solid blocks *or* diode blocks
//!   (`DiodeBlock.isDiode`). Steel's block registry exposes no diode query, so
//!   repeaters/comparators do not count as supports here.
//! - Vanilla's `hasLevelCollision` also rejects world-border collisions; Steel
//!   exposes no border-collision query for arbitrary AABBs, so only block
//!   collisions are checked.
//! - Steel has no `getPickResult` hook, so the middle-click painting item is
//!   client-side only.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::painting_variant::PaintingVariant;
use steel_registry::vanilla_entity_data::PaintingEntityData;
use steel_registry::vanilla_game_rules::ENTITY_DROPS;
use steel_registry::vanilla_painting_variant_tags::PaintingVariantTag;
use steel_registry::{
    REGISTRY, RegistryExt, RegistryReference, TaggedRegistryExt, sound_events, vanilla_entities,
    vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, Direction, Downcast as _, DowncastType, DowncastTypeKey, Identifier};
use steel_utils::{WorldAabb, axis::Axis};

use crate::entity::block_attached_entity::{BlockAttachedEntity, BlockAttachedEntityBase};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityBaseState, EntityMoveError, EntitySyncedData,
    SharedEntity, next_entity_id,
};
use crate::physics::{MoveResult, MoverType, WorldCollisionProvider, has_block_collision};
use crate::world::World;

/// Vanilla `Painting.DEPTH`: painting thickness along the facing axis.
const DEPTH: f64 = 0.0625;
/// Vanilla `Painting.calculateBoundingBox` wall offset (`shiftToBlockWall`).
const WALL_OFFSET: f64 = 0.46875;
/// Vanilla `HangingEntity.calculateSupportBox` epsilon.
const SUPPORT_EPSILON: f64 = 1.0e-7;

/// Vanilla painting entity.
#[entity_behavior(class = "Painting")]
pub struct PaintingEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<PaintingEntityData>,
    block_attached_entity_base: BlockAttachedEntityBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PaintingEntity`.
unsafe impl DowncastType for PaintingEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/painting");
}

impl PaintingEntity {
    /// Creates a fresh painting from the generic entity factory path.
    ///
    /// Factory spawns carry no placement rotation or variant, so this mirrors
    /// `Painting(level, pos)` with the default south facing and the registry's
    /// default variant, exactly like vanilla's plain constructor.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_attached(
            entity_type,
            id,
            BlockPos::new(
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            ),
            Direction::South,
            world,
        )
    }

    /// Creates a fresh painting attached to `block_pos`, facing `direction`.
    #[must_use]
    pub fn new_attached(
        entity_type: EntityTypeRef,
        id: i32,
        block_pos: BlockPos,
        direction: Direction,
        world: Weak<World>,
    ) -> Self {
        let entity = Self {
            base: EntityBase::new_with_state(
                id,
                EntityBaseState::new_with_bounding_box(
                    Self::painting_center(block_pos, direction, 1, 1),
                    entity_type.dimensions,
                    Self::painting_bounding_box(block_pos, direction, 1, 1),
                ),
                world,
            ),
            entity_type,
            entity_data: SyncMutex::new(PaintingEntityData::new()),
            block_attached_entity_base: BlockAttachedEntityBase::new(block_pos),
        };
        entity.set_direction(direction);
        entity
    }

    /// Creates a painting from persistent entity data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        let position = load.position;
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(PaintingEntityData::new()),
            block_attached_entity_base: BlockAttachedEntityBase::new(BlockPos::new(
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            )),
        }
    }

    /// Vanilla `Painting.create`: picks the largest fitting placeable variant.
    ///
    /// Returns `None` when no placeable variant survives at this position, which
    /// is also vanilla's "placement fails" signal.
    #[must_use]
    pub fn create_attached(
        world: &Arc<World>,
        block_pos: BlockPos,
        direction: Direction,
    ) -> Option<Self> {
        let candidates = REGISTRY
            .painting_variants
            .get_tag(&PaintingVariantTag::PLACEABLE)?;
        let fitting: Vec<&'static PaintingVariant> = candidates
            .into_iter()
            .filter(|variant| {
                let value = variant.value();
                let box_ =
                    Self::painting_bounding_box(block_pos, direction, value.width, value.height);
                Self::survives_box(world, i32::MIN, direction, box_)
            })
            .collect();
        if fitting.is_empty() {
            return None;
        }

        let largest = fitting
            .iter()
            .map(|variant| variant.value().width * variant.value().height)
            .max()?;
        let largest: Vec<&'static PaintingVariant> = fitting
            .into_iter()
            .filter(|variant| variant.value().width * variant.value().height == largest)
            .collect();
        let selected = largest[rand::random_range(0..largest.len())];

        let painting = Self::new_attached(
            &vanilla_entities::PAINTING,
            next_entity_id(),
            block_pos,
            direction,
            Arc::downgrade(world),
        );
        painting.set_variant(selected);
        Some(painting)
    }

    /// Returns the painting's facing direction.
    #[must_use]
    pub fn direction(&self) -> Direction {
        *self.entity_data.lock().hanging_entity().direction.get()
    }

    /// Returns the current painting variant reference.
    #[must_use]
    pub fn variant(&self) -> &'static PaintingVariant {
        self.entity_data.lock().painting_variant.get().value()
    }

    /// Returns the current variant's registry key (vanilla NBT `variant`).
    #[must_use]
    pub fn variant_key(&self) -> Identifier {
        self.variant().key.clone()
    }

    /// Sets the variant from a registry key.
    ///
    /// Returns `false` for unknown keys, matching vanilla's "variant not found
    /// in registry" load behavior (the previous variant is kept).
    pub fn set_variant_key(&self, key: &Identifier) -> bool {
        match REGISTRY.painting_variants.by_key(key) {
            Some(variant) => {
                let variant: &'static PaintingVariant = variant;
                self.set_variant(variant);
                true
            }
            None => false,
        }
    }

    /// Sets the variant and reflows the bounding box like vanilla's synced-data hook.
    pub fn set_variant(&self, variant: &'static PaintingVariant) {
        self.entity_data
            .lock()
            .painting_variant
            .set(RegistryReference::new(variant));
        self.recalculate_position_or_warn();
    }

    /// Sets the facing direction, yaw, and bounding box (vanilla `HangingEntity.setDirection`).
    pub fn set_direction(&self, direction: Direction) {
        debug_assert!(
            direction.is_horizontal(),
            "paintings only face horizontal directions"
        );
        self.entity_data
            .lock()
            .hanging_entity_mut()
            .direction
            .set(direction);
        let yaw = direction_2d_data_value(direction) as f32 * 90.0;
        self.base.set_rotation((yaw, self.rotation().1));
        self.base.set_old_rotation_to_current();
        self.recalculate_position_or_warn();
    }

    /// Vanilla `Painting.playPlacementSound`.
    pub fn play_placement_sound(&self) {
        self.play_sound(&sound_events::ENTITY_PAINTING_PLACE, 1.0, 1.0);
    }

    /// Vanilla `Painting.offsetForPaintingSize`: even sizes shift half a block.
    const fn offset_for_painting_size(size: i32) -> f64 {
        if size % 2 == 0 { 0.5 } else { 0.0 }
    }

    /// Center of the vanilla painting box, before the variant is applied.
    fn painting_center(
        block_pos: BlockPos,
        direction: Direction,
        width: i32,
        height: i32,
    ) -> DVec3 {
        Self::painting_bounding_box(block_pos, direction, width, height).center()
    }

    /// Vanilla `Painting.calculateBoundingBox`.
    #[must_use]
    pub fn painting_bounding_box(
        block_pos: BlockPos,
        direction: Direction,
        width: i32,
        height: i32,
    ) -> WorldAabb {
        let wall = DVec3::new(
            f64::from(block_pos.x()) + 0.5,
            f64::from(block_pos.y()) + 0.5,
            f64::from(block_pos.z()) + 0.5,
        ) - direction.offset_vec().as_dvec3() * WALL_OFFSET;
        let left = direction.rotate_y_counter_clockwise();
        let position = wall
            + left.offset_vec().as_dvec3() * Self::offset_for_painting_size(width)
            + DVec3::new(0.0, Self::offset_for_painting_size(height), 0.0);
        let (x_size, y_size, z_size) = if direction.axis() == Axis::X {
            (DEPTH, f64::from(height), f64::from(width))
        } else {
            (f64::from(width), f64::from(height), DEPTH)
        };
        WorldAabb::of_size(position, x_size, y_size, z_size)
    }

    /// Current variant width/height.
    fn variant_size(&self) -> (i32, i32) {
        let value = self.variant().value();
        (value.width, value.height)
    }

    /// Recomputes the entity center and bounding box from facing + variant.
    fn try_recalculate_position(&self) -> Result<(), EntityMoveError> {
        let block_pos = self.block_attached_entity_base.pos();
        let direction = self.direction();
        let (width, height) = self.variant_size();
        let box_ = Self::painting_bounding_box(block_pos, direction, width, height);
        let center = box_.center();
        self.base.try_set_position(center)?;
        self.base.set_bounding_box(box_);
        Ok(())
    }

    fn recalculate_position_or_warn(&self) {
        if let Err(error) = self.try_recalculate_position() {
            log::warn!(
                "failed to commit painting {} position recalculation: {error}",
                self.base.id()
            );
        }
    }

    /// Vanilla `HangingEntity.survives` against a candidate box.
    fn survives_box(
        world: &Arc<World>,
        self_id: i32,
        direction: Direction,
        box_: WorldAabb,
    ) -> bool {
        if has_block_collision(&WorldCollisionProvider::new(world), box_) {
            return false;
        }

        // Vanilla `calculateSupportBox()`: shifted half a block behind the
        // painting and deflated by 1.0E-7.
        let support_box = box_
            .translate(direction.offset_vec().as_dvec3() * -0.5)
            .deflate(SUPPORT_EPSILON);
        if !support_box_covers(world, support_box) {
            return false;
        }

        !has_coexisting_hanging(world, self_id, direction, box_)
    }

    /// Vanilla `Block.popResource` spawn through `HangingEntity.spawnAtLocation`,
    /// which offsets the drop by 0.15 blocks along the facing direction.
    fn drop_painting_item(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let mut item = ItemStack::new(&vanilla_items::PAINTING);
        if let Some(custom_name) = self.custom_name() {
            item.set(CUSTOM_NAME, custom_name);
        }
        let offset = self.direction().offset_vec().as_dvec3() * 0.15;
        let _ = world.spawn_item(self.position() + offset, item);
    }
}

/// Vanilla `HangingEntity.isSupportingBlock`: every block in the support box
/// must be solid.
///
/// Vanilla also accepts diode blocks; Steel has no diode query (reported).
fn support_box_covers(world: &World, support_box: WorldAabb) -> bool {
    if support_box.is_empty() {
        return false;
    }
    let min = BlockPos::containing(
        support_box.min_x(),
        support_box.min_y(),
        support_box.min_z(),
    );
    let max = BlockPos::containing(
        support_box.max_x() - SUPPORT_EPSILON,
        support_box.max_y() - SUPPORT_EPSILON,
        support_box.max_z() - SUPPORT_EPSILON,
    );
    BlockPos::between_closed(min, max).all(|pos| world.get_block_state(pos).is_solid())
}

/// Vanilla `HangingEntity.canCoexist(false)`: any other hanging entity that
/// shares this painting's type or facing blocks placement.
fn has_coexisting_hanging(
    world: &Arc<World>,
    self_id: i32,
    direction: Direction,
    box_: WorldAabb,
) -> bool {
    world
        .get_entities_in_aabb_matching(&box_, |entity| {
            entity.id() != self_id
                && (entity.downcast_ref::<PaintingEntity>().is_some()
                    || entity.as_item_frame().is_some())
        })
        .into_iter()
        .any(|entity| {
            if entity.downcast_ref::<PaintingEntity>().is_some() {
                // Paintings always conflict with other paintings.
                return true;
            }
            entity
                .as_item_frame()
                .is_some_and(|frame| frame.direction() == direction)
        })
}

impl Entity for PaintingEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn spawn_data(&self) -> i32 {
        direction_3d_data_value(self.direction())
    }

    fn spawn_position(&self) -> DVec3 {
        let block_pos = self.block_attached_entity_base.pos();
        DVec3::new(
            f64::from(block_pos.x()),
            f64::from(block_pos.y()),
            f64::from(block_pos.z()),
        )
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn tick(self: Arc<Self>) {
        self.tick_block_attached_entity();
    }

    fn is_pickable(&self) -> bool {
        self.is_pickable_block_attached_entity()
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.hurt_block_attached_entity(world, source, amount)
    }

    fn skip_attack_interaction(&self, source: &SharedEntity) -> bool {
        self.skip_attack_interaction_block_attached_entity(source)
    }

    fn push_impulse(&self, impulse: DVec3) {
        self.push_impulse_block_attached_entity(impulse);
    }

    fn move_entity(self: Arc<Self>, mover_type: MoverType, delta: DVec3) -> Option<MoveResult> {
        self.move_entity_block_attached_entity(mover_type, delta)
    }

    fn refresh_dimensions(&self) {
        self.refresh_dimensions_block_attached_entity();
    }

    fn try_set_position(&self, pos: DVec3) -> Result<(), EntityMoveError> {
        self.try_set_position_block_attached_entity(pos)
    }

    fn snap_to(&self, position: DVec3, _yaw: f32, _pitch: f32) {
        // Vanilla `Painting.snapTo` only moves the entity; rotation comes from
        // the facing direction.
        if let Err(error) = self.try_set_position(position) {
            log::warn!(
                "failed to commit painting {} snap position: {error}",
                self.base.id()
            );
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_block_attached_entity(nbt);
        // Vanilla `HangingEntity` writes `facing` with the 2D data codec.
        nbt.insert(
            "facing",
            i8::try_from(direction_2d_data_value(self.direction())).unwrap_or(0),
        );
        nbt.insert("variant", self.variant_key().to_string());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_block_attached_entity(nbt);

        let facing = nbt
            .byte("facing")
            .and_then(|value| direction_from_2d_data_value(i32::from(value)))
            .or_else(|| nbt.int("facing").and_then(direction_from_2d_data_value));
        if let Some(direction) = facing {
            self.set_direction(direction);
        }

        if let Some(variant) = nbt
            .string("variant")
            .and_then(|key| key.to_str().as_ref().parse::<Identifier>().ok())
        {
            self.set_variant_key(&variant);
        }

        self.recalculate_position_or_warn();
    }
}

impl BlockAttachedEntity for PaintingEntity {
    fn block_attached_entity_base(&self) -> &BlockAttachedEntityBase {
        &self.block_attached_entity_base
    }

    fn survives(&self) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        let block_pos = self.block_attached_entity_base.pos();
        let direction = self.direction();
        let (width, height) = self.variant_size();
        let box_ = Self::painting_bounding_box(block_pos, direction, width, height);
        Self::survives_box(&world, self.base.id(), direction, box_)
    }

    fn drop_item(&self, caused_by: Option<&dyn Entity>) {
        let Some(world) = self.level() else {
            return;
        };
        if !world.get_game_rule(&ENTITY_DROPS) {
            return;
        }
        self.play_sound(&sound_events::ENTITY_PAINTING_BREAK, 1.0, 1.0);
        let infinite = caused_by
            .and_then(|entity| entity.as_player())
            .is_some_and(|player| player.has_infinite_materials());
        if infinite {
            return;
        }
        self.drop_painting_item();
    }

    fn recalculate_bounding_box(&self) -> Result<(), EntityMoveError> {
        self.try_recalculate_position()
    }
}

/// Vanilla `Direction.get2DDataValue`: South 0, West 1, North 2, East 3.
const fn direction_2d_data_value(direction: Direction) -> i32 {
    match direction {
        Direction::South | Direction::Down | Direction::Up => 0,
        Direction::West => 1,
        Direction::North => 2,
        Direction::East => 3,
    }
}

/// Vanilla `Direction.from2DDataValue`.
const fn direction_from_2d_data_value(value: i32) -> Option<Direction> {
    match value.rem_euclid(4) {
        0 => Some(Direction::South),
        1 => Some(Direction::West),
        2 => Some(Direction::North),
        3 => Some(Direction::East),
        _ => None,
    }
}

/// Vanilla `Direction.get3DDataValue` for the add-entity packet data field.
const fn direction_3d_data_value(direction: Direction) -> i32 {
    match direction {
        Direction::Down => 0,
        Direction::Up => 1,
        Direction::North => 2,
        Direction::South => 3,
        Direction::West => 4,
        Direction::East => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::string::ToString;
    use steel_registry::vanilla_entities;

    fn new_painting() -> PaintingEntity {
        steel_registry::init_vanilla_registry();
        PaintingEntity::new_attached(
            &vanilla_entities::PAINTING,
            1,
            BlockPos::new(10, 64, -3),
            Direction::North,
            Weak::new(),
        )
    }

    /// Re-borrows owned NBT the way entity loading does from disk.
    fn with_reborrow<R>(
        nbt: &simdnbt::owned::NbtCompound,
        visitor: impl FnOnce(simdnbt::borrow::NbtCompound<'_, '_>) -> R,
    ) -> R {
        use simdnbt::borrow::read_compound;
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = Cursor::new(bytes.as_slice());
        let base = read_compound(&mut cursor).expect("entity NBT should reborrow");
        let borrowed: simdnbt::borrow::NbtCompound<'_, '_> = (&base).into();
        visitor(borrowed)
    }

    #[test]
    fn facing_drives_spawn_data_and_yaw() {
        let painting = new_painting();
        assert_eq!(painting.direction(), Direction::North);
        assert_eq!(painting.spawn_data(), 2);
        assert_eq!(painting.rotation().0, 180.0);
        assert_eq!(painting.spawn_position(), DVec3::new(10.0, 64.0, -3.0));
    }

    #[test]
    fn bounding_box_matches_vanilla_geometry() {
        let painting = new_painting();
        let expected = PaintingEntity::painting_bounding_box(
            BlockPos::new(10, 64, -3),
            Direction::North,
            1,
            1,
        );
        assert_eq!(painting.bounding_box(), expected);
        // 1x1 north: DEPTH thick in Z, one block in X and Y.
        assert!((expected.max_z() - expected.min_z() - DEPTH).abs() < 1.0e-9);
        assert!((expected.max_x() - expected.min_x() - 1.0).abs() < 1.0e-9);
        assert!((expected.max_y() - expected.min_y() - 1.0).abs() < 1.0e-9);
    }

    #[test]
    fn even_sizes_shift_half_a_block() {
        let odd = PaintingEntity::painting_bounding_box(BlockPos::ZERO, Direction::South, 1, 1);
        let even = PaintingEntity::painting_bounding_box(BlockPos::ZERO, Direction::South, 2, 2);
        assert!((even.center().x - odd.center().x - 0.5).abs() < 1.0e-9);
        assert!((even.center().y - odd.center().y - 0.5).abs() < 1.0e-9);
    }

    #[test]
    fn facing_and_variant_round_trip_through_nbt() {
        let painting = new_painting();
        painting.set_direction(Direction::East);
        let mut nbt = NbtCompound::new();
        painting.save_additional(&mut nbt);
        assert_eq!(nbt.byte("facing"), Some(3));

        let variant_key = painting.variant_key().to_string();
        assert_eq!(
            nbt.string("variant").map(ToString::to_string),
            Some(variant_key.clone())
        );

        let reloaded = new_painting();
        with_reborrow(&nbt, |borrowed| reloaded.load_additional(borrowed));
        assert_eq!(reloaded.direction(), Direction::East);
        assert_eq!(reloaded.variant_key().to_string(), variant_key);
    }

    #[test]
    fn unknown_variants_keep_the_previous_selection() {
        let painting = new_painting();
        let before = painting.variant_key();
        let unknown = "minecraft:not_a_painting"
            .parse()
            .expect("valid identifier");
        assert!(!painting.set_variant_key(&unknown));
        assert_eq!(painting.variant_key(), before);
    }

    #[test]
    fn registry_variant_sizes_are_used_for_the_box() {
        let painting = new_painting();
        let (width, height) = painting.variant_size();
        assert!(width >= 1 && height >= 1);
        assert_eq!(
            painting.bounding_box(),
            PaintingEntity::painting_bounding_box(
                BlockPos::new(10, 64, -3),
                Direction::North,
                width,
                height
            )
        );
    }
}

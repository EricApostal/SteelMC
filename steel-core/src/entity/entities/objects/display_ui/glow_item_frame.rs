//! Glow item frame entity: placement, synced item/facing data, persistence,
//! comparator integration, survival checks, drops, damage and interaction.
//!
//! Vanilla reference: `GlowItemFrame`
//! (`world/entity/decoration/GlowItemFrame.java`) on `HangingEntity`
//! (`world/entity/decoration/HangingEntity.java`) and `BlockAttachedEntity`.
//!
//! Reported parity gaps:
//! - Vanilla `removeFramedMap` needs `MapItemSavedData`. Steel's map data model
//!   exists (`steel-core/src/map/data.rs`, `MapItemSavedData::remove_frame`,
//!   `steel-core/src/map/storage.rs`, `MapStorage::with_map_mut`) but `World`
//!   exposes no map-storage accessor, so the frame cannot resolve its map id to
//!   saved data. `MapItemSavedData` also has no public
//!   `is_tracked_count_over_limit`, which vanilla's interact path uses to reject
//!   a 257th tracked decoration.
//! - The `Entity` trait has no `getPickResult` hook (only blocks expose
//!   `get_clone_item_stack`), so vanilla `ItemFrame.getPickResult` has nowhere
//!   to land.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::DEGREE_90;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, MAP_ID};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entity_data::GlowItemFrameEntityData;
use steel_registry::vanilla_game_rules::ENTITY_DROPS;
use steel_registry::{sound_events, vanilla_blocks, vanilla_game_events, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{
    BlockPos, Direction, Downcast as _, DowncastType, DowncastTypeKey, WorldAabb, axis::Axis,
};

use crate::behavior::{BLOCK_BEHAVIORS, InteractionResult};
use crate::entity::block_attached_entity::{BlockAttachedEntity, BlockAttachedEntityBase};
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::display_ui::PaintingEntity;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityBaseState, EntityMoveError, EntitySyncedData,
    ItemFrame, SharedEntity,
};
use crate::physics::{
    CollisionWorld, MoveResult, MoverType, WorldCollisionProvider, has_block_collision,
};
use crate::player::Player;
use crate::world::World;

/// Vanilla `ItemFrame.NUM_ROTATIONS`.
const NUM_ROTATIONS: i32 = 8;
/// Vanilla `ItemFrame.DEFAULT_DROP_CHANCE`.
const DEFAULT_DROP_CHANCE: f32 = 1.0;
/// Vanilla `HangingEntity.spawnAtLocation` attachment offset.
const DROP_OFFSET: f64 = 0.15;

/// Glow item frame state needed by end-city structure markers.
///
/// Implements the vanilla server surface: placement, synced item/facing data,
/// persistence, comparator integration, support checks, drops, damage and
/// interaction.
#[entity_behavior(class = "GlowItemFrame")]
pub struct GlowItemFrameEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<GlowItemFrameEntityData>,
    block_attached_entity_base: BlockAttachedEntityBase,
    /// Vanilla `ItemFrame.dropChance`: probability the framed item drops.
    drop_chance: SyncMutex<f32>,
    /// Vanilla `ItemFrame.fixed`: indestructible except by creative or
    /// `BYPASSES_INVULNERABILITY` damage, and immune to movement/pushing.
    fixed: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `GlowItemFrameEntity`.
unsafe impl DowncastType for GlowItemFrameEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/glow_item_frame");
}

impl GlowItemFrameEntity {
    /// Creates a fresh item frame from the generic entity factory path.
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

    /// Creates a fresh item frame attached to `block_pos`.
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
                    Self::frame_center(block_pos, direction),
                    entity_type.dimensions,
                    Self::frame_bounding_box(block_pos, direction, false),
                )
                .with_rotation(Self::rotation_for_direction(direction)),
                world,
            ),
            entity_type,
            entity_data: SyncMutex::new(GlowItemFrameEntityData::new()),
            block_attached_entity_base: BlockAttachedEntityBase::new(block_pos),
            drop_chance: SyncMutex::new(DEFAULT_DROP_CHANCE),
            fixed: SyncMutex::new(false),
        };
        entity
            .entity_data
            .lock()
            .item_frame
            .hanging_entity
            .direction
            .set(direction);
        entity
    }

    /// Creates an item frame from persistent entity data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        let position = load.position;
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(GlowItemFrameEntityData::new()),
            block_attached_entity_base: BlockAttachedEntityBase::new(BlockPos::new(
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            )),
            drop_chance: SyncMutex::new(DEFAULT_DROP_CHANCE),
            fixed: SyncMutex::new(false),
        }
    }

    /// Vanilla `ItemFrame.getItem`.
    #[must_use]
    pub fn item(&self) -> ItemStack {
        self.entity_data.lock().item_frame.item.get().clone()
    }

    /// Vanilla `ItemFrame.getRotation`.
    #[must_use]
    pub fn rotation(&self) -> i32 {
        *self.entity_data.lock().item_frame.rotation.get()
    }

    /// Vanilla `ItemFrame.setRotation(int)`.
    pub fn set_rotation(&self, rotation: i32) {
        self.set_rotation_with_update(rotation, true);
    }

    /// Vanilla `ItemFrame.setRotation(int, boolean)`.
    fn set_rotation_with_update(&self, rotation: i32, update_neighbors: bool) {
        self.entity_data
            .lock()
            .item_frame
            .rotation
            .set(rotation.rem_euclid(NUM_ROTATIONS));
        if update_neighbors && let Some(world) = self.level() {
            world.update_neighbor_for_output_signal(
                self.block_attached_entity_base.pos(),
                &vanilla_blocks::AIR,
            );
        }
    }

    /// Vanilla `ItemFrame.dropChance`.
    #[must_use]
    pub fn drop_chance(&self) -> f32 {
        *self.drop_chance.lock()
    }

    /// Vanilla `ItemFrame.fixed`.
    #[must_use]
    pub fn is_fixed(&self) -> bool {
        *self.fixed.lock()
    }

    /// Sets the framed item, matching vanilla by storing a single item.
    pub fn set_item(&self, item: ItemStack) {
        self.set_item_with_update(item, true);
    }

    /// Sets the framed item and optionally notifies nearby comparators.
    pub(crate) fn set_item_with_update(&self, mut item: ItemStack, update_comparators: bool) {
        let has_item = !item.is_empty();
        if has_item {
            item.set_count(1);
        }
        self.entity_data.lock().item_frame.item.set(item);
        self.recalculate_position_or_warn();
        if has_item {
            self.play_sound(&sound_events::ENTITY_GLOW_ITEM_FRAME_ADD_ITEM, 1.0, 1.0);
        }
        if update_comparators && let Some(world) = self.level() {
            world.update_neighbor_for_output_signal(
                self.block_attached_entity_base.pos(),
                &vanilla_blocks::AIR,
            );
        }
    }

    fn set_direction(&self, direction: Direction) {
        self.entity_data
            .lock()
            .item_frame
            .hanging_entity
            .direction
            .set(direction);
        self.base
            .set_rotation(Self::rotation_for_direction(direction));
        self.recalculate_position_or_warn();
    }

    /// Vanilla `HangingEntity.survives`.
    ///
    /// Exposed as an inherent method because [`BlockAttachedEntity`] lives in
    /// the private `crate::entity::block_attached_entity` module, while
    /// `HangingEntityItem` needs the same gate vanilla runs before adding the
    /// frame to the level.
    pub fn survives(&self) -> bool {
        BlockAttachedEntity::survives(self)
    }

    /// Vanilla `ItemFrame.playPlacementSound`: `ITEM_FRAME_PLACE` at the frame,
    /// volume `1.0`, pitch `1.0`, neutral source.
    pub fn play_placement_sound(&self) {
        if let Some(world) = self.level() {
            world.play_sound_at(
                &sound_events::ENTITY_GLOW_ITEM_FRAME_PLACE,
                SoundSource::Neutral,
                self.position(),
                1.0,
                1.0,
                None,
            );
        }
    }

    fn try_recalculate_position(&self) -> Result<(), EntityMoveError> {
        let block_pos = self.block_attached_entity_base.pos();
        let direction = *self
            .entity_data
            .lock()
            .item_frame
            .hanging_entity
            .direction
            .get();
        let position = Self::frame_center(block_pos, direction);
        self.base.try_set_position(position)?;
        self.base.set_bounding_box(Self::frame_bounding_box(
            block_pos,
            direction,
            self.has_framed_map(),
        ));

        Ok(())
    }

    fn recalculate_position_or_warn(&self) {
        if let Err(error) = self.try_recalculate_position() {
            log::warn!(
                "failed to commit glow item frame {} position recalculation: {error}",
                self.base.id()
            );
        }
    }

    fn has_framed_map(&self) -> bool {
        self.entity_data.lock().item_frame.item.get().has(MAP_ID)
    }

    fn frame_center(block_pos: BlockPos, direction: Direction) -> DVec3 {
        let off = direction.offset_vec().as_dvec3() * 0.46875;
        block_pos.0.as_dvec3() + DVec3::splat(0.5) - off
    }

    fn rotation_for_direction(direction: Direction) -> (f32, f32) {
        if direction.is_horizontal() {
            (
                f32::from(direction_2d_data_value(direction)) * DEGREE_90,
                0.0,
            )
        } else {
            let pitch = match direction {
                Direction::Up => -DEGREE_90,
                Direction::Down => DEGREE_90,
                Direction::North | Direction::South | Direction::West | Direction::East => 0.0,
            };
            (0.0, pitch)
        }
    }

    fn frame_bounding_box(
        block_pos: BlockPos,
        direction: Direction,
        has_framed_map: bool,
    ) -> WorldAabb {
        let center = Self::frame_center(block_pos, direction);
        let size = if has_framed_map { 1.0 } else { 0.75 };
        let x_size = if direction.axis() == Axis::X {
            0.0625
        } else {
            size
        };
        let y_size = if direction.axis() == Axis::Y {
            0.0625
        } else {
            size
        };
        let z_size = if direction.axis() == Axis::Z {
            0.0625
        } else {
            size
        };
        WorldAabb::new(
            center.x - x_size / 2.0,
            center.y - y_size / 2.0,
            center.z - z_size / 2.0,
            center.x + x_size / 2.0,
            center.y + y_size / 2.0,
            center.z + z_size / 2.0,
        )
    }

    /// Vanilla `ItemFrame.getFrameItemStack`.
    fn frame_item_stack(&self) -> ItemStack {
        let mut stack = ItemStack::new(&vanilla_items::GLOW_ITEM_FRAME);
        if let Some(custom_name) = self.custom_name() {
            stack.set(CUSTOM_NAME, custom_name);
        }
        stack
    }

    /// Vanilla `HangingEntity.spawnAtLocation`: offsets the drop by `0.15`
    /// blocks along the frame's facing direction.
    fn spawn_drop(&self, stack: ItemStack) {
        let step = self.direction().offset_vec();
        let offset = DVec3::new(
            f64::from(step.x) * DROP_OFFSET,
            0.0,
            f64::from(step.z) * DROP_OFFSET,
        );
        self.spawn_at_location_with_offset(stack, offset);
    }

    /// Vanilla `ItemFrame.dropItem(level, causedBy, withFrame)`.
    fn drop_item_inner(&self, caused_by: Option<&dyn Entity>, with_frame: bool) {
        if self.is_fixed() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };

        let mut item = self.item();
        self.set_item_with_update(ItemStack::empty(), true);

        let caused_player = caused_by.and_then(|entity| entity.as_player());
        if !world.get_game_rule(&ENTITY_DROPS) {
            // Vanilla `removeFramedMap(item)` when `causedBy == null`: no
            // map-storage accessor on `World` (reported gap).
            return;
        }
        if caused_player.is_some_and(|player| player.has_infinite_materials()) {
            // Vanilla `removeFramedMap(item)`: no map-storage accessor on
            // `World` (reported gap).
            return;
        }

        if with_frame {
            self.spawn_drop(self.frame_item_stack());
        }

        if !item.is_empty() {
            item.set_count(1);
            // Vanilla `removeFramedMap(item)`: no map-storage accessor on
            // `World` (reported gap).
            if rand::random::<f32>() < self.drop_chance() {
                self.spawn_drop(item);
            }
        }
    }

    /// Vanilla `ItemFrame.hurtServer`.
    fn hurt_item_frame(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.is_fixed() {
            return can_hurt_when_fixed(source)
                && self.hurt_block_attached_entity(world, source, amount);
        }

        if self.is_invulnerable_to_base(source) {
            return false;
        }

        if !source.is(&DamageTypeTag::IS_EXPLOSION) && !self.item().is_empty() {
            let caused_by = source.causing_entity().map(AsRef::as_ref);
            self.drop_item_inner(caused_by, false);
            self.game_event_with_source_entity(&vanilla_game_events::BLOCK_CHANGE, caused_by);
            self.play_sound(&sound_events::ENTITY_GLOW_ITEM_FRAME_REMOVE_ITEM, 1.0, 1.0);
            return true;
        }

        self.hurt_block_attached_entity(world, source, amount)
    }

    /// Vanilla `ItemFrame.interact`.
    fn interact_item_frame(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        if self.is_fixed() {
            return InteractionResult::Pass;
        }

        let held_item = player.inventory.lock().get_item_in_hand(hand).clone();
        let frame_has_item = !self.item().is_empty();
        if frame_has_item {
            self.play_sound(&sound_events::ENTITY_GLOW_ITEM_FRAME_ROTATE_ITEM, 1.0, 1.0);
            self.set_rotation(self.rotation() + 1);
            self.game_event_with_source_entity(&vanilla_game_events::BLOCK_CHANGE, Some(player));
            return InteractionResult::Success;
        }

        if held_item.is_empty() || self.is_removed() {
            return InteractionResult::Pass;
        }

        // Vanilla also rejects a held map whose saved data tracks more than 256
        // decorations; Steel exposes no tracked-count query (reported gap).
        self.set_item(held_item);
        self.game_event_with_source_entity(&vanilla_game_events::BLOCK_CHANGE, Some(player));
        let has_infinite_materials = player.has_infinite_materials();
        player
            .inventory
            .lock()
            .mutate_item_in_hand(hand, |stack| stack.consume(1, has_infinite_materials));
        InteractionResult::Success
    }
}

/// Vanilla `ItemFrame.canHurtWhenFixed`.
fn can_hurt_when_fixed(source: &DamageSource) -> bool {
    source.is(&DamageTypeTag::BYPASSES_INVULNERABILITY) || source.is_creative_player()
}

/// Vanilla `HangingEntity.canCoexist`: any other hanging entity in the pop box
/// with the same direction (or, unless allowed, the same type) blocks the frame.
fn can_coexist(
    world: &World,
    self_id: i32,
    self_type: EntityTypeRef,
    direction: Direction,
    pop_box: WorldAabb,
    allow_intersecting_same_type: bool,
) -> bool {
    !world.has_entity_in_aabb_matching(&pop_box, |entity| {
        if entity.id() == self_id {
            return false;
        }
        let intersects_same_type =
            !allow_intersecting_same_type && std::ptr::eq(entity.entity_type(), self_type);
        intersects_same_type || hanging_direction(entity) == Some(direction)
    })
}

/// Vanilla `HangingEntity.getDirection` for every hanging entity kind.
fn hanging_direction(entity: &dyn Entity) -> Option<Direction> {
    if let Some(frame) = entity.as_item_frame() {
        return Some(frame.direction());
    }
    entity
        .downcast_ref::<PaintingEntity>()
        .map(PaintingEntity::direction)
}

impl ItemFrame for GlowItemFrameEntity {
    fn direction(&self) -> Direction {
        *self
            .entity_data
            .lock()
            .item_frame
            .hanging_entity
            .direction
            .get()
    }

    fn analog_output(&self) -> i32 {
        let entity_data = self.entity_data.lock();
        if entity_data.item_frame.item.get().is_empty() {
            0
        } else {
            *entity_data.item_frame.rotation.get() % NUM_ROTATIONS + 1
        }
    }
}

impl Entity for GlowItemFrameEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn spawn_data(&self) -> i32 {
        direction_3d_data_value(
            *self
                .entity_data
                .lock()
                .item_frame
                .hanging_entity
                .direction
                .get(),
        )
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
        self.hurt_item_frame(world, source, amount)
    }

    fn interact(
        &self,
        player: &Player,
        hand: InteractionHand,
        _location: DVec3,
    ) -> InteractionResult {
        self.interact_item_frame(player, hand)
    }

    fn skip_attack_interaction(&self, source: &SharedEntity) -> bool {
        self.skip_attack_interaction_block_attached_entity(source)
    }

    fn push_impulse(&self, impulse: DVec3) {
        if !self.is_fixed() {
            self.push_impulse_block_attached_entity(impulse);
        }
    }

    fn move_entity(self: Arc<Self>, mover_type: MoverType, delta: DVec3) -> Option<MoveResult> {
        if self.is_fixed() {
            return None;
        }
        self.move_entity_block_attached_entity(mover_type, delta)
    }

    fn refresh_dimensions(&self) {
        self.refresh_dimensions_block_attached_entity();
    }

    fn try_set_position(&self, pos: DVec3) -> Result<(), EntityMoveError> {
        self.try_set_position_block_attached_entity(pos)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_block_attached_entity(nbt);

        // Read the separately-locked state first: `is_invisible` locks the same
        // entity-data mutex that is held below.
        let drop_chance = self.drop_chance();
        let invisible = self.is_invisible();
        let fixed = self.is_fixed();

        let entity_data = self.entity_data.lock();
        let item = entity_data.item_frame.item.get();
        if !item.is_empty() {
            nbt.insert("Item", item.to_nbt_tag_ref());
        }
        nbt.insert("ItemRotation", *entity_data.item_frame.rotation.get() as i8);
        nbt.insert("ItemDropChance", drop_chance);
        nbt.insert(
            "Facing",
            direction_3d_data_value(*entity_data.item_frame.hanging_entity.direction.get()) as i8,
        );
        nbt.insert("Invisible", i8::from(invisible));
        nbt.insert("Fixed", i8::from(fixed));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_block_attached_entity(nbt);

        let item = nbt
            .compound("Item")
            .and_then(|item_tag| ItemStack::from_borrowed_compound(&item_tag))
            .unwrap_or_else(ItemStack::empty);
        self.set_item_with_update(item, false);

        self.set_rotation_with_update(i32::from(nbt.byte("ItemRotation").unwrap_or(0)), false);

        *self.drop_chance.lock() = nbt.float("ItemDropChance").unwrap_or(DEFAULT_DROP_CHANCE);

        let facing = nbt
            .byte("Facing")
            .and_then(|value| direction_from_3d_data_value(i32::from(value)))
            .or_else(|| nbt.int("Facing").and_then(direction_from_3d_data_value))
            .unwrap_or(Direction::Down);
        self.set_direction(facing);

        self.entity_data
            .set_base_invisible_flag(nbt.byte("Invisible").is_some_and(|value| value != 0));
        *self.fixed.lock() = nbt.byte("Fixed").is_some_and(|value| value != 0);

        self.recalculate_position_or_warn();
    }
}

impl BlockAttachedEntity for GlowItemFrameEntity {
    fn block_attached_entity_base(&self) -> &BlockAttachedEntityBase {
        &self.block_attached_entity_base
    }

    fn survives(&self) -> bool {
        if self.is_fixed() {
            return true;
        }

        let Some(world) = self.level() else {
            return false;
        };
        let block_pos = self.block_attached_entity_base.pos();
        let direction = self.direction();

        // Vanilla `hasLevelCollision(getPopBox())`: block and border collisions.
        let pop_box = Self::frame_bounding_box(block_pos, direction, false);
        let collision_world = WorldCollisionProvider::for_entity(&world, self);
        if has_block_collision(&collision_world, pop_box)
            || collision_world.has_world_border_collision(&pop_box)
        {
            return false;
        }

        // Vanilla `isSupportingBlock`: solid, or a diode for horizontal frames.
        let support_state = world.get_block_state(block_pos.relative(direction.opposite()));
        let supported = support_state.is_solid()
            || direction.is_horizontal()
                && BLOCK_BEHAVIORS
                    .get_behavior(support_state.get_block())
                    .is_diode();
        supported
            && can_coexist(
                &world,
                self.id(),
                self.entity_type,
                direction,
                pop_box,
                true,
            )
    }

    fn drop_item(&self, caused_by: Option<&dyn Entity>) {
        self.play_sound(&sound_events::ENTITY_GLOW_ITEM_FRAME_BREAK, 1.0, 1.0);
        self.drop_item_inner(caused_by, true);
        self.game_event_with_source_entity(&vanilla_game_events::BLOCK_CHANGE, caused_by);
    }

    fn recalculate_bounding_box(&self) -> Result<(), EntityMoveError> {
        self.try_recalculate_position()
    }
}

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

const fn direction_from_3d_data_value(value: i32) -> Option<Direction> {
    match value {
        0 => Some(Direction::Down),
        1 => Some(Direction::Up),
        2 => Some(Direction::North),
        3 => Some(Direction::South),
        4 => Some(Direction::West),
        5 => Some(Direction::East),
        _ => None,
    }
}

const fn direction_2d_data_value(direction: Direction) -> u8 {
    match direction {
        Direction::South | Direction::Down | Direction::Up => 0,
        Direction::West => 1,
        Direction::North => 2,
        Direction::East => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;
    use steel_registry::{vanilla_entities, vanilla_items};

    #[test]
    fn item_frame_persists_structure_marker_state() {
        let frame = GlowItemFrameEntity::new_attached(
            &vanilla_entities::GLOW_ITEM_FRAME,
            1,
            BlockPos::new(12, 80, 14),
            Direction::West,
            Weak::new(),
        );
        frame.set_item(ItemStack::new(&vanilla_items::ELYTRA));

        let mut nbt = NbtCompound::new();
        frame.save_additional(&mut nbt);

        assert_eq!(nbt.byte("Facing"), Some(4));
        assert_eq!(nbt.byte("ItemRotation"), Some(0));
        assert_eq!(nbt.float("ItemDropChance"), Some(1.0));
        assert_eq!(nbt.byte("Invisible"), Some(0));
        assert_eq!(nbt.byte("Fixed"), Some(0));
        let Some(item) = nbt.compound("Item") else {
            panic!("glow item frame should save framed item");
        };
        assert_eq!(
            item.string("id").map(ToString::to_string),
            Some("minecraft:elytra".to_owned())
        );
        assert_eq!(item.int("count"), Some(1));
    }

    #[test]
    fn item_frame_is_pickable_like_vanilla() {
        let frame = GlowItemFrameEntity::new_attached(
            &vanilla_entities::GLOW_ITEM_FRAME,
            1,
            BlockPos::new(12, 80, 14),
            Direction::West,
            Weak::new(),
        );

        assert!(frame.is_pickable());
    }

    #[test]
    fn analog_output_uses_item_presence_and_rotation() {
        let frame = GlowItemFrameEntity::new_attached(
            &vanilla_entities::GLOW_ITEM_FRAME,
            1,
            BlockPos::new(12, 80, 14),
            Direction::West,
            Weak::new(),
        );
        assert_eq!(frame.analog_output(), 0);

        frame.set_item_with_update(ItemStack::new(&vanilla_items::ELYTRA), false);
        assert_eq!(frame.analog_output(), 1);
        frame.entity_data.lock().item_frame.rotation.set(7);
        assert_eq!(frame.analog_output(), 8);
    }

    #[test]
    fn rotation_wraps_like_vanilla() {
        let frame = GlowItemFrameEntity::new_attached(
            &vanilla_entities::GLOW_ITEM_FRAME,
            1,
            BlockPos::new(12, 80, 14),
            Direction::West,
            Weak::new(),
        );

        frame.set_rotation(9);
        assert_eq!(frame.rotation(), 1);
        frame.set_rotation(-1);
        assert_eq!(frame.rotation(), 7);
    }

    #[test]
    fn item_frame_round_trips_drop_chance_and_fixed() {
        let frame = GlowItemFrameEntity::new_attached(
            &vanilla_entities::GLOW_ITEM_FRAME,
            1,
            BlockPos::new(12, 80, 14),
            Direction::West,
            Weak::new(),
        );
        frame.set_item(ItemStack::new(&vanilla_items::ELYTRA));
        frame.set_rotation(3);
        *frame.drop_chance.lock() = 0.25;
        *frame.fixed.lock() = true;

        let mut nbt = NbtCompound::new();
        frame.save_additional(&mut nbt);
        assert_eq!(nbt.float("ItemDropChance"), Some(0.25));
        assert_eq!(nbt.byte("Fixed"), Some(1));

        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = std::io::Cursor::new(bytes.as_slice());
        let borrowed = simdnbt::borrow::read_compound(&mut cursor)
            .expect("saved glow item frame data should reborrow");
        let view: BorrowedNbtCompoundView<'_, '_> = (&borrowed).into();

        let loaded = GlowItemFrameEntity::new_attached(
            &vanilla_entities::GLOW_ITEM_FRAME,
            1,
            BlockPos::new(12, 80, 14),
            Direction::West,
            Weak::new(),
        );
        loaded.load_additional(view);
        assert_eq!(loaded.rotation(), 3);
        assert_eq!(loaded.drop_chance(), 0.25);
        assert!(loaded.is_fixed());
        assert!(loaded.item().is(&vanilla_items::ELYTRA));
    }
}

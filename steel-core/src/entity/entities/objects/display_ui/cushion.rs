//! Vanilla cushion entity: a dyeable, sittable block-attached seat.
//!
//! Vanilla reference: `Cushion` (`world/entity/decoration/Cushion.java`) and
//! its placement rules in `CushionItem` (`world/item/CushionItem.java`).
//!
//! Steel's entity layer supplies the vehicle plumbing vanilla uses, so the
//! sit/stand interaction is implemented for real:
//! [`Entity::start_riding`](crate::entity::Entity::start_riding) puts the player
//! on the cushion and `interact` plays `ENTITY_CUSHION_SIT`.
//!
//! Reported gaps:
//! - Vanilla `Cushion.removePassenger` plays `ENTITY_CUSHION_GET_UP`. Steel's
//!   [`Entity`](crate::entity::Entity) trait has no passenger-removal hook, so
//!   the get-up sound has no call site.
//! - Vanilla `Cushion.thunderHit` kills and drops the cushion with a
//!   `LIGHTNING_DROP_INVULNERABLE_TICKS` (20) invulnerable item entity. Steel
//!   has no `thunderHit` hook and no entity-side item invulnerability setter,
//!   so lightning neither destroys nor drops cushions yet.
//! - Vanilla `getPickResult` has no Steel counterpart (middle-click only).
//! - `CushionItem.useOn` placement lives in the item-behavior lane; this file
//!   exposes [`CushionEntity::can_be_placed_at`] and
//!   [`CushionEntity::would_survive_at`] so that behavior can stay faithful.
//!   The `CUSHION_USES_COLLISION_SHAPE` raycast epsilon
//!   (`CushionItem.COLLISION_SHAPE_RAYCAST_EPSILON` = 0.001) belongs to that
//!   lane's hit-result recalculation.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::dye_color::DyeColor;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::{BlockParticleOption, ParticleData};
use steel_registry::vanilla_entity_data::CushionEntityData;
use steel_registry::vanilla_game_rules::ENTITY_DROPS;
use steel_registry::{self, sound_events, vanilla_block_tags, vanilla_particle_types};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey, WorldAabb};

use crate::behavior::InteractionResult;
use crate::entity::block_attached_entity::{BlockAttachedEntity, BlockAttachedEntityBase};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityBaseState, EntityMoveError, EntitySyncedData,
    RemovalReason, SharedEntity,
};
use crate::physics::{MoveResult, MoverType, WorldCollisionProvider, has_block_collision};
use crate::player::Player;
use crate::world::World;

/// Vanilla `Cushion.DEFAULT_COLOR`.
const DEFAULT_COLOR: DyeColor = DyeColor::White;
/// Vanilla `AABB.nextDeflated()` epsilon.
const NEXT_DEFLATED_EPSILON: f64 = 1.0e-7;
/// Vanilla anchor-box height used by `hasAnchorBelow`.
const ANCHOR_SLICE_HEIGHT: f64 = 0.015_625;
/// Vanilla anchor search depth below the cushion.
const ANCHOR_SEARCH_DEPTH: f64 = 0.125;

/// Vanilla cushion entity.
#[entity_behavior(class = "Cushion")]
pub struct CushionEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<CushionEntityData>,
    block_attached_entity_base: BlockAttachedEntityBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CushionEntity`.
unsafe impl DowncastType for CushionEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/cushion");
}

impl CushionEntity {
    /// Creates a fresh cushion from the generic entity factory path.
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
            world,
        )
    }

    /// Creates a fresh cushion attached to `block_pos`.
    #[must_use]
    pub fn new_attached(
        entity_type: EntityTypeRef,
        id: i32,
        block_pos: BlockPos,
        world: Weak<World>,
    ) -> Self {
        let box_ = Self::cushion_bounding_box(entity_type, block_pos);
        let center = box_.center();
        Self {
            base: EntityBase::new_with_state(
                id,
                EntityBaseState::new_with_bounding_box(center, entity_type.dimensions, box_),
                world,
            ),
            entity_type,
            entity_data: SyncMutex::new(CushionEntityData::new()),
            block_attached_entity_base: BlockAttachedEntityBase::new(block_pos),
        }
    }

    /// Creates a cushion from persistent entity data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        let position = load.position;
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(CushionEntityData::new()),
            block_attached_entity_base: BlockAttachedEntityBase::new(BlockPos::new(
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            )),
        }
    }

    /// Returns the cushion's dye color.
    #[must_use]
    pub fn color(&self) -> DyeColor {
        *self.entity_data.lock().color.get()
    }

    /// Sets the cushion's dye color.
    pub fn set_color(&self, color: DyeColor) {
        self.entity_data.lock().color.set(color);
    }

    /// Vanilla `Cushion.canBePlacedAt`: anchored and not buried.
    #[must_use]
    pub fn can_be_placed_at(world: &Arc<World>, bounding_box: WorldAabb) -> bool {
        Self::would_survive_at(world, bounding_box) && !Self::is_anchor_buried(world, bounding_box)
    }

    /// Vanilla `Cushion.wouldSurviveAt`: has an anchor below, not suffocated.
    #[must_use]
    pub fn would_survive_at(world: &Arc<World>, bounding_box: WorldAabb) -> bool {
        Self::has_anchor_below(world, bounding_box)
            && !Self::is_covered_by_suffocating_blocks(world, bounding_box)
    }

    /// Vanilla `Cushion.hasAnchorBelow`: a collision shape under the cushion.
    fn has_anchor_below(world: &Arc<World>, bounding_box: WorldAabb) -> bool {
        let anchor_box = WorldAabb::new(
            bounding_box.min_x(),
            bounding_box.min_y() - ANCHOR_SLICE_HEIGHT,
            bounding_box.min_z(),
            next_down(bounding_box.max_x()),
            bounding_box.min_y(),
            next_down(bounding_box.max_z()),
        );
        let search = anchor_box.translate(DVec3::new(0.0, -ANCHOR_SEARCH_DEPTH, 0.0));
        let provider = WorldCollisionProvider::new(world);
        has_block_collision(&provider, search) || has_block_collision(&provider, anchor_box)
    }

    /// Vanilla `Cushion.isAnchorBuried`: the resting slice must stay exposed.
    fn is_anchor_buried(world: &Arc<World>, bounding_box: WorldAabb) -> bool {
        let resting_slice = WorldAabb::new(
            bounding_box.min_x(),
            bounding_box.min_y(),
            bounding_box.min_z(),
            bounding_box.max_x(),
            bounding_box.min_y() + ANCHOR_SLICE_HEIGHT,
            bounding_box.max_z(),
        )
        .deflate(NEXT_DEFLATED_EPSILON);
        // Vanilla subtracts each block collider from the exposed surface and
        // reports "buried" when nothing remains. Steel's collision query
        // answers "is there any collider in this slice", which is exactly the
        // `exposedSurface.isEmpty()` condition.
        let provider = WorldCollisionProvider::new(world);
        has_block_collision(&provider, resting_slice)
    }

    /// Vanilla `Cushion.isCoveredBySuffocatingBlocks`.
    fn is_covered_by_suffocating_blocks(world: &Arc<World>, bounding_box: WorldAabb) -> bool {
        let deflated = bounding_box.deflate(NEXT_DEFLATED_EPSILON);
        let min = BlockPos::containing(deflated.min_x(), deflated.min_y(), deflated.min_z());
        let max = BlockPos::containing(deflated.max_x(), deflated.max_y(), deflated.max_z());
        BlockPos::between_closed(min, max).all(|pos| world.get_block_state(pos).is_suffocating())
    }

    /// Vanilla `Cushion.getCushionItemStackWithData`.
    fn cushion_item_stack_with_data(&self) -> ItemStack {
        let mut stack = ItemStack::new(cushion_item(self.color()));
        if let Some(custom_name) = self.custom_name() {
            stack.set(CUSTOM_NAME, custom_name);
        }
        stack
    }

    /// Vanilla `Cushion.showBreakingParticles`: 10 colored-wool block particles.
    fn show_breaking_particles(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let position = self.position();
        let dimensions = self.base.dimensions();
        world.send_particles(
            ParticleData::new(
                &vanilla_particle_types::BLOCK,
                BlockParticleOption::new(wool_block(self.color()).default_state()),
            ),
            DVec3::new(
                position.x,
                position.y + f64::from(dimensions.height) * 2.0 / 3.0,
                position.z,
            ),
            10,
            DVec3::new(
                f64::from(dimensions.width) / 4.0,
                f64::from(dimensions.height) / 4.0,
                f64::from(dimensions.width) / 4.0,
            ),
            0.05,
        );
    }

    /// Vanilla `Cushion.destroyIfInFire`: first fire block inside hurts the cushion.
    pub fn destroy_if_in_fire(&self) {
        if self.is_removed() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let box_ = self.bounding_box().deflate(NEXT_DEFLATED_EPSILON);
        let min = BlockPos::containing(box_.min_x(), box_.min_y(), box_.min_z());
        let max = BlockPos::containing(box_.max_x(), box_.max_y(), box_.max_z());
        let in_fire = BlockPos::between_closed(min, max).any(|pos| {
            world
                .get_block_state(pos)
                .get_block()
                .has_tag(&vanilla_block_tags::BlockTag::FIRE)
        });
        if in_fire {
            self.hurt(
                world.as_ref(),
                &DamageSource::environment(&steel_registry::vanilla_damage_types::IN_FIRE),
                1.0,
            );
        }
    }

    /// Vanilla `Cushion.setPos`: snapping keeps the anchor block in sync.
    fn snap_to_anchor(&self, position: DVec3) -> Result<(), EntityMoveError> {
        self.block_attached_entity_base()
            .set_pos(BlockPos::containing(position.x, position.y, position.z));
        self.try_set_position_block_attached_entity(position)
    }

    /// Bounding box for a cushion anchored at `block_pos`.
    fn cushion_bounding_box(entity_type: EntityTypeRef, block_pos: BlockPos) -> WorldAabb {
        WorldAabb::entity_box(
            f64::from(block_pos.x()) + 0.5,
            f64::from(block_pos.y()),
            f64::from(block_pos.z()) + 0.5,
            f64::from(entity_type.dimensions.half_width()),
            f64::from(entity_type.dimensions.height),
        )
    }
}

/// Java `Math.nextDown` for `f64`, used by vanilla's `AABB.nextDeflated()`.
///
/// In IEEE-754 bit order, positive magnitudes count up from zero and negative
/// magnitudes count down, so moving one ulp toward negative infinity moves
/// *away* from zero above zero and *toward* zero below zero.
fn next_down(value: f64) -> f64 {
    if value.is_nan() || value == f64::NEG_INFINITY {
        value
    } else if value == 0.0 {
        -f64::from_bits(1)
    } else if value > 0.0 {
        f64::from_bits(value.to_bits() - 1)
    } else {
        f64::from_bits(value.to_bits() - 1)
    }
}

/// Registry lookup for the cushion item matching `color`.
fn cushion_item(color: DyeColor) -> &'static steel_registry::items::Item {
    use steel_registry::vanilla_items;
    match color {
        DyeColor::White => &vanilla_items::WHITE_CUSHION,
        DyeColor::Orange => &vanilla_items::ORANGE_CUSHION,
        DyeColor::Magenta => &vanilla_items::MAGENTA_CUSHION,
        DyeColor::LightBlue => &vanilla_items::LIGHT_BLUE_CUSHION,
        DyeColor::Yellow => &vanilla_items::YELLOW_CUSHION,
        DyeColor::Lime => &vanilla_items::LIME_CUSHION,
        DyeColor::Pink => &vanilla_items::PINK_CUSHION,
        DyeColor::Gray => &vanilla_items::GRAY_CUSHION,
        DyeColor::LightGray => &vanilla_items::LIGHT_GRAY_CUSHION,
        DyeColor::Cyan => &vanilla_items::CYAN_CUSHION,
        DyeColor::Purple => &vanilla_items::PURPLE_CUSHION,
        DyeColor::Blue => &vanilla_items::BLUE_CUSHION,
        DyeColor::Brown => &vanilla_items::BROWN_CUSHION,
        DyeColor::Green => &vanilla_items::GREEN_CUSHION,
        DyeColor::Red => &vanilla_items::RED_CUSHION,
        DyeColor::Black => &vanilla_items::BLACK_CUSHION,
    }
}

/// Registry lookup for the wool block matching `color` (breaking particles).
fn wool_block(color: DyeColor) -> &'static steel_registry::blocks::Block {
    use steel_registry::vanilla_blocks;
    match color {
        DyeColor::White => &vanilla_blocks::WHITE_WOOL,
        DyeColor::Orange => &vanilla_blocks::ORANGE_WOOL,
        DyeColor::Magenta => &vanilla_blocks::MAGENTA_WOOL,
        DyeColor::LightBlue => &vanilla_blocks::LIGHT_BLUE_WOOL,
        DyeColor::Yellow => &vanilla_blocks::YELLOW_WOOL,
        DyeColor::Lime => &vanilla_blocks::LIME_WOOL,
        DyeColor::Pink => &vanilla_blocks::PINK_WOOL,
        DyeColor::Gray => &vanilla_blocks::GRAY_WOOL,
        DyeColor::LightGray => &vanilla_blocks::LIGHT_GRAY_WOOL,
        DyeColor::Cyan => &vanilla_blocks::CYAN_WOOL,
        DyeColor::Purple => &vanilla_blocks::PURPLE_WOOL,
        DyeColor::Blue => &vanilla_blocks::BLUE_WOOL,
        DyeColor::Brown => &vanilla_blocks::BROWN_WOOL,
        DyeColor::Green => &vanilla_blocks::GREEN_WOOL,
        DyeColor::Red => &vanilla_blocks::RED_WOOL,
        DyeColor::Black => &vanilla_blocks::BLACK_WOOL,
    }
}

impl Entity for CushionEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn tick(self: Arc<Self>) {
        // Vanilla `BlockAttachedEntity.tick` runs `tickAtCheckInterval` (fire
        // destruction) before the survival check, every 100 ticks.
        self.check_below_world();
        if self.block_attached_entity_base().should_check_survival() {
            self.destroy_if_in_fire();
            if !self.is_removed() && !self.survives() {
                self.set_removed(RemovalReason::Discarded);
                self.drop_item(None);
            }
        }
    }

    fn interact(
        &self,
        player: &Player,
        _hand: InteractionHand,
        _location: DVec3,
    ) -> InteractionResult {
        if player.is_secondary_use_active() || self.is_vehicle() {
            return InteractionResult::Pass;
        }

        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };
        let Some(cushion) = world.get_entity_by_id(self.id()) else {
            return InteractionResult::Pass;
        };
        if player.start_riding(&cushion) {
            self.play_sound(&sound_events::ENTITY_CUSHION_SIT, 1.0, 1.0);
            return InteractionResult::SuccessServer;
        }
        InteractionResult::Consume
    }

    fn is_pickable(&self) -> bool {
        self.is_pickable_block_attached_entity()
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        // Vanilla `Cushion.hurtServer`: players without build permission, or
        // outside their interaction bounds, cannot break the cushion.
        if let Some(player) = source
            .causing_entity()
            .and_then(|entity| entity.as_player())
            && (!player.abilities.lock().may_build
                || !world.may_interact(player, self.block_attached_entity_base.pos()))
        {
            return false;
        }
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
        self.snap_to_anchor(pos)
    }

    fn snap_to(&self, position: DVec3, _yaw: f32, _pitch: f32) {
        if let Err(error) = self.snap_to_anchor(position) {
            log::warn!(
                "failed to commit cushion {} snap position: {error}",
                self.base.id()
            );
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_block_attached_entity(nbt);
        nbt.insert("color", self.color().id() as i8);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_block_attached_entity(nbt);
        self.set_color(
            nbt.byte("color")
                .map_or(DEFAULT_COLOR, |value| DyeColor::by_id(i32::from(value))),
        );
    }
}

impl BlockAttachedEntity for CushionEntity {
    fn block_attached_entity_base(&self) -> &BlockAttachedEntityBase {
        &self.block_attached_entity_base
    }

    fn survives(&self) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        Self::would_survive_at(&world, self.bounding_box())
    }

    fn drop_item(&self, caused_by: Option<&dyn Entity>) {
        let Some(world) = self.level() else {
            return;
        };
        self.play_sound(&sound_events::ENTITY_CUSHION_BREAK, 1.0, 1.0);
        self.show_breaking_particles();
        if !world.get_game_rule(&ENTITY_DROPS) {
            return;
        }
        let infinite = caused_by
            .and_then(|entity| entity.as_player())
            .is_some_and(|player| player.has_infinite_materials());
        if infinite {
            return;
        }
        self.spawn_at_location(self.cushion_item_stack_with_data(), 0.0);
    }

    fn recalculate_bounding_box(&self) -> Result<(), EntityMoveError> {
        let block_pos = self.block_attached_entity_base.pos();
        let box_ = Self::cushion_bounding_box(self.entity_type, block_pos);
        self.base.set_bounding_box(box_);
        self.base.mark_velocity_sync();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::vanilla_entities;

    /// Re-borrows owned NBT the way entity loading does from disk.
    fn with_reborrow<R>(
        nbt: &simdnbt::owned::NbtCompound,
        visitor: impl FnOnce(simdnbt::borrow::NbtCompound<'_, '_>) -> R,
    ) -> R {
        use simdnbt::borrow::read_compound;
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = std::io::Cursor::new(bytes.as_slice());
        let base = read_compound(&mut cursor).expect("entity NBT should reborrow");
        let borrowed: simdnbt::borrow::NbtCompound<'_, '_> = (&base).into();
        visitor(borrowed)
    }

    fn new_cushion() -> CushionEntity {
        CushionEntity::new_attached(
            &vanilla_entities::CUSHION,
            1,
            BlockPos::new(3, 70, -8),
            Weak::new(),
        )
    }

    #[test]
    fn default_color_and_anchor_match_vanilla() {
        let cushion = new_cushion();
        assert_eq!(cushion.color(), DEFAULT_COLOR);
        assert_eq!(
            cushion.block_attached_entity_base().pos(),
            BlockPos::new(3, 70, -8)
        );
        let expected = CushionEntity::cushion_bounding_box(
            &vanilla_entities::CUSHION,
            BlockPos::new(3, 70, -8),
        );
        assert_eq!(cushion.bounding_box(), expected);
    }

    #[test]
    fn color_round_trips_through_nbt() {
        let cushion = new_cushion();
        cushion.set_color(DyeColor::LightBlue);
        let mut nbt = NbtCompound::new();
        cushion.save_additional(&mut nbt);
        assert_eq!(nbt.byte("color"), Some(DyeColor::LightBlue.id() as i8));

        let reloaded = new_cushion();
        with_reborrow(&nbt, |borrowed| reloaded.load_additional(borrowed));
        assert_eq!(reloaded.color(), DyeColor::LightBlue);
    }

    #[test]
    fn cushion_items_are_registry_entries_per_color() {
        for color in DyeColor::VALUES {
            let item = cushion_item(color);
            assert!(
                item.key.path.contains("cushion"),
                "expected a cushion item for {color:?}, got {}",
                item.key
            );
        }
    }

    #[test]
    fn next_down_matches_java_math() {
        assert!(next_down(1.0) < 1.0);
        assert_eq!(next_down(0.0), -f64::from_bits(1));
        assert!(next_down(-1.0) > -1.0);
        assert_eq!(next_down(f64::NAN).is_nan(), true);
        assert_eq!(next_down(f64::NEG_INFINITY), f64::NEG_INFINITY);
    }
}

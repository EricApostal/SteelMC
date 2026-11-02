//! Conversion from ABI descriptors into registry entries.

use std::slice;

use steel_abi::{
    STEEL_PROPERTY_BOOL, STEEL_PROPERTY_ENUM, STEEL_PROPERTY_INT, STEEL_PUSH_BLOCK,
    STEEL_PUSH_DESTROY, STEEL_PUSH_IGNORE, STEEL_PUSH_POPPED, STEEL_REF_BLOCK, SteelAabb,
    SteelBlockDesc, SteelBlockEntityDesc, SteelEntityDesc, SteelItemDesc, SteelLight,
    SteelPropertyDesc, SteelShapeTable, SteelStrView,
};
use steel_registry::block_entity_type::BlockEntityType;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::behavior::{BlockConfig, PushReaction};
use steel_registry::blocks::properties::{BoolProperty, IntProperty, Property, StringEnumProperty};
use steel_registry::blocks::shapes::VoxelShape;
use steel_registry::blocks::{Block, BlockLightProperties, LightSource, ShapeSource};
use steel_registry::data_components::vanilla_components::MAX_STACK_SIZE;
use steel_registry::entity_type::{EntityDimensions, EntityFlags, EntityType, MobCategory};
use steel_registry::items::Item;
use steel_registry::{Registry, RegistryExt};
use steel_utils::{BlockLocalAabb, Identifier};
use text_components::TextComponent;

/// Parses `namespace:path`.
pub fn identifier(view: SteelStrView) -> Result<Identifier, String> {
    // SAFETY: the view is supplied by the extension for the duration of the call.
    let value =
        unsafe { view.as_str() }.ok_or_else(|| "identifier is not valid UTF-8".to_owned())?;
    let Some((namespace, path)) = value.split_once(':') else {
        return Err(format!("identifier `{value}` is missing a namespace"));
    };
    if namespace.is_empty() || path.is_empty() {
        return Err(format!("identifier `{value}` is empty"));
    }
    Ok(Identifier::new(namespace.to_owned(), path.to_owned()))
}

/// Copies a borrowed string.
pub fn string(view: SteelStrView) -> Result<String, String> {
    // SAFETY: the view is supplied by the extension for the duration of the call.
    let value = unsafe { view.as_str() }.ok_or_else(|| "string is not valid UTF-8".to_owned())?;
    Ok(value.to_owned())
}

/// Borrows a caller-provided array for the duration of the call.
///
/// # Safety
///
/// `ptr` must point to `len` initialized values of `T` when `len > 0`.
pub unsafe fn array<'a, T>(ptr: *const T, len: usize) -> Result<&'a [T], String> {
    if len == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() {
        return Err("null array pointer with a non-zero length".to_owned());
    }
    // SAFETY: the caller guarantees the pointer and length.
    Ok(unsafe { slice::from_raw_parts(ptr, len) })
}

fn bool_at(aabb: &SteelAabb) -> BlockLocalAabb {
    BlockLocalAabb::new(
        f64::from(aabb.min_x),
        f64::from(aabb.min_y),
        f64::from(aabb.min_z),
        f64::from(aabb.max_x),
        f64::from(aabb.max_y),
        f64::from(aabb.max_z),
    )
}

fn leak_shape(boxes: &[SteelAabb]) -> VoxelShape {
    let boxes: Vec<BlockLocalAabb> = boxes.iter().map(bool_at).collect();
    let boxes: &'static [BlockLocalAabb] = Box::leak(boxes.into_boxed_slice());
    VoxelShape::from_boxes(boxes)
}

fn build_shape(table: &SteelShapeTable, state_count: usize, fallback: VoxelShape) -> ShapeSource {
    if table.override_count == 0 && table.box_count == 0 {
        return ShapeSource::Table(Box::leak(vec![fallback; state_count].into_boxed_slice()));
    }

    let mut shapes = vec![fallback; state_count];
    if table.box_count > 0 {
        // SAFETY: the extension supplied the array for this call.
        let boxes = unsafe { array(table.boxes, table.box_count) }.unwrap_or(&[]);
        let default = leak_shape(boxes);
        shapes.fill(default);
    }
    // SAFETY: the extension supplied the array for this call.
    for entry in unsafe { array(table.overrides, table.override_count) }.unwrap_or(&[]) {
        // SAFETY: the extension supplied the array for this call.
        let boxes = unsafe { array(entry.boxes, entry.box_count) }.unwrap_or(&[]);
        if let Some(slot) = shapes.get_mut(entry.state_offset as usize) {
            *slot = leak_shape(boxes);
        }
    }
    ShapeSource::Table(Box::leak(shapes.into_boxed_slice()))
}

fn build_light(light: SteelLight, state_count: usize) -> LightSource {
    let properties = BlockLightProperties {
        light_emission: light.emission,
        light_dampening: light.dampening,
        use_shape_for_light_occlusion: light.use_shape_for_occlusion != 0,
    };
    LightSource::Table(Box::leak(vec![properties; state_count].into_boxed_slice()))
}

fn leak_property(desc: &SteelPropertyDesc) -> Result<&'static dyn Property, String> {
    let name = string(desc.name)?;
    let name: &'static str = Box::leak(name.into_boxed_str());
    match desc.kind {
        STEEL_PROPERTY_BOOL => Ok(Box::leak(Box::new(BoolProperty::new(name)))),
        STEEL_PROPERTY_INT => {
            let min = u8::try_from(desc.min).map_err(|_| "int property minimum out of range")?;
            let max = u8::try_from(desc.max).map_err(|_| "int property maximum out of range")?;
            if min > max {
                return Err("int property minimum exceeds maximum".to_owned());
            }
            Ok(Box::leak(Box::new(IntProperty::new(name, min, max))))
        }
        STEEL_PROPERTY_ENUM => {
            // SAFETY: the extension supplied the array for this call.
            let values = unsafe { array(desc.values, desc.value_count) }?;
            let mut leaked = Vec::with_capacity(values.len());
            for value in values {
                let value = string(*value)?;
                leaked.push(Box::leak(value.into_boxed_str()) as &'static str);
            }
            if leaked.is_empty() {
                return Err("enum property has no values".to_owned());
            }
            let values: &'static [&'static str] = Box::leak(leaked.into_boxed_slice());
            Ok(Box::leak(Box::new(StringEnumProperty::new(
                name, values, values,
            ))))
        }
        other => Err(format!("unknown property kind {other}")),
    }
}

fn push_reaction(value: u32) -> PushReaction {
    match value {
        STEEL_PUSH_DESTROY => PushReaction::Popped,
        STEEL_PUSH_BLOCK => PushReaction::Push,
        STEEL_PUSH_IGNORE => PushReaction::IgnoreEntity,
        STEEL_PUSH_POPPED => PushReaction::Popped,
        _ => PushReaction::PushPull,
    }
}

/// Converts and registers a block descriptor.
pub fn register_block(desc: &SteelBlockDesc, registry: &mut Registry) -> Result<BlockRef, String> {
    let key = identifier(desc.key)?;
    if registry.blocks.by_key(&key).is_some() {
        return Err(format!("block `{key}` is already registered"));
    }

    // SAFETY: the extension supplied the property array for this call.
    let property_descs = unsafe { array(desc.properties, desc.property_count) }?;
    let mut properties = Vec::with_capacity(property_descs.len());
    for property in property_descs {
        properties.push(leak_property(property)?);
    }
    let properties: &'static [&'static dyn Property] = Box::leak(properties.into_boxed_slice());

    let mut state_count: usize = 1;
    for property in properties {
        state_count = state_count.saturating_mul(property.value_count());
        if state_count > usize::from(u16::MAX) {
            return Err("block property set exceeds the 16-bit state id space".to_owned());
        }
    }
    let state_count = state_count.max(1);
    if state_count > registry.blocks.available_state_ids() {
        return Err("the global block state id space is exhausted".to_owned());
    }

    let mut config = BlockConfig::new();
    config.has_collision = desc.flags & steel_abi::STEEL_BLOCK_HAS_COLLISION != 0;
    config.can_occlude = desc.flags & steel_abi::STEEL_BLOCK_CAN_OCCLUDE != 0;
    config.is_air = desc.flags & steel_abi::STEEL_BLOCK_IS_AIR != 0;
    config.replaceable = desc.flags & steel_abi::STEEL_BLOCK_REPLACEABLE != 0;
    config.is_randomly_ticking =
        desc.flags & steel_abi::STEEL_BLOCK_RANDOMLY_TICKING != 0 || desc.randomly_ticking != 0;
    config.liquid = desc.flags & steel_abi::STEEL_BLOCK_LIQUID != 0;
    config.requires_correct_tool_for_drops =
        desc.flags & steel_abi::STEEL_BLOCK_REQUIRES_CORRECT_TOOL != 0;
    config.force_solid_off = desc.flags & steel_abi::STEEL_BLOCK_FORCE_SOLID_OFF != 0;
    config.force_solid_on = desc.flags & steel_abi::STEEL_BLOCK_FORCE_SOLID_ON != 0;
    config.ignited_by_lava = desc.flags & steel_abi::STEEL_BLOCK_IGNITED_BY_LAVA != 0;
    config.push_reaction = push_reaction(desc.push_reaction);
    config.destroy_time = desc.destroy_time;
    config.explosion_resistance = desc.explosion_resistance;
    config.friction = desc.friction;
    config.speed_factor = desc.speed_factor;
    config.jump_factor = desc.jump_factor;
    config.bounce_restitution = desc.bounce_restitution;
    config.fall_distance_reduction = desc.fall_distance_reduction;
    config.max_horizontal_offset = desc.max_horizontal_offset;
    config.max_vertical_offset = desc.max_vertical_offset;

    let collision = build_shape(&desc.collision, state_count, VoxelShape::FULL_BLOCK);
    let support = build_shape(&desc.support, state_count, VoxelShape::FULL_BLOCK);
    let outline = build_shape(&desc.outline, state_count, VoxelShape::FULL_BLOCK);
    let occlusion = build_shape(&desc.occlusion, state_count, VoxelShape::FULL_BLOCK);
    let interaction = build_shape(&desc.interaction, state_count, VoxelShape::EMPTY);
    let visual = build_shape(&desc.visual, state_count, VoxelShape::FULL_BLOCK);

    let mut block = Block::new(key, config, properties)
        .with_shape_sources(collision, support, outline, occlusion, interaction, visual)
        .with_light_source(build_light(desc.light, state_count))
        .with_suffocating(steel_registry::blocks::StateBooleanData::new(
            desc.suffocating != 0,
            &[],
        ))
        .with_redstone_conductor(steel_registry::blocks::StateBooleanData::new(
            desc.redstone_conductor != 0,
            &[],
        ));
    block.default_state_offset = u16::try_from(desc.default_state_offset).unwrap_or(0);
    let block: &'static Block = Box::leak(Box::new(block));
    registry.blocks.register(block);
    Ok(block)
}

/// Converts and registers an item descriptor.
pub fn register_item(
    desc: &SteelItemDesc,
    registry: &mut Registry,
) -> Result<steel_registry::items::ItemRef, String> {
    let key = identifier(desc.key)?;
    if registry.items.by_key(&key).is_some() {
        return Err(format!("item `{key}` is already registered"));
    }

    let block = if desc.block.kind == STEEL_REF_BLOCK {
        Some(
            registry
                .blocks
                .by_id(desc.block.index as usize)
                .ok_or_else(|| "item block reference is not registered".to_owned())?,
        )
    } else {
        None
    };

    let name = TextComponent::plain(key.to_string());
    let mut item = Item::new(key, name, None);
    item.block = block;
    item.components
        .set(MAX_STACK_SIZE, Some(desc.max_stack_size.clamp(1, 99)));
    let item: &'static Item = Box::leak(Box::new(item));
    registry.items.register(item);
    Ok(item)
}

/// Converts and registers a block-entity type descriptor.
pub fn register_block_entity_type(
    desc: &SteelBlockEntityDesc,
    registry: &mut Registry,
) -> Result<(), String> {
    let key = identifier(desc.key)?;
    if registry.block_entity_types.by_key(&key).is_some() {
        return Err(format!("block entity type `{key}` is already registered"));
    }
    // SAFETY: the extension supplied the array for this call.
    let refs = unsafe { array(desc.valid_blocks, desc.valid_block_count) }?;
    let mut blocks = Vec::with_capacity(refs.len());
    for reference in refs {
        let Some(block) = registry.blocks.by_id(reference.index as usize) else {
            return Err("block entity type references an unregistered block".to_owned());
        };
        blocks.push(block);
    }
    let valid_blocks: &'static [BlockRef] = Box::leak(blocks.into_boxed_slice());
    let kind = BlockEntityType { key, valid_blocks };
    let kind: &'static BlockEntityType = Box::leak(Box::new(kind));
    registry.block_entity_types.register(kind);
    Ok(())
}

fn mob_category(value: u32) -> MobCategory {
    match value {
        steel_abi::STEEL_MOB_CATEGORY_MONSTER => MobCategory::Monster,
        steel_abi::STEEL_MOB_CATEGORY_CREATURE => MobCategory::Creature,
        steel_abi::STEEL_MOB_CATEGORY_AMBIENT => MobCategory::Ambient,
        steel_abi::STEEL_MOB_CATEGORY_AXOLOTLS => MobCategory::Axolotls,
        steel_abi::STEEL_MOB_CATEGORY_UNDERGROUND_WATER_CREATURE => {
            MobCategory::UndergroundWaterCreature
        }
        steel_abi::STEEL_MOB_CATEGORY_WATER_CREATURE => MobCategory::WaterCreature,
        steel_abi::STEEL_MOB_CATEGORY_WATER_AMBIENT => MobCategory::WaterAmbient,
        _ => MobCategory::Misc,
    }
}

/// Converts and registers an entity type descriptor.
pub fn register_entity_type(desc: &SteelEntityDesc, registry: &mut Registry) -> Result<(), String> {
    let key = identifier(desc.key)?;
    if registry.entity_types.by_key(&key).is_some() {
        return Err(format!("entity type `{key}` is already registered"));
    }
    let flags = EntityFlags {
        is_pushable: true,
        is_attackable: true,
        is_pickable: true,
        can_be_collided_with: true,
        is_pushed_by_fluid: true,
        can_freeze: false,
        can_be_hit_by_projectile: true,
        is_sensitive_to_water: false,
        can_breathe_underwater: false,
        can_be_seen_as_enemy: false,
        piston_push_reaction: PushReaction::PushPull,
    };
    let entity_type = EntityType {
        key,
        client_tracking_range: desc.client_tracking_range,
        update_interval: desc.update_interval,
        track_deltas: desc.flags & steel_abi::STEEL_ENTITY_TRACK_DELTAS != 0,
        dimensions: EntityDimensions::new(desc.width, desc.height, desc.eye_height),
        fixed: false,
        mob_category: mob_category(desc.mob_category),
        fire_immune: desc.flags & steel_abi::STEEL_ENTITY_FIRE_IMMUNE != 0,
        summonable: desc.flags & steel_abi::STEEL_ENTITY_SUMMONABLE != 0,
        allowed_in_peaceful: desc.flags & steel_abi::STEEL_ENTITY_ALLOWED_IN_PEACEFUL != 0,
        can_spawn_far_from_player: desc.flags & steel_abi::STEEL_ENTITY_CAN_SPAWN_FAR_FROM_PLAYER
            != 0,
        can_serialize: desc.flags & steel_abi::STEEL_ENTITY_CAN_SERIALIZE != 0,
        only_op_can_set_nbt: false,
        is_abstract_boat: false,
        is_abstract_minecart: false,
        is_projectile: false,
        is_abstract_arrow: false,
        is_abstract_horse: false,
        is_abstract_nautilus: false,
        flags,
        default_attributes: &[],
    };
    let entity_type: &'static EntityType = Box::leak(Box::new(entity_type));
    registry.entity_types.register(entity_type);
    Ok(())
}

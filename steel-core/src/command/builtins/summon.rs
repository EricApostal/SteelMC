//! Entity summoning command.

use std::sync::Arc;

use glam::DVec3;
use simdnbt::owned::NbtCompound;
use steel_registry::entity_type::EntityTypeRef;
use steel_utils::{BlockPos, Identifier, translations, types::Difficulty};
use text_components::{TextComponent, translation::Translation};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::{
    entity::{AddEntityError, ENTITIES, EntitySpawnReason, SharedEntity, next_entity_id},
    world::World,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("summon"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("summon").then(
        argument("entity", SteelArgumentType::summonable_entity())
            .executes(|context| summon_entity(context, context.source().position(), None))
            .then(
                argument("pos", SteelArgumentType::vec3(true))
                    .executes(|context| {
                        let position = context.coordinates("pos")?;
                        summon_entity(context, position.position(context.source()), None)
                    })
                    .then(
                        // Vanilla nests `nbt` under `pos` and uses
                        // `CompoundTagArgument`, like `/data merge`.
                        argument("nbt", SteelArgumentType::nbt_compound()).executes(|context| {
                            let position = context.coordinates("pos")?;
                            let nbt = context.nbt_compound("nbt")?;
                            summon_entity(context, position.position(context.source()), Some(nbt))
                        }),
                    ),
            ),
    )
    // Vanilla `SummonCommand.createEntity` additionally loads the NBT's
    // `Passengers` (and riding chain) recursively through
    // `EntityType.loadEntityRecursive`, which Steel's owned-compound entity
    // loader does not support yet (owner: entity lane,
    // `entity/entity/mod.rs` + `entity/mod.rs`): a compound with `Passengers`
    // spawns the root entity but not its passengers.
}

fn summon_entity(
    context: &SteelCommandContext<CommandSource>,
    position: DVec3,
    nbt: Option<&NbtCompound>,
) -> Result<i32, CommandSyntaxError> {
    let entity_type = context.entity_type("entity")?;
    let entity = create_inner(context, entity_type, position, nbt)?;
    let message = translations::COMMANDS_SUMMON_SUCCESS
        .message([entity.display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(1)
}

/// Vanilla `SummonCommand`'s no-NBT branch (empty compound, `finalizeSpawn`
/// set), shared with `/execute ... summon`.
pub(super) fn create_entity(
    context: &SteelCommandContext<CommandSource>,
    entity_type: EntityTypeRef,
    position: DVec3,
) -> Result<SharedEntity, CommandSyntaxError> {
    create_inner(context, entity_type, position, None)
}

fn create_inner(
    context: &SteelCommandContext<CommandSource>,
    entity_type: EntityTypeRef,
    position: DVec3,
    nbt: Option<&NbtCompound>,
) -> Result<SharedEntity, CommandSyntaxError> {
    create_entity_in_world(context.source().world(), entity_type, position, nbt)
}

/// Vanilla `SummonCommand.createEntity`, minus argument parsing.
///
/// `nbt: None` is the command's empty-compound branch, which passes
/// `finalize = true`; supplying an NBT argument passes `finalize = false`.
fn create_entity_in_world(
    world: &Arc<World>,
    entity_type: EntityTypeRef,
    position: DVec3,
    nbt: Option<&NbtCompound>,
) -> Result<SharedEntity, CommandSyntaxError> {
    if !World::is_in_spawnable_bounds(BlockPos::from(position)) {
        return Err(command_failed(
            &translations::COMMANDS_SUMMON_INVALID_POSITION,
        ));
    }

    if world.difficulty() == Difficulty::Peaceful && !entity_type.allowed_in_peaceful {
        return Err(command_failed(
            &translations::COMMANDS_SUMMON_FAILED_PEACEFUL,
        ));
    }

    let Some(entity) = ENTITIES.create(
        entity_type,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    ) else {
        return Err(command_failed(&translations::COMMANDS_SUMMON_FAILED));
    };

    if let Some(nbt) = nbt {
        // Vanilla `SummonCommand.createEntity` copies the compound, stamps the
        // argument's type id over any `id` inside, loads it into the entity and
        // snaps to the command position while keeping the loaded rotation.
        // Loading first and snapping after matches that exactly; `Entity.load`
        // covers the same save fields as `/data modify entity`'s `setData`.
        apply_command_nbt(&entity, position, nbt)?;
    } else if let Some(mob) = entity.as_mob() {
        // Vanilla `SummonCommand` sets `finalizeSpawn` only without NBT: the
        // `pos + nbt` executable passes `finalize = false` (bytecode
        // `iconst_0` after `CompoundTagArgument.getCompoundTag`), while the
        // `pos`-only and no-position executables pass `true`.
        let _ = mob.finalize_spawn(world, EntitySpawnReason::Command, None);
    }

    match world.try_add_entity(Arc::clone(&entity)) {
        Ok(()) => Ok(entity),
        Err(AddEntityError::DuplicateUuid { .. }) => {
            Err(command_failed(&translations::COMMANDS_SUMMON_FAILED_UUID))
        }
        Err(_) => Err(command_failed(&translations::COMMANDS_SUMMON_FAILED)),
    }
}

fn command_failed(translation: &'static Translation<0>) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(translation))
}

/// Vanilla `SummonCommand.createEntity`'s NBT step: load the compound into the
/// entity and then apply the processor's
/// `snapTo(pos.x, pos.y, pos.z, getYRot(), getXRot())`, so the command position
/// wins over the NBT's `Pos` while the NBT's `Rotation` is kept.
fn apply_command_nbt(
    entity: &SharedEntity,
    position: DVec3,
    nbt: &NbtCompound,
) -> Result<(), CommandSyntaxError> {
    entity.load_with_owned_components(nbt).map_err(|error| {
        CommandSyntaxError::dynamic(format!("Failed to apply NBT to entity: {error}"))
    })?;
    let (yaw, pitch) = entity.rotation();
    entity.snap_to(position, yaw, pitch);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::create_dispatcher;
    use super::*;
    use crate::bootstrap::init_globals;
    use crate::command::{
        brigadier::{CommandDispatcher, NodeId},
        execution::{CommandSource, SteelArgumentType, SteelCommandRuntime},
    };
    use crate::entity::entities::PigEntity;
    use crate::entity::{init_entities, next_entity_id};
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
    use simdnbt::owned::{NbtList, NbtTag};
    use steel_registry::equipment::EquipmentSlot;
    use steel_registry::vanilla_entities;
    use steel_utils::ChunkPos;
    use steel_utils::text::DisplayResolutor;
    use text_components::TextComponent;

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

    #[test]
    fn summon_graph_uses_typed_entity_and_deferred_position_arguments() {
        init_globals();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let root = child(&dispatcher, dispatcher.root(), "summon");
        let entity = child(&dispatcher, root, "entity");
        assert_eq!(
            dispatcher
                .node(entity)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::summonable_entity())
        );
        assert!(matches!(
            dispatcher.node(entity),
            Some(node) if node.is_executable()
        ));

        let position = child(&dispatcher, entity, "pos");
        assert_eq!(
            dispatcher
                .node(position)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::vec3(true))
        );
        assert!(matches!(
            dispatcher.node(position),
            Some(node) if node.is_executable()
        ));

        // Vanilla nests `nbt` (`CompoundTagArgument`, like `/data merge`) under
        // `pos`; it executes, and `pos` keeps the no-NBT branch.
        let nbt = child(&dispatcher, position, "nbt");
        assert_eq!(
            dispatcher.node(nbt).and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::nbt_compound())
        );
        assert!(matches!(
            dispatcher.node(nbt),
            Some(node) if node.is_executable()
        ));
        assert!(dispatcher.children(nbt).is_some_and(<[_]>::is_empty));
    }

    #[test]
    fn summon_with_nbt_position_skips_finalize_spawn_like_vanilla() {
        // Vanilla `SummonCommand`: the `pos`-only executable calls
        // `spawnEntity(..., new CompoundTag(), true)`, while the `pos + nbt`
        // executable calls `spawnEntity(..., getCompoundTag(c, "nbt"), false)`
        // (`SummonCommand.lambda$register$2` passes `iconst_0`). An empty
        // compound therefore does *not* finalize a mob.
        init_globals();
        // `init_globals` covers the registry and block behaviours; the entity
        // factory registry is separate, and without it `ENTITIES.create` is empty
        // and `/summon` fails with `commands.summon.failed`.
        init_entities();
        let fixture = fresh_test_world("summon_nbt_finalize");
        // `/summon` adds the entity through the world, which requires the target
        // chunk to be loaded.
        insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
        let position = DVec3::new(0.5, 64.0, 0.5);

        let finalized = create_entity_in_world(
            &fixture.world,
            &vanilla_entities::ILLUSIONER,
            position,
            None,
        )
        .expect("summon without NBT should succeed");
        let has_main_hand = |entity: &SharedEntity| {
            entity
                .as_living_entity()
                .is_some_and(|living| living.has_item_in_slot(EquipmentSlot::MainHand))
        };
        assert!(
            has_main_hand(&finalized),
            "`/summon illusioner <pos>` must run finalizeSpawn and equip the bow"
        );

        let with_nbt = create_entity_in_world(
            &fixture.world,
            &vanilla_entities::ILLUSIONER,
            position,
            Some(&NbtCompound::new()),
        )
        .expect("summon with an empty NBT compound should succeed");
        assert!(
            !has_main_hand(&with_nbt),
            "`/summon illusioner <pos> {{}}` must skip finalizeSpawn in vanilla 26.3"
        );
    }

    #[test]
    fn command_nbt_loads_before_snapping_to_the_command_position() {
        init_globals();
        let fixture = fresh_test_world("summon_command_nbt");
        let position = DVec3::new(10.5, 64.0, -3.25);
        let entity: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            next_entity_id(),
            DVec3::new(0.0, -64.0, 0.0),
            Arc::downgrade(&fixture.world),
        ));

        let mut nbt = NbtCompound::new();
        // Vanilla stamping: the compound may carry a `Pos` that the command
        // position must override.
        nbt.insert("Pos", NbtList::Double(vec![100.0, 100.0, 100.0]));
        nbt.insert("Rotation", NbtList::Float(vec![45.0, -10.0]));
        nbt.insert("NoGravity", NbtTag::Byte(1));
        nbt.insert("CustomName", TextComponent::plain("Bob").to_codec_nbt());

        apply_command_nbt(&entity, position, &nbt).expect("command NBT should apply");

        assert_eq!(entity.position(), position);
        assert_eq!(entity.rotation(), (45.0, -10.0));
        assert!(entity.is_no_gravity());
        assert_eq!(
            entity
                .custom_name()
                .map(|name| name.to_plain(&DisplayResolutor)),
            Some("Bob".to_owned())
        );
    }
}

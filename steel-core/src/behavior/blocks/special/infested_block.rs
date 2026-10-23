//! `InfestedBlock`/`InfestedRotatedPillarBlock` behaviors
//! (`net.minecraft.world.level.block.InfestedBlock`).

use std::sync::{Arc, OnceLock};

use glam::DVec3;
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{Axis, BlockStateProperties, EnumProperty};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_enchantment_tags::EnchantmentTag;
use steel_registry::vanilla_game_rules::BLOCK_DROPS;
use steel_registry::{REGISTRY, RegistryExt as _, TaggedRegistryExt as _, vanilla_entities};
use steel_utils::{BlockPos, BlockStateId, Identifier};

use crate::behavior::{BlockBehavior, BlockLootContext, BlockPlaceContext};
use crate::entity::{ENTITIES, EntitySpawnReason, SharedEntity, next_entity_id};
use crate::world::World;

const AXIS: &EnumProperty<Axis> = &BlockStateProperties::AXIS;

/// Returns whether the tool carries an enchantment tagged
/// `minecraft:prevents_infested_spawns` (vanilla silk touch).
fn prevents_infested_spawns(tool: &ItemStack) -> bool {
    let Some(enchantments) = tool.get_enchantments() else {
        return false;
    };
    enchantments.iter().any(|(key, _)| {
        REGISTRY.enchantments.by_key(key).is_some_and(|ench| {
            REGISTRY
                .enchantments
                .is_in_tag(ench, &EnchantmentTag::PREVENTS_INFESTED_SPAWNS)
        })
    })
}

/// Spawns the vanilla silverfish ambush for an infested block.
///
/// TODO(deps): the entity lane has no `Silverfish` factory yet, so this
/// currently returns `false` and spawns nothing. The spawn path is complete and
/// will work as soon as `vanilla_entities::SILVERFISH` is registered.
fn spawn_infestation(world: &Arc<World>, pos: BlockPos) -> bool {
    if !ENTITIES.has_factory(&vanilla_entities::SILVERFISH) {
        return false;
    }

    let position = DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()),
        f64::from(pos.z()) + 0.5,
    );
    let Some(entity): Option<SharedEntity> = ENTITIES.create(
        &vanilla_entities::SILVERFISH,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    ) else {
        return false;
    };
    if let Some(mob) = entity.as_mob() {
        let _ = mob.finalize_spawn(world, EntitySpawnReason::Triggered, None);
    }
    world.try_add_entity(entity).is_ok()
}

/// `InfestedBlock` behavior.
#[block_behavior]
pub struct InfestedBlock {
    block: BlockRef,
    /// Vanilla host block; Steel's extracted loot/state data already encodes
    /// the host mapping, so the field is only kept for parity with the
    /// extracted constructor arguments.
    #[expect(dead_code, reason = "host mapping is fully encoded in extracted data")]
    #[json_arg(vanilla_blocks, json = "host_block")]
    host_block: BlockRef,
}

impl InfestedBlock {
    /// Creates an infested block behavior.
    #[must_use]
    pub const fn new(block: BlockRef, host_block: BlockRef) -> Self {
        Self { block, host_block }
    }

    /// Vanilla `InfestedBlock.isCompatibleHostBlock`: the state belongs to one of the
    /// seven regular host blocks (stone/cobblestone/stone-bricks family).
    ///
    /// Derived from the registry-wide InfestedBlock host↔infested map in the generated
    /// `register_block_behaviors` table (regular hosts map to `InfestedBlock` blocks,
    /// `deepslate` maps to `InfestedRotatedPillarBlock`).
    #[must_use]
    pub fn is_compatible_host_block(state: BlockStateId) -> bool {
        Self::infested_state_by_host(state).is_some()
    }

    /// Vanilla `InfestedBlock.getStateByHostBlock`: the infested block's default state
    /// for the given host state, if the host has an infested counterpart.
    #[must_use]
    pub fn infested_state_by_host(host_state: BlockStateId) -> Option<BlockStateId> {
        let host = REGISTRY.blocks.by_state_id(host_state)?;
        let infested = infested_block_for_host(host)?;
        Some(infested.default_state())
    }

    /// Vanilla `InfestedBlock.getHostBlockStateByInfested`: the host block's default
    /// state for the given infested state.
    #[must_use]
    pub fn host_state_by_infested(infested_state: BlockStateId) -> Option<BlockStateId> {
        let infested = REGISTRY.blocks.by_state_id(infested_state)?;
        let host = host_block_for_infested(infested)?;
        Some(host.default_state())
    }

    /// Vanilla `InfestedBlock.isInfested`: whether the state belongs to an infested block.
    #[must_use]
    pub fn is_infested_block_state(state: BlockStateId) -> bool {
        let Some(block) = REGISTRY.blocks.by_state_id(state) else {
            return false;
        };
        host_block_for_infested(block).is_some()
    }
}

/// Registry-wide host↔infested map shared by both infested block classes.
///
/// Built from the extracted `classes.json` table (the same source the generated
/// `register_block_behaviors` uses): every block whose class is `InfestedBlock` or
/// `InfestedRotatedPillarBlock` contributes its `host_block` constructor argument, so
/// neither direction of the map can drift from the registry data.
fn infested_mappings() -> &'static InfestedMappings {
    static MAPPINGS: OnceLock<InfestedMappings> = OnceLock::new();
    MAPPINGS.get_or_init(|| {
        use steel_registry::RegistryExt as _;

        let classes: serde_json::Value =
            serde_json::from_str(include_str!("../../../../build/classes.json"))
                .expect("extracted classes.json is valid JSON");
        let mut mappings = InfestedMappings::default();
        for block in classes["blocks"]
            .as_array()
            .expect("classes.json contains blocks")
        {
            let Some(class) = block["class"].as_str() else {
                continue;
            };
            if class != "InfestedBlock" && class != "InfestedRotatedPillarBlock" {
                continue;
            }
            let (Some(name), Some(host_name)) =
                (block["name"].as_str(), block["host_block"].as_str())
            else {
                continue;
            };
            let (Some(infested), Some(host)) = (
                REGISTRY
                    .blocks
                    .by_key(&Identifier::vanilla(name.to_owned())),
                REGISTRY
                    .blocks
                    .by_key(&Identifier::vanilla(host_name.to_owned())),
            ) else {
                continue;
            };
            mappings.host_to_infested.push((host, infested));
            mappings.infested_to_host.push((infested, host));
        }
        mappings
    })
}

/// Both directions of the extracted host↔infested table.
#[derive(Default)]
struct InfestedMappings {
    host_to_infested: Vec<(BlockRef, BlockRef)>,
    infested_to_host: Vec<(BlockRef, BlockRef)>,
}

/// Finds the infested block registered for a host block.
fn infested_block_for_host(host: BlockRef) -> Option<BlockRef> {
    infested_mappings()
        .host_to_infested
        .iter()
        .find_map(|&(candidate, infested)| (candidate == host).then_some(infested))
}

/// Finds the host block registered for an infested block.
fn host_block_for_infested(infested: BlockRef) -> Option<BlockRef> {
    infested_mappings()
        .infested_to_host
        .iter()
        .find_map(|&(candidate, host)| (candidate == infested).then_some(host))
}

impl BlockBehavior for InfestedBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn get_drops(
        &self,
        _state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        // Vanilla infested blocks drop nothing without silk touch; the
        // extracted loot tables already encode that, so Steel keeps the
        // override empty instead of inventing drops.
        Some(Vec::new())
    }

    fn spawn_after_break(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        tool: &ItemStack,
        _drop_experience: bool,
    ) {
        // Vanilla `InfestedBlock.spawnAfterBreak`.
        if world.get_game_rule(&BLOCK_DROPS) && !prevents_infested_spawns(tool) {
            let _ = spawn_infestation(world, pos);
        }
    }
}

/// `InfestedRotatedPillarBlock` behavior (axis-oriented infested variant).
#[block_behavior]
pub struct InfestedRotatedPillarBlock {
    block: BlockRef,
    /// See [`InfestedBlock::host_block`].
    #[expect(dead_code, reason = "host mapping is fully encoded in extracted data")]
    #[json_arg(vanilla_blocks, json = "host_block")]
    host_block: BlockRef,
}

impl InfestedRotatedPillarBlock {
    /// Creates an infested pillar block behavior.
    #[must_use]
    pub const fn new(block: BlockRef, host_block: BlockRef) -> Self {
        Self { block, host_block }
    }
}

impl BlockBehavior for InfestedRotatedPillarBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(AXIS, context.clicked_face().axis()),
        )
    }

    fn get_drops(
        &self,
        _state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        Some(Vec::new())
    }

    fn spawn_after_break(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        tool: &ItemStack,
        drop_experience: bool,
    ) {
        InfestedBlock {
            block: self.block,
            host_block: self.host_block,
        }
        .spawn_after_break(state, world, pos, tool, drop_experience);
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};

    use super::InfestedBlock;

    /// The extracted host↔infested table resolves in both directions and rejects
    /// blocks outside it (vanilla `InfestedBlock.isCompatibleHostBlock`).
    #[test]
    fn host_infested_map_round_trips() {
        init_vanilla_registry();

        let pairs = [
            (&vanilla_blocks::STONE, &vanilla_blocks::INFESTED_STONE),
            (
                &vanilla_blocks::COBBLESTONE,
                &vanilla_blocks::INFESTED_COBBLESTONE,
            ),
            (
                &vanilla_blocks::STONE_BRICKS,
                &vanilla_blocks::INFESTED_STONE_BRICKS,
            ),
            (
                &vanilla_blocks::MOSSY_STONE_BRICKS,
                &vanilla_blocks::INFESTED_MOSSY_STONE_BRICKS,
            ),
            (
                &vanilla_blocks::CRACKED_STONE_BRICKS,
                &vanilla_blocks::INFESTED_CRACKED_STONE_BRICKS,
            ),
            (
                &vanilla_blocks::CHISELED_STONE_BRICKS,
                &vanilla_blocks::INFESTED_CHISELED_STONE_BRICKS,
            ),
            (
                &vanilla_blocks::DEEPSLATE,
                &vanilla_blocks::INFESTED_DEEPSLATE,
            ),
        ];

        for (host, infested) in pairs {
            let host_state = host.default_state();
            let infested_state = infested.default_state();

            assert!(InfestedBlock::is_compatible_host_block(host_state));
            assert_eq!(
                InfestedBlock::infested_state_by_host(host_state),
                Some(infested_state)
            );
            assert!(InfestedBlock::is_infested_block_state(infested_state));
            assert_eq!(
                InfestedBlock::host_state_by_infested(infested_state),
                Some(host_state)
            );
        }

        assert!(!InfestedBlock::is_compatible_host_block(
            vanilla_blocks::DIRT.default_state()
        ));
        assert!(!InfestedBlock::is_infested_block_state(
            vanilla_blocks::STONE.default_state()
        ));
    }
}

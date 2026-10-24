//! Advancement trigger raises for behavior-level call sites.
//!
//! The generic plumbing lives in [`crate::player::advancement::triggers`]
//! (`player_view`, `event_base`, `raise`); these helpers mirror the exact vanilla
//! raise sites (`CriteriaTriggers.*.trigger(...)`), verified against the vanilla
//! jar, that live outside the player module: item behaviors in
//! [`crate::behavior::items`], the brushable and beacon block entities, and the
//! golem build in [`crate::world::village`].
//!
//! Every helper takes the caller's [`World`] instead of reading it back from the
//! player, and fires for every nearby [`Player`] vanilla notifies — not only the
//! interacting one.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::advancement::triggers::TriggerEvent;
use steel_registry::item_stack::ItemStack;
use steel_utils::{BlockPos, Identifier, WorldAabb};

use crate::entity::{Entity, living_loot_equipment, loot_equipment};
use crate::player::Player;
use crate::player::advancement::triggers::{
    WorldLevelView, entity_view, event_base, player_view, raise,
};
use crate::world::World;

/// Raises `minecraft:consume_item` (`CriteriaTriggers.CONSUME_ITEM`).
///
/// Vanilla `Consumable.onConsume` fires this for the consuming `ServerPlayer`
/// together with the `minecraft:used` item stat, before running the consume
/// effects.
pub fn raise_consume_item(player: &Player, world: &Arc<World>, item: &ItemStack) -> usize {
    let origin = player.block_position();
    let level = WorldLevelView::new(world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("consume_item"),
        &TriggerEvent::ConsumeItem {
            base: event_base(&view, &level, origin),
            item,
        },
    )
}

/// Raises `minecraft:item_used_on_block` (`CriteriaTriggers.ITEM_USED_ON_BLOCK`).
///
/// The vanilla item-level call sites (`ShearsItem`, `HoneycombItem`,
/// `BlockTransformer`) fire this with the clicked block position and the used
/// stack, before the block itself changes.
///
/// The central raise is a separate vanilla site: `ServerPlayerGameMode.useItemOn`
/// fires it when the *block state's* own `useItemOn` consumes the action (for
/// signs, campfires, lodestones, ...). Steel's equivalent
/// (`player/game_mode/item_interaction.rs`) still lacks it; that file belongs to
/// the game-mode lane.
pub fn raise_item_used_on_block(
    player: &Player,
    world: &Arc<World>,
    pos: BlockPos,
    tool: &ItemStack,
) -> usize {
    let level = WorldLevelView::new(world, pos);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("item_used_on_block"),
        &TriggerEvent::ItemUsedOnBlock {
            base: event_base(&view, &level, pos),
            tool,
        },
    )
}

/// Raises `minecraft:used_ender_eye` (`CriteriaTriggers.USED_ENDER_EYE`).
///
/// Vanilla `EnderEyeItem.use` fires this with the squared horizontal distance
/// between the player and the located stronghold.
pub fn raise_used_ender_eye(player: &Player, world: &Arc<World>, distance_sqr: f64) -> usize {
    let origin = player.block_position();
    let level = WorldLevelView::new(world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("used_ender_eye"),
        &TriggerEvent::UsedEnderEye {
            base: event_base(&view, &level, origin),
            distance_sqr,
        },
    )
}

/// Raises `minecraft:player_generates_container_loot`
/// (`CriteriaTriggers.GENERATE_LOOT`).
///
/// Vanilla `BrushableBlockEntity.unpackLootTable` fires this with the block
/// entity's loot table key as soon as the table is unpacked, before the loot is
/// rolled.
pub fn raise_generate_loot(player: &Player, world: &Arc<World>, loot_table: &Identifier) -> usize {
    let origin = player.block_position();
    let level = WorldLevelView::new(world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("player_generates_container_loot"),
        &TriggerEvent::LootTable {
            base: event_base(&view, &level, origin),
            loot_table,
        },
    )
}

/// Raises `minecraft:construct_beacon` (`CriteriaTriggers.CONSTRUCT_BEACON`).
///
/// Vanilla `BeaconBlockEntity.tick` fires this exactly on the activation
/// transition — a completed pyramid scan taking the beacon from zero levels to a
/// positive level — for every `ServerPlayer` intersecting
/// `AABB(beacon, beacon.y - 4).inflate(10, 5, 10)`, with the beacon's new level.
pub fn raise_construct_beacon(world: &Arc<World>, pos: BlockPos, levels: i32) -> usize {
    let (x, y, z) = (f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
    let area = WorldAabb::from_min_max(
        DVec3::new(x - 10.0, y - 9.0, z - 10.0),
        DVec3::new(x + 10.0, y + 5.0, z + 10.0),
    );

    let mut granted = 0;
    for entity in world
        .get_entities_in_aabb_matching(&area, |entity| {
            entity.entity_type() == &steel_registry::vanilla_entities::PLAYER
        })
    {
        let Some(player) = entity.as_player() else {
            continue;
        };
        let level = WorldLevelView::new(world, pos);
        let equipment = living_loot_equipment(player);
        let known = player.recipe_book().unlocked_identifiers().to_vec();
        let view = player_view(player, &level, Some(&equipment), &known);
        granted += raise(
            player,
            &Identifier::vanilla_static("construct_beacon"),
            &TriggerEvent::ConstructBeacon {
                base: event_base(&view, &level, pos),
                levels,
            },
        );
    }
    granted
}

/// Raises `minecraft:summoned_entity` (`CriteriaTriggers.SUMMONED_ENTITY`).
///
/// Vanilla `CarvedPumpkinBlock.spawnGolemInWorld` fires this for every
/// `ServerPlayer` within 5 blocks of the golem's bounding box, after the golem
/// has joined the level.
pub fn raise_golem_summoned(world: &Arc<World>, summoned: &dyn Entity) -> usize {
    let origin = summoned.block_position();
    let area = summoned.bounding_box().inflate(5.0);
    let level = WorldLevelView::new(world, origin);
    let summoned_equipment = loot_equipment(summoned);
    let summoned_view = entity_view(summoned, &level, summoned_equipment.as_ref());

    let mut granted = 0;
    for entity in
        world.get_entities_in_aabb_matching(&area, |entity| entity.as_player().is_some())
    {
        let Some(player) = entity.as_player() else {
            continue;
        };
        let equipment = living_loot_equipment(player);
        let known = player.recipe_book().unlocked_identifiers().to_vec();
        let player_view = player_view(player, &level, Some(&equipment), &known);
        granted += raise(
            player,
            &Identifier::vanilla_static("summoned_entity"),
            &TriggerEvent::SummonedEntity {
                base: event_base(&player_view, &level, origin),
                entity: &summoned_view,
            },
        );
    }
    granted
}

/// Assertions shared by the advancement call-site tests.
#[cfg(test)]
pub(crate) mod test_support {
    use steel_registry::REGISTRY;
    use steel_registry::advancement::registry::AdvancementRef;
    use steel_utils::Identifier;

    use crate::player::Player;

    /// Whether `player` has completed `criterion` of the vanilla `advancement`.
    ///
    /// Panics when the advancement is not registered, so a typo cannot make a test
    /// pass silently.
    #[must_use]
    pub(crate) fn criterion_granted(
        player: &Player,
        advancement: &'static str,
        criterion: &str,
    ) -> bool {
        let advancement: AdvancementRef = REGISTRY
            .advancements
            .value_by_key(&Identifier::vanilla_static(advancement))
            .unwrap_or_else(|| panic!("`{advancement}` is not a registered advancement"));
        player.advancements.lock().progress.get_progress(advancement).is_some_and(
            |progress| progress.get_completed_criteria().any(|name| name == criterion),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::item_stack::ItemStack;
    use steel_registry::{REGISTRY, init_vanilla_registry, vanilla_blocks, vanilla_items};
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos, Identifier};

    use super::test_support::criterion_granted;
    use super::{raise_item_used_on_block, raise_used_ender_eye};
    use crate::player::Player;
    use crate::test_support::{TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk};

    const COPPER_POS: BlockPos = BlockPos::new(8, 64, 8);

    /// A world holding a copper block, and a player to raise the trigger for.
    fn fixture(name: &'static str) -> (TestWorld, Arc<Player>) {
        init_vanilla_registry();
        let world_fixture = fresh_test_world(name);
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(COPPER_POS));
        assert!(world.set_block(
            COPPER_POS,
            vanilla_blocks::COPPER_BLOCK.default_state(),
            UpdateFlags::UPDATE_ALL
        ));
        let player = TestPlayerBuilder::new(Arc::clone(world), "AdvancementHooks", 1).build();
        (world_fixture, player)
    }

    /// The `item_used_on_block` helper must build the event the vanilla criteria
    /// expect: `minecraft:husbandry/wax_on` listens for it with a copper-block
    /// location and a honeycomb tool.
    #[test]
    fn item_used_on_block_grants_wax_on_for_the_matching_block_and_tool() {
        let (world_fixture, player) = fixture("advancement_hooks_item_used_on_block");
        let world = &world_fixture.world;

        let honeycomb = ItemStack::new(&vanilla_items::HONEYCOMB);
        let granted = raise_item_used_on_block(&player, world, COPPER_POS, &honeycomb);
        assert!(granted >= 1, "honeycomb on copper matches `husbandry/wax_on`");
        assert!(criterion_granted(&player, "husbandry/wax_on", "wax_on"));

        // The criterion's `match_tool` term means the same block with another tool
        // must not grant another criterion.
        let shears = ItemStack::new(&vanilla_items::SHEARS);
        assert_eq!(
            raise_item_used_on_block(&player, world, COPPER_POS, &shears),
            0
        );
    }

    /// Vanilla registers no advancement on `used_ender_eye`, so the helper is verified
    /// structurally: it addresses the vanilla trigger and creates no progress while
    /// nothing listens for it.
    #[test]
    fn used_ender_eye_addresses_the_vanilla_trigger() {
        let (world_fixture, player) = fixture("advancement_hooks_used_ender_eye");
        let world = &world_fixture.world;
        let trigger = Identifier::vanilla_static("used_ender_eye");
        assert!(
            REGISTRY.advancements.criteria_for_trigger(&trigger).is_empty(),
            "vanilla ships no advancement on `used_ender_eye`"
        );

        assert_eq!(raise_used_ender_eye(&player, world, 25.0), 0);
        assert!(player.advancements.lock().progress.is_empty());
    }
}

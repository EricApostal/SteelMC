//! Advancement trigger raises for behavior-level call sites.
//!
//! The generic plumbing lives in [`crate::player::advancement::triggers`]
//! (`player_view`, `event_base`, `raise`); these helpers mirror the exact vanilla
//! raise sites (`CriteriaTriggers.*.trigger(...)`), verified against the vanilla
//! jar, that live outside the player module: item behaviors in
//! [`crate::behavior::items`], the brushable and beacon block entities, the
//! golem build in [`crate::world::village`], and the brewing stand bottle slot
//! in [`crate::inventory::menu::kinds`].
//!
//! Every helper takes the caller's [`World`] instead of reading it back from the
//! player, and fires for every nearby [`Player`] vanilla notifies — not only the
//! interacting one. The brewing potion raise reads it back from the taker instead:
//! vanilla notifies only the player taking the potion.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::advancement::predicates::EffectInstanceView;
use steel_registry::advancement::triggers::TriggerEvent;
use steel_registry::data_components::vanilla_components::PotionContents;
use steel_registry::item_stack::ItemStack;
use steel_utils::{BlockPos, Identifier, WorldAabb};

use crate::entity::{Entity, LivingEntity as _, living_loot_equipment, loot_equipment};
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

/// Raises `minecraft:brewed_potion` (`CriteriaTriggers.BREWED_POTION`).
///
/// Vanilla `BrewingStandMenu.PotionSlot.onTake` fires this with the potion
/// contents of the stack being taken out of a bottle slot; the criterion's
/// `potion` term (when present) names the brewed potion's identifier.
pub fn raise_brewed_potion(player: &Player, contents: &PotionContents) -> usize {
    let world = player.get_world();
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("brewed_potion"),
        &TriggerEvent::BrewedPotion {
            base: event_base(&view, &level, origin),
            potion: contents.potion().map(|potion| &potion.value().key),
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
/// signs, campfires, lodestones, ...). Steel's equivalent is
/// `player/game_mode/item_interaction.rs`, which raises it for every consuming
/// block-state interaction.
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

/// Raises `minecraft:effects_changed` (`CriteriaTriggers.EFFECTS_CHANGED`).
///
/// Vanilla `ServerPlayer.onEffectAdded`/`onEffectUpdated`/`onEffectsRemoved` fire
/// this with the player's effects after the change. The source entity is absent
/// because Steel's effect API does not carry one.
pub fn raise_effects_changed(player: &Player) -> usize {
    let world = player.get_world();
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    // Snapshot the effects before taking the equipment (inventory) lock.
    let effects: Vec<_> = player
        .active_mob_effects()
        .into_iter()
        .map(|effect| EffectInstanceView {
            effect: &effect.effect().key,
            amplifier: effect.amplifier(),
            duration: effect.duration(),
            ambient: effect.is_ambient(),
            visible: effect.is_visible(),
        })
        .collect();
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("effects_changed"),
        &TriggerEvent::EffectsChanged {
            base: event_base(&view, &level, origin),
            effects: &effects,
            source: None,
        },
    )
}

/// Raises `minecraft:player_interacted_with_entity`
/// (`CriteriaTriggers.PLAYER_INTERACTED_WITH_ENTITY`).
///
/// Vanilla `ServerGamePacketListenerImpl.handleInteract` fires this for any
/// consuming `Success`, with the pre-interaction hand stack when the result
/// `wasItemInteraction()` and an empty stack otherwise; `BottleItem.useItemOn`
/// fires it for the dragon's-breath bottle.
pub fn raise_player_interacted_with_entity(
    player: &Player,
    entity: &dyn Entity,
    item: &ItemStack,
) -> usize {
    let origin = entity.block_position();
    let Some(world) = entity.level() else {
        return 0;
    };
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let entity_equipment = loot_equipment(entity);
    let entity_view = entity_view(entity, &level, entity_equipment.as_ref());
    raise(
        player,
        &Identifier::vanilla_static("player_interacted_with_entity"),
        &TriggerEvent::PlayerInteractedWithEntity {
            base: event_base(&player_entity, &level, origin),
            item,
            entity: &entity_view,
        },
    )
}

/// Raises `minecraft:enchanted_item` (`CriteriaTriggers.ENCHANTED_ITEM`).
///
/// Vanilla `EnchantmentMenu.clickMenuButton` fires this after `awardStat`
/// (ENCHANT_ITEM) with the enchanted stack and the experience levels the offer
/// cost (`buttonId + 1`).
pub fn raise_enchanted_item(player: &Player, item: &ItemStack, levels: i32) -> usize {
    let world = player.get_world();
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("enchanted_item"),
        &TriggerEvent::EnchantedItem {
            base: event_base(&view, &level, origin),
            item,
            levels,
        },
    )
}

/// Raises `minecraft:player_sheared_equipment`
/// (`CriteriaTriggers.PLAYER_SHEARED_EQUIPMENT`).
///
/// Vanilla `Mob.shearItem` fires this with the shears in hand and the mob being
/// sheared, after the sheared equipment has been dropped.
pub fn raise_player_sheared_equipment(
    player: &Player,
    entity: &dyn Entity,
    item: &ItemStack,
) -> usize {
    let origin = entity.block_position();
    let Some(world) = entity.level() else {
        return 0;
    };
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let entity_equipment = loot_equipment(entity);
    let entity_view = entity_view(entity, &level, entity_equipment.as_ref());
    raise(
        player,
        &Identifier::vanilla_static("player_sheared_equipment"),
        &TriggerEvent::PlayerShearedEquipment {
            base: event_base(&player_entity, &level, origin),
            item,
            entity: &entity_view,
        },
    )
}

/// Raises `minecraft:placed_block` (`CriteriaTriggers.PLACED_BLOCK`).
///
/// Vanilla `BlockItem.place` fires this after `setPlacedBy` for the placed
/// position and the placed stack; `BucketItem.use` fires it for a filled bucket
/// that emptied its fluid into the world.
pub fn raise_placed_block(
    player: &Player,
    world: &Arc<World>,
    pos: BlockPos,
    item: &ItemStack,
) -> usize {
    let level = WorldLevelView::new(world, pos);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("placed_block"),
        &TriggerEvent::PlacedBlock {
            base: event_base(&view, &level, pos),
            tool: item,
        },
    )
}

/// Raises `minecraft:filled_bucket` (`CriteriaTriggers.FILLED_BUCKET`).
///
/// Vanilla `BucketItem.use` fires this the moment a source fluid is picked up,
/// with the resulting filled bucket stack; `Bucketable.bucketMobPickup` fires it
/// for the bucket a mob is scooped into.
pub fn raise_filled_bucket(player: &Player, world: &Arc<World>, item: &ItemStack) -> usize {
    let origin = player.block_position();
    let level = WorldLevelView::new(world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("filled_bucket"),
        &TriggerEvent::FilledBucket {
            base: event_base(&view, &level, origin),
            item,
        },
    )
}

/// Raises `minecraft:fishing_rod_hooked` (`CriteriaTriggers.FISHING_ROD_HOOKED`).
///
/// Vanilla `FishingHook.retrieve` fires this twice: once with the hooked entity
/// and an empty item list when the hook had caught an entity, and once with the
/// fished loot and the hook itself standing in for the `entity` context when the
/// hook had caught fish. `hooked` is the hooked entity for the first form and
/// `None` for the second, in which case the hook is used as the entity.
pub fn raise_fishing_rod_hooked(
    player: &Player,
    world: &Arc<World>,
    rod: &ItemStack,
    hook: &dyn Entity,
    hooked: Option<&dyn Entity>,
    items: &[ItemStack],
) -> usize {
    let origin = hook.block_position();
    let level = WorldLevelView::new(world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let hook_equipment = loot_equipment(hook);
    let hook_view = entity_view(hook, &level, hook_equipment.as_ref());
    let hooked_equipment = hooked.and_then(loot_equipment);
    let hooked_view = hooked
        .map(|hooked| entity_view(hooked, &level, hooked_equipment.as_ref()))
        .unwrap_or(hook_view);
    raise(
        player,
        &Identifier::vanilla_static("fishing_rod_hooked"),
        &TriggerEvent::FishingRodHooked {
            base: event_base(&player_entity, &level, origin),
            rod,
            hooked: Some(&hooked_view),
            items,
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
    for entity in world.get_entities_in_aabb_matching(&area, |entity| {
        entity.entity_type() == &steel_registry::vanilla_entities::PLAYER
    }) {
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
    for entity in world.get_entities_in_aabb_matching(&area, |entity| entity.as_player().is_some())
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

/// Raises `minecraft:shot_crossbow` (`CriteriaTriggers.SHOT_CROSSBOW`).
///
/// Vanilla `CrossbowItem.performShooting` fires this for the shooting
/// `ServerPlayer` with the crossbow stack, after the loaded projectiles have
/// been fired (and therefore after the stack has lost its
/// `charged_projectiles` component) and before the `used` stat is awarded. It is
/// the only `SHOT_CROSSBOW` raise site in the jar; a charge that never fires
/// does not raise it.
pub fn raise_shot_crossbow(player: &Player, world: &Arc<World>, item: &ItemStack) -> usize {
    let origin = player.block_position();
    let level = WorldLevelView::new(world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("shot_crossbow"),
        &TriggerEvent::ShotCrossbow {
            base: event_base(&view, &level, origin),
            item,
        },
    )
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
        player
            .advancements
            .lock()
            .progress
            .get_progress(advancement)
            .is_some_and(|progress| {
                progress
                    .get_completed_criteria()
                    .any(|name| name == criterion)
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::item_stack::ItemStack;
    use steel_registry::registry_reference::RegistryReference;
    use steel_registry::{
        REGISTRY, init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_game_rules,
        vanilla_items, vanilla_mob_effects, vanilla_potions,
    };
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos, Identifier};

    use super::test_support::criterion_granted;
    use super::{
        PotionContents, raise_brewed_potion, raise_enchanted_item, raise_filled_bucket,
        raise_fishing_rod_hooked, raise_item_used_on_block, raise_placed_block,
        raise_player_interacted_with_entity, raise_player_sheared_equipment, raise_used_ender_eye,
    };
    use crate::entity::MobEffectInstance;
    use crate::entity::{Entity as _, LivingEntity as _};
    use crate::player::Player;
    use crate::player::advancement::triggers::{
        WorldLevelView, raise_levitation, raise_started_riding,
    };
    use crate::test_support::{
        TestEntity, TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };

    const COPPER_POS: BlockPos = BlockPos::new(8, 64, 8);

    /// A world holding a copper block, and a player to raise the trigger for.
    fn fixture(name: &'static str) -> (TestWorld, Arc<Player>) {
        init_vanilla_registry();
        // `World::set_block` needs the block behavior registry; the other suites
        // only get it populated by whichever test happens to initialize it first.
        crate::behavior::init_behaviors();
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
        assert!(
            granted >= 1,
            "honeycomb on copper matches `husbandry/wax_on`"
        );
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
            REGISTRY
                .advancements
                .criteria_for_trigger(&trigger)
                .is_empty(),
            "vanilla ships no advancement on `used_ender_eye`"
        );

        assert_eq!(raise_used_ender_eye(&player, world, 25.0), 0);
        assert!(player.advancements.lock().progress.is_empty());
    }

    /// `nether/brew_potion` listens for `brewed_potion` with no potion term, so
    /// raising the helper with brewed potion contents completes it, mirroring a
    /// take from a brewing stand bottle slot.
    #[test]
    fn brewed_potion_grants_brew_potion() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("advancement_hooks_brewed_potion");
        let world = &world_fixture.world;
        // The test player has no server to announce the chat message to.
        assert!(world.set_game_rule(&vanilla_game_rules::SHOW_ADVANCEMENT_MESSAGES, false));
        let player = TestPlayerBuilder::new(Arc::clone(world), "BrewedPotion", 1).build();

        let contents = PotionContents::new(
            Some(RegistryReference::new(&vanilla_potions::AWKWARD)),
            None,
            Vec::new(),
            None,
        );
        assert!(raise_brewed_potion(&player, &contents) >= 1);
        assert!(criterion_granted(&player, "nether/brew_potion", "potion"));
    }

    /// `husbandry/ride_a_boat_with_a_goat` listens for `started_riding` with a
    /// `player.vehicle` term requiring a boat carrying a goat.
    ///
    /// IMPORTANT: the positive case cannot be asserted yet. `entity_view` leaves
    /// `EntityView.vehicle`/`passenger` as `None`, so the player view the trigger
    /// builds never exposes the ridden vehicle and the criterion can never match.
    /// Populating those needs recursively built child views (a real change to the
    /// view plumbing), so `started_riding` is reported as a raise-wired gap, not a
    /// landed trigger. This test pins the part that does hold today: mounting alone
    /// raises nothing until the trigger is raised, and the raise addresses the
    /// vanilla trigger.
    #[test]
    fn started_riding_addresses_the_vanilla_trigger() {
        let (_world, player) = fixture("advancement_hooks_started_riding");
        assert!(
            !REGISTRY
                .advancements
                .criteria_for_trigger(&Identifier::vanilla_static("started_riding"))
                .is_empty(),
            "vanilla ships `husbandry/ride_a_boat_with_a_goat` on `started_riding`"
        );
        // The view carries no `vehicle`, so nothing can match; the raise must not
        // fabricate progress either.
        assert_eq!(raise_started_riding(&player), 0);
    }

    /// `nether/all_potions` listens for `effects_changed` with an `effects` term
    /// requiring seventeen named effects. Adding them through the `LivingEntity`
    /// hook must raise the trigger after each change, and the criterion must only
    /// complete once the last one lands.
    #[test]
    fn effects_changed_grants_all_potions_for_the_full_effect_set() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("advancement_hooks_effects_changed");
        crate::behavior::init_behaviors();
        let player =
            TestPlayerBuilder::new(Arc::clone(&world_fixture.world), "EffectsChanged", 1).build();

        let all_effects = [
            vanilla_mob_effects::FIRE_RESISTANCE,
            vanilla_mob_effects::INFESTED,
            vanilla_mob_effects::INVISIBILITY,
            vanilla_mob_effects::JUMP_BOOST,
            vanilla_mob_effects::NIGHT_VISION,
            vanilla_mob_effects::OOZING,
            vanilla_mob_effects::POISON,
            vanilla_mob_effects::REGENERATION,
            vanilla_mob_effects::RESISTANCE,
            vanilla_mob_effects::SLOW_FALLING,
            vanilla_mob_effects::SLOWNESS,
            vanilla_mob_effects::SPEED,
            vanilla_mob_effects::STRENGTH,
            vanilla_mob_effects::WATER_BREATHING,
            vanilla_mob_effects::WEAKNESS,
            vanilla_mob_effects::WEAVING,
            vanilla_mob_effects::WIND_CHARGED,
        ];
        let (last, rest) = all_effects.split_last().expect("non-empty effect set");
        for effect in rest {
            player.add_mob_effect(MobEffectInstance::new(*effect, 0));
        }
        assert!(
            !criterion_granted(&player, "nether/all_potions", "all_effects"),
            "the criterion needs every listed effect"
        );

        player.add_mob_effect(MobEffectInstance::new(*last, 0));
        assert!(criterion_granted(
            &player,
            "nether/all_potions",
            "all_effects"
        ));
    }

    /// `end/levitate` listens for `levitation` with a `distance` term of at least
    /// 50 blocks of climb, measured from where levitation began.
    #[test]
    fn levitation_grants_levitate_for_a_long_climb() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("advancement_hooks_levitation");
        crate::behavior::init_behaviors();
        let player =
            TestPlayerBuilder::new(Arc::clone(&world_fixture.world), "Levitation", 1).build();
        let world = player.get_world();
        let origin = player.block_position();
        let level = WorldLevelView::new(&world, origin);
        let (x, y, z) = (
            f64::from(origin.x()),
            f64::from(origin.y()),
            f64::from(origin.z()),
        );

        assert_eq!(
            raise_levitation(&player, &level, glam::DVec3::new(x, y - 10.0, z), 20),
            0,
            "a short climb does not reach `end/levitate`"
        );
        assert!(raise_levitation(&player, &level, glam::DVec3::new(x, y - 60.0, z), 60) >= 1);
        assert!(criterion_granted(&player, "end/levitate", "levitated"));
    }

    /// `adventure/brush_armadillo` listens for `player_interacted_with_entity` with
    /// an `entity` term naming the armadillo and an `item` term naming the brush;
    /// the helper must expose both.
    #[test]
    fn player_interacted_with_entity_grants_brush_armadillo() {
        let (world_fixture, player) = fixture("advancement_hooks_interacted_with_entity");
        let world = &world_fixture.world;
        let armadillo = TestEntity::shared(
            11,
            glam::DVec3::new(8.0, 64.0, 8.0),
            Arc::downgrade(world),
            &vanilla_entities::ARMADILLO,
        );
        let brush = ItemStack::new(&vanilla_items::BRUSH);

        assert!(
            raise_player_interacted_with_entity(&player, armadillo.as_ref(), &brush) >= 1,
            "brushing an armadillo matches `husbandry/brush_armadillo`"
        );
        assert!(criterion_granted(
            &player,
            "adventure/brush_armadillo",
            "brush_armadillo"
        ));

        // The `entity` term pins the armadillo, so the same item on another mob
        // must not complete the criterion.
        let cow = TestEntity::shared(
            12,
            glam::DVec3::new(8.0, 64.0, 8.0),
            Arc::downgrade(world),
            &vanilla_entities::COW,
        );
        assert_eq!(
            raise_player_interacted_with_entity(&player, cow.as_ref(), &brush),
            0
        );
    }

    /// `story/enchant_item` listens for `enchanted_item` with no conditions, so any
    /// enchanted stack with a positive offer cost completes it.
    #[test]
    fn enchanted_item_grants_enchant_item() {
        let (_world, player) = fixture("advancement_hooks_enchanted_item");
        let sword = ItemStack::new(&vanilla_items::DIAMOND_SWORD);
        assert!(
            raise_enchanted_item(&player, &sword, 1) >= 1,
            "enchanting a sword matches `story/enchant_item`"
        );
        assert!(criterion_granted(
            &player,
            "story/enchant_item",
            "enchanted_item"
        ));
    }

    /// `husbandry/remove_wolf_armor` listens for `player_sheared_equipment` with an
    /// `entity` term naming the wolf and an `item` term naming the armor; both must
    /// reach the criterion from the event.
    #[test]
    fn player_sheared_equipment_grants_remove_wolf_armor() {
        let (world_fixture, player) = fixture("advancement_hooks_sheared_equipment");
        let world = &world_fixture.world;
        let wolf = TestEntity::shared(
            9,
            glam::DVec3::new(8.0, 64.0, 8.0),
            Arc::downgrade(world),
            &vanilla_entities::WOLF,
        );
        let armor = ItemStack::new(&vanilla_items::WOLF_ARMOR);

        assert!(
            raise_player_sheared_equipment(&player, wolf.as_ref(), &armor) >= 1,
            "shearing wolf armor matches `husbandry/remove_wolf_armor`"
        );
        assert!(criterion_granted(
            &player,
            "husbandry/remove_wolf_armor",
            "remove_wolf_armor"
        ));

        // The `entity` term pins the wolf, so shearing another mob's gear must not
        // complete the same criterion.
        let cow = TestEntity::shared(
            10,
            glam::DVec3::new(8.0, 64.0, 8.0),
            Arc::downgrade(world),
            &vanilla_entities::COW,
        );
        assert_eq!(
            raise_player_sheared_equipment(&player, cow.as_ref(), &armor),
            0
        );
    }

    /// `husbandry/plant_seed` listens for `placed_block` with a `location` term
    /// matching the planted crop block; the raise must expose the placed position
    /// so the location condition resolves the crop there.
    #[test]
    fn placed_block_grants_plant_seed_for_the_placed_crop() {
        let (world_fixture, player) = fixture("advancement_hooks_placed_block");
        let world = &world_fixture.world;
        assert!(world.set_block(
            COPPER_POS,
            vanilla_blocks::WHEAT.default_state(),
            UpdateFlags::UPDATE_ALL
        ));

        let seeds = ItemStack::new(&vanilla_items::WHEAT_SEEDS);
        assert!(
            raise_placed_block(&player, world, COPPER_POS, &seeds) >= 1,
            "a wheat block at the raise position matches `husbandry/plant_seed`"
        );
        assert!(criterion_granted(&player, "husbandry/plant_seed", "wheat"));
    }

    /// `husbandry/tactical_fishing` listens for `filled_bucket` with an `item`
    /// term naming the filled mob bucket; `Bucketable.bucketMobPickup` hands the
    /// captured-mob stack to the raise.
    #[test]
    fn filled_bucket_grants_tactical_fishing_for_a_mob_bucket() {
        let (world_fixture, player) = fixture("advancement_hooks_filled_bucket");
        let world = &world_fixture.world;

        let cod_bucket = ItemStack::new(&vanilla_items::COD_BUCKET);
        assert!(
            raise_filled_bucket(&player, world, &cod_bucket) >= 1,
            "a cod bucket matches `husbandry/tactical_fishing`"
        );
        assert!(criterion_granted(
            &player,
            "husbandry/tactical_fishing",
            "cod_bucket"
        ));

        // A plain water bucket is not a mob bucket, so it must not stand in.
        let water = ItemStack::new(&vanilla_items::WATER_BUCKET);
        assert_eq!(raise_filled_bucket(&player, world, &water), 0);
    }

    /// `husbandry/fishy_business` listens for `fishing_rod_hooked` with an `item`
    /// term naming the caught fish; the helper must expose the fished loot to the
    /// criterion and the hook as the entity context, as `FishingHook.retrieve`
    /// does for the loot form.
    #[test]
    fn fishing_rod_hooked_grants_fishy_business_for_the_caught_fish() {
        let (world_fixture, player) = fixture("advancement_hooks_fishing_rod_hooked");
        let world = &world_fixture.world;
        let hook = TestEntity::shared(
            7,
            glam::DVec3::new(8.0, 64.0, 8.0),
            Arc::downgrade(world),
            &vanilla_entities::FISHING_BOBBER,
        );
        let rod = ItemStack::new(&vanilla_items::FISHING_ROD);

        let cod = [ItemStack::new(&vanilla_items::COD)];
        assert!(raise_fishing_rod_hooked(&player, world, &rod, hook.as_ref(), None, &cod) >= 1);
        assert!(criterion_granted(
            &player,
            "husbandry/fishy_business",
            "cod"
        ));

        // The `item` term is per-fish, so a salmon must complete its own criterion
        // and must not stand in for the cod one.
        let salmon = [ItemStack::new(&vanilla_items::SALMON)];
        assert!(raise_fishing_rod_hooked(&player, world, &rod, hook.as_ref(), None, &salmon) >= 1);
        assert!(criterion_granted(
            &player,
            "husbandry/fishy_business",
            "salmon"
        ));
    }
}

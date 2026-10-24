//! Advancement trigger plumbing.
//!
//! Bridges Steel's world state to the registry's trigger framework: builds the
//! [`EntityView`]/[`LevelView`] values a criterion tests, raises a trigger for a
//! player, and grants every criterion that matches.
//!
//! Vanilla raises triggers from `ServerPlayer`/gameplay code and
//! `PlayerAdvancements.award` walks the listeners registered for that trigger;
//! this module is the Steel equivalent of both halves.

use std::sync::Arc;

use crate::entity::{Entity, entity_loot_ref, living_loot_equipment, loot_equipment};
use crate::player::Player;
use crate::world::{LevelReader as _, World};
use steel_registry::REGISTRY;
use steel_registry::advancement::predicates::{
    DamageView, EffectInstanceView, EntityComponentsPredicate, EntityView, EntityViewFlags,
    LevelView, PlayerView,
};
use steel_registry::advancement::triggers::{EventBase, TriggerEvent};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_utils::{BlockPos, BlockStateId, Identifier};

/// [`LevelView`] over a live [`World`] at one position.
pub struct WorldLevelView<'a> {
    world: &'a Arc<World>,
    position: BlockPos,
}

impl<'a> WorldLevelView<'a> {
    /// Creates a view of `world` at `position`.
    #[must_use]
    pub const fn new(world: &'a Arc<World>, position: BlockPos) -> Self {
        Self { world, position }
    }
}

impl LevelView for WorldLevelView<'_> {
    fn position(&self) -> (f64, f64, f64) {
        let pos = self.position;
        (f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()))
    }

    fn dimension(&self) -> &Identifier {
        &self.world.dimension_type.key
    }

    fn biome_at(&self, position: (f64, f64, f64)) -> Option<&Identifier> {
        self.world
            .biome_at(block_pos(position))
            .map(|biome| &biome.key)
    }

    fn structure_at(&self, _position: (f64, f64, f64)) -> Option<&Identifier> {
        // Steel keeps structure starts only while generating chunks; there is no
        // per-position structure lookup, so structure predicates cannot match yet
        // (`minecraft_trials_edition` needs one). Reported as a gap in the report.
        None
    }

    fn block_state_at(&self, position: (f64, f64, f64)) -> Option<BlockStateId> {
        Some(self.world.get_block_state(block_pos(position)))
    }

    fn fluid_at(&self, position: (f64, f64, f64)) -> Option<&Identifier> {
        let state = crate::fluid::state::get_fluid_state(self.world, block_pos(position));
        Some(&state.fluid_id.key)
    }

    fn light_at(&self, position: (f64, f64, f64)) -> i32 {
        i32::from(
            self.world
                .max_local_raw_brightness(block_pos(position), self.world.sky_darkening()),
        )
    }

    fn is_smokey_at(&self, position: (f64, f64, f64)) -> bool {
        let state = self.world.get_block_state(block_pos(position).below());
        state.get_block().key == Identifier::vanilla_static("campfire")
    }

    fn can_see_sky_at(&self, position: (f64, f64, f64)) -> bool {
        self.world.can_see_sky(block_pos(position))
    }
}

fn block_pos(position: (f64, f64, f64)) -> BlockPos {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "vanilla BlockPos.containing truncates the world position the same way"
    )]
    BlockPos::new(
        position.0.floor() as i32,
        position.1.floor() as i32,
        position.2.floor() as i32,
    )
}

/// Builds the [`EntityView`] for a live entity.
///
/// The returned view owns its effect list; it must outlive any event that refers
/// to it.
#[must_use]
pub fn entity_view<'a>(
    entity: &'a dyn Entity,
    level: &'a dyn LevelView,
    equipment: Option<&'a steel_registry::loot_table::EntityEquipmentSlots>,
) -> EntityView<'a> {
    let loot_ref = entity_loot_ref(entity, equipment);
    let effects = entity
        .as_living_entity()
        .map(|living| {
            living
                .active_mob_effects()
                .into_iter()
                .map(|effect| EffectInstanceView {
                    effect: &effect.effect().key,
                    amplifier: effect.amplifier(),
                    duration: effect.duration(),
                    ambient: effect.is_ambient(),
                    visible: effect.is_visible(),
                })
                .collect()
        })
        .unwrap_or_default();
    let position = entity.position();
    let is_flying = entity
        .as_player()
        .is_some_and(|player| player.abilities.lock().flying);
    EntityView {
        entity_type: loot_ref.entity_type,
        position: (position.x, position.y, position.z),
        level,
        stepping_on: None,
        flags: EntityViewFlags {
            is_on_ground: entity.on_ground(),
            is_on_fire: loot_ref.flags.is_on_fire,
            is_sneaking: loot_ref.flags.is_sneaking,
            is_sprinting: loot_ref.flags.is_sprinting,
            is_swimming: loot_ref.flags.is_swimming,
            is_flying,
            is_baby: loot_ref.flags.is_baby,
        },
        equipment,
        effects,
        components: EntityComponentsPredicate {
            cat_variant: None,
            frog_variant: None,
            wolf_variant: None,
            sheep_color: loot_ref.sheep_color,
            sheep_sheared: loot_ref.sheep_sheared,
            chicken_variant: loot_ref.chicken_variant.cloned(),
        },
        vehicle: None,
        passenger: None,
        player: None,
        lightning: None,
        held_item: loot_ref
            .equipment
            .map(|equipment| equipment.get(EquipmentSlot::MainHand)),
    }
}

/// Builds the [`EntityView`] for a player, including its `type_specific/player`
/// facts.
///
/// `unlocked_recipes` is passed in by the caller rather than read here: the recipe
/// book lives behind a mutex whose guard cannot outlive the call, and the view
/// borrows its slice for `'a`. Callers snapshot `ServerRecipeBook::known()`.
#[must_use]
pub fn player_view<'a>(
    player: &'a Player,
    level: &'a dyn LevelView,
    equipment: Option<&'a steel_registry::loot_table::EntityEquipmentSlots>,
    unlocked_recipes: &'a [Identifier],
) -> EntityView<'a> {
    let mut view = entity_view(player, level, equipment);
    view.player = Some(PlayerView {
        looking_at: None,
        unlocked_recipes,
    });
    view
}

/// Raises `trigger` for `player` with `event`, granting every matching criterion.
///
/// Returns the number of criteria that were newly granted.
pub fn raise(player: &Player, trigger: &Identifier, event: &TriggerEvent<'_>) -> usize {
    let criteria = REGISTRY.advancements.criteria_for_trigger(trigger);
    if criteria.is_empty() {
        return 0;
    }
    let mut granted = 0;
    {
        let mut guard = player.advancements.lock();
        for (advancement, name) in criteria {
            let Some(criterion) = advancement.get_criterion(name) else {
                continue;
            };
            if criterion.test(event) && guard.award(player, advancement, name) {
                granted += 1;
            }
        }
    }
    // Vanilla's `AdvancementRewards.grant` awards recipe rewards inline; Steel
    // defers them because the award path raises `recipe_unlocked`, which takes the
    // advancement lock again. The lock is released here, so apply them now.
    player.flush_pending_recipe_awards();
    granted
}

/// Builds an [`EventBase`] for `player` at `origin`.
#[must_use]
pub const fn event_base<'a>(
    player: &'a EntityView<'a>,
    level: &'a dyn LevelView,
    origin: BlockPos,
) -> EventBase<'a> {
    EventBase {
        player,
        level,
        origin,
    }
}

/// Raises the `tick` trigger for a player (`CriteriaTriggers.TICK`).
pub fn raise_tick(player: &Player, level: &dyn LevelView, origin: BlockPos) {
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("tick"),
        &TriggerEvent::Player(event_base(&view, level, origin)),
    );
}

/// Raises the `location` trigger for a player, with a view at its own position.
pub fn raise_location(player: &Player, world: &Arc<World>, origin: BlockPos) {
    let level = WorldLevelView::new(world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("location"),
        &TriggerEvent::Player(event_base(&view, &level, origin)),
    );
}

/// Raises `minecraft:villager_trade` (`CriteriaTriggers.TRADE`).
///
/// Vanilla `AbstractVillager.notifyTrade` fires this with the traded villager and
/// the offer's result stack.
pub fn raise_villager_trade(player: &Player, villager: &dyn Entity, result: &ItemStack) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = villager.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let base = event_base(&player_entity, &level, origin);
    let villager_equipment = loot_equipment(villager);
    let villager_view = entity_view(villager, &level, villager_equipment.as_ref());
    raise(
        player,
        &Identifier::vanilla_static("villager_trade"),
        &TriggerEvent::VillagerTrade {
            base,
            villager: &villager_view,
            item: result,
        },
    )
}

/// Raises `minecraft:cured_zombie_villager` (`CriteriaTriggers.CURED_ZOMBIE_VILLAGER`).
///
/// Vanilla `ZombieVillager.finishConversion` fires this with the curing player,
/// the zombie villager and the villager it became.
pub fn raise_cured_zombie_villager(
    player: &Player,
    zombie: &dyn Entity,
    villager: &dyn Entity,
) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = zombie.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let base = event_base(&player_entity, &level, origin);
    let zombie_equipment = loot_equipment(zombie);
    let zombie_view = entity_view(zombie, &level, zombie_equipment.as_ref());
    let villager_equipment = loot_equipment(villager);
    let villager_view = entity_view(villager, &level, villager_equipment.as_ref());
    raise(
        player,
        &Identifier::vanilla_static("cured_zombie_villager"),
        &TriggerEvent::CuredZombieVillager {
            base,
            zombie: &zombie_view,
            villager: &villager_view,
        },
    )
}

/// Raises `minecraft:voluntary_exile` (`CriteriaTriggers.RAID_OMEN`).
///
/// Vanilla `Raider.killedRaiderCaptain`-style path: the player killed a raid
/// captain carrying the ominous banner.
pub fn raise_voluntary_exile(player: &Player, level: &dyn LevelView, origin: BlockPos) -> usize {
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("voluntary_exile"),
        &TriggerEvent::Player(event_base(&view, level, origin)),
    )
}

/// Raises `minecraft:hero_of_the_village` (`CriteriaTriggers.RAID_WIN`).
pub fn raise_hero_of_the_village(
    player: &Player,
    level: &dyn LevelView,
    origin: BlockPos,
) -> usize {
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("hero_of_the_village"),
        &TriggerEvent::Player(event_base(&view, level, origin)),
    )
}

/// Builds a [`DamageView`] for a live damage source.
///
/// Vanilla's `KilledTrigger` only consults the source's predicate (damage type
/// tags and the source/direct entities), so the damage amounts are reported as
/// zero.
fn damage_view<'a>(
    source: &'a crate::entity::damage::DamageSource,
    source_view: &'a Option<EntityView<'a>>,
    direct_view: &'a Option<EntityView<'a>>,
) -> DamageView<'a> {
    DamageView {
        original: 0.0,
        actual: 0.0,
        blocked: false,
        source_entity: source_view.as_ref(),
        direct_entity: direct_view.as_ref(),
        type_key: Some(&source.damage_type.key),
        is_direct: source.is_direct(),
    }
}

/// Raises `minecraft:player_killed_entity` (`CriteriaTriggers.PLAYER_KILLED_ENTITY`).
///
/// Vanilla `ServerPlayer.awardKillScore` fires this with the killed entity and
/// the killing blow.
pub fn raise_player_killed_entity(
    player: &Player,
    victim: &dyn Entity,
    killing_blow: &crate::entity::damage::DamageSource,
) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = victim.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let base = event_base(&player_entity, &level, origin);
    let victim_equipment = loot_equipment(victim);
    let victim_view = entity_view(victim, &level, victim_equipment.as_ref());

    let source_equipment = killing_blow
        .causing_entity()
        .and_then(|entity| loot_equipment(&**entity));
    let source_view = killing_blow
        .causing_entity()
        .map(|entity| entity_view(&**entity, &level, source_equipment.as_ref()));
    let direct_equipment = killing_blow
        .direct_entity()
        .and_then(|entity| loot_equipment(&**entity));
    let direct_view = killing_blow
        .direct_entity()
        .map(|entity| entity_view(&**entity, &level, direct_equipment.as_ref()));

    let damage = damage_view(killing_blow, &source_view, &direct_view);
    raise(
        player,
        &Identifier::vanilla_static("player_killed_entity"),
        &TriggerEvent::PlayerKilledEntity {
            base,
            entity: &victim_view,
            killing_blow: Some(damage),
        },
    )
}

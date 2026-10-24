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
    // Cheap guard: skip the per-tick view snapshots when nothing listens.
    if REGISTRY
        .advancements
        .criteria_for_trigger(&Identifier::vanilla_static("tick"))
        .is_empty()
    {
        return;
    }
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
    // Cheap guard: skip the view snapshots when nothing listens.
    if REGISTRY
        .advancements
        .criteria_for_trigger(&Identifier::vanilla_static("location"))
        .is_empty()
    {
        return;
    }
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
/// `KilledTrigger` only consults the source's predicate (damage type tags and the
/// source/direct entities) and passes no amounts, so callers raising a kill use
/// zeroes. The hurt triggers (`entity_hurt_player`/`player_hurt_entity`) pass the
/// dealt (`original`) and taken (`actual`) amounts vanilla forwards.
fn damage_view<'a>(
    source: &'a crate::entity::damage::DamageSource,
    source_view: &'a Option<EntityView<'a>>,
    direct_view: &'a Option<EntityView<'a>>,
    original: f32,
    actual: f32,
    blocked: bool,
) -> DamageView<'a> {
    DamageView {
        original,
        actual,
        blocked,
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

    let damage = damage_view(killing_blow, &source_view, &direct_view, 0.0, 0.0, false);
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

/// Raises `minecraft:entity_killed_player` (`CriteriaTriggers.ENTITY_KILLED_PLAYER`).
///
/// Vanilla `Entity.awardKillScore` fires this when the victim is a
/// `ServerPlayer`, with that player as the trigger owner and the killer as the
/// event entity.
pub fn raise_entity_killed_player(
    player: &Player,
    killer: &dyn Entity,
    killing_blow: &crate::entity::damage::DamageSource,
) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let base = event_base(&player_entity, &level, origin);
    let killer_equipment = loot_equipment(killer);
    let killer_view = entity_view(killer, &level, killer_equipment.as_ref());

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

    let damage = damage_view(killing_blow, &source_view, &direct_view, 0.0, 0.0, false);
    raise(
        player,
        &Identifier::vanilla_static("entity_killed_player"),
        &TriggerEvent::EntityKilledPlayer {
            base,
            entity: &killer_view,
            killing_blow: Some(damage),
        },
    )
}

/// Raises `minecraft:entity_hurt_player` (`CriteriaTriggers.ENTITY_HURT_PLAYER`).
///
/// Vanilla `LivingEntity.hurtServer` fires this when the victim is a
/// `ServerPlayer` that took full damage: `original` is the pre-blocking amount,
/// `actual` the post-cooldown amount, and `blocked` whether any damage was
/// blocked by an item.
pub fn raise_entity_hurt_player(
    player: &Player,
    source: &crate::entity::damage::DamageSource,
    original: f32,
    actual: f32,
    blocked: bool,
) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let base = event_base(&player_entity, &level, origin);

    let source_equipment = source
        .causing_entity()
        .and_then(|entity| loot_equipment(&**entity));
    let source_view = source
        .causing_entity()
        .map(|entity| entity_view(&**entity, &level, source_equipment.as_ref()));
    let direct_equipment = source
        .direct_entity()
        .and_then(|entity| loot_equipment(&**entity));
    let direct_view = source
        .direct_entity()
        .map(|entity| entity_view(&**entity, &level, direct_equipment.as_ref()));

    let damage = damage_view(
        source,
        &source_view,
        &direct_view,
        original,
        actual,
        blocked,
    );
    raise(
        player,
        &Identifier::vanilla_static("entity_hurt_player"),
        &TriggerEvent::EntityHurtPlayer { base, damage },
    )
}

/// Raises `minecraft:player_hurt_entity` (`CriteriaTriggers.PLAYER_HURT_ENTITY`).
///
/// Vanilla `LivingEntity.hurtServer` fires this when the damage source's entity
/// is a `ServerPlayer` and the victim took full damage, with the same amount
/// semantics as [`raise_entity_hurt_player`].
pub fn raise_player_hurt_entity(
    player: &Player,
    victim: &dyn Entity,
    source: &crate::entity::damage::DamageSource,
    original: f32,
    actual: f32,
    blocked: bool,
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

    let source_equipment = source
        .causing_entity()
        .and_then(|entity| loot_equipment(&**entity));
    let source_view = source
        .causing_entity()
        .map(|entity| entity_view(&**entity, &level, source_equipment.as_ref()));
    let direct_equipment = source
        .direct_entity()
        .and_then(|entity| loot_equipment(&**entity));
    let direct_view = source
        .direct_entity()
        .map(|entity| entity_view(&**entity, &level, direct_equipment.as_ref()));

    let damage = damage_view(
        source,
        &source_view,
        &direct_view,
        original,
        actual,
        blocked,
    );
    raise(
        player,
        &Identifier::vanilla_static("player_hurt_entity"),
        &TriggerEvent::PlayerHurtEntity {
            base,
            damage,
            victim: &victim_view,
        },
    )
}

/// Raises `minecraft:used_totem` (`CriteriaTriggers.USED_TOTEM`).
///
/// Vanilla `LivingEntity.checkTotemDeathProtection` fires this with a copy of
/// the consumed totem stack after awarding the `used` stat.
pub fn raise_used_totem(player: &Player, item: &ItemStack) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("used_totem"),
        &TriggerEvent::UsedTotem {
            base: event_base(&view, &level, origin),
            item,
        },
    )
}

/// Raises `minecraft:changed_dimension`
/// (`CriteriaTriggers.CHANGED_DIMENSION`).
///
/// Vanilla `ServerPlayer.triggerDimensionChangeTriggers` fires this with the
/// dimension keys on either side of the change.
pub fn raise_changed_dimension(player: &Player, from: &Identifier, to: &Identifier) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("changed_dimension"),
        &TriggerEvent::ChangedDimension {
            base: event_base(&view, &level, origin),
            from,
            to,
        },
    )
}

/// Raises `minecraft:slept_in_bed` (`CriteriaTriggers.SLEPT_IN_BED`).
///
/// Vanilla `ServerPlayer.startSleepInBed` fires this with the sleeping player.
pub fn raise_slept_in_bed(player: &Player) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("slept_in_bed"),
        &TriggerEvent::Player(event_base(&view, &level, origin)),
    )
}

/// Raises `minecraft:tame_animal` (`CriteriaTriggers.TAME_ANIMAL`).
///
/// Vanilla `TamableAnimal.tame` fires this for the taming player with the newly
/// tamed animal.
pub fn raise_tame_animal(player: &Player, animal: &dyn Entity) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = animal.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let base = event_base(&player_entity, &level, origin);
    let animal_equipment = loot_equipment(animal);
    let animal_view = entity_view(animal, &level, animal_equipment.as_ref());
    raise(
        player,
        &Identifier::vanilla_static("tame_animal"),
        &TriggerEvent::TameAnimal {
            base,
            entity: &animal_view,
        },
    )
}

/// Raises `minecraft:bred_animals` (`CriteriaTriggers.BRED_ANIMALS`).
///
/// Vanilla `Animal.finalizeSpawnChildFromBreeding` fires this for the love-cause
/// player with both parents and the offspring once it exists.
pub fn raise_bred_animals(
    player: &Player,
    parent: &dyn Entity,
    partner: &dyn Entity,
    child: Option<&dyn Entity>,
) -> usize {
    let Some(world) = player.level() else {
        return 0;
    };
    let origin = parent.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let player_entity = player_view(player, &level, Some(&equipment), &known);
    let base = event_base(&player_entity, &level, origin);
    let parent_equipment = loot_equipment(parent);
    let parent_view = entity_view(parent, &level, parent_equipment.as_ref());
    let partner_equipment = loot_equipment(partner);
    let partner_view = entity_view(partner, &level, partner_equipment.as_ref());
    let child_equipment = child.and_then(loot_equipment);
    let child_view = child.map(|child| entity_view(child, &level, child_equipment.as_ref()));
    raise(
        player,
        &Identifier::vanilla_static("bred_animals"),
        &TriggerEvent::BredAnimals {
            base,
            parent: &parent_view,
            partner: &partner_view,
            child: child_view.as_ref(),
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{
        REGISTRY, init_vanilla_registry, vanilla_damage_types, vanilla_entities, vanilla_items,
    };
    use steel_utils::Identifier;

    use super::*;
    use crate::behavior::advancement_hooks::test_support::criterion_granted;
    use crate::entity::damage::DamageSource;
    use crate::player::Player;
    use crate::test_support::{TestEntity, TestPlayerBuilder, TestWorld, fresh_test_world};

    const SPAWN: DVec3 = DVec3::new(8.0, 64.0, 8.0);

    /// A fresh world and a player in it.
    ///
    /// The world key doubles as a description of the test, but it cannot double as
    /// the player name: `GameProfile` only accepts up to 16 alphanumerics or
    /// underscores.
    fn fixture(name: &'static str) -> (TestWorld, Arc<Player>) {
        init_vanilla_registry();
        let world_fixture = fresh_test_world(name);
        let player =
            TestPlayerBuilder::new(Arc::clone(&world_fixture.world), "TrigTester", 1).build();
        (world_fixture, player)
    }

    /// A throwaway entity of `entity_type` at the world spawn.
    fn entity(
        world: &Arc<World>,
        id: i32,
        entity_type: steel_registry::entity_type::EntityTypeRef,
    ) -> crate::entity::SharedEntity {
        TestEntity::shared(id, SPAWN, Arc::downgrade(world), entity_type)
    }

    #[test]
    fn tick_grants_unlock_right_away() {
        let (_world, player) = fixture("triggers_tick");
        let world = player.get_world();
        let origin = player.block_position();
        let level = WorldLevelView::new(&world, origin);
        raise_tick(&player, &level, origin);
        // Vanilla gives every recipe advancement an unconditional `unlock_right_away`
        // tick criterion next to `has_the_recipe`, so the first tick grants it.
        assert!(criterion_granted(
            &player,
            "recipes/decorations/crafting_table",
            "unlock_right_away"
        ));
    }

    #[test]
    fn tick_and_location_address_vanilla_triggers() {
        let (_world, player) = fixture("triggers_location");
        let world = player.get_world();
        let origin = player.block_position();
        let level = WorldLevelView::new(&world, origin);
        raise_tick(&player, &level, origin);
        raise_location(&player, &world, origin);
        assert!(
            !REGISTRY
                .advancements
                .criteria_for_trigger(&Identifier::vanilla_static("location"))
                .is_empty()
        );
    }

    #[test]
    fn slept_in_bed_grants_sleep_in_bed_advancement() {
        let (_world, player) = fixture("triggers_slept_in_bed");
        assert!(raise_slept_in_bed(&player) >= 1);
        // Vanilla ships no `slept_in_bed` criterion on `adventure/root`; the
        // `SLEPT_IN_BED` trigger is only listened for by `adventure/sleep_in_bed`.
        assert!(criterion_granted(
            &player,
            "adventure/sleep_in_bed",
            "slept_in_bed"
        ));
    }

    #[test]
    fn changed_dimension_grants_nether_root() {
        let (_world, player) = fixture("triggers_changed_dimension");
        let overworld = Identifier::vanilla_static("overworld");
        let nether = Identifier::vanilla_static("the_nether");
        assert!(raise_changed_dimension(&player, &overworld, &nether) >= 1);
        assert!(criterion_granted(&player, "nether/root", "entered_nether"));
    }

    #[test]
    fn used_totem_grants_totem_of_undying() {
        let (_world, player) = fixture("triggers_used_totem");
        let totem = ItemStack::new(&vanilla_items::TOTEM_OF_UNDYING);
        assert!(raise_used_totem(&player, &totem) >= 1);
        // `used_totem` is a criterion of `adventure/totem_of_undying`, not of
        // `adventure/kill_a_mob`.
        assert!(criterion_granted(
            &player,
            "adventure/totem_of_undying",
            "used_totem"
        ));
    }

    #[test]
    fn tame_animal_grants_tame_an_animal() {
        let (world_fixture, player) = fixture("triggers_tame_animal");
        let cow = entity(&world_fixture.world, 2, &vanilla_entities::COW);
        assert!(raise_tame_animal(&player, cow.as_ref()) >= 1);
        assert!(criterion_granted(
            &player,
            "husbandry/tame_an_animal",
            "tamed_animal"
        ));
    }

    #[test]
    fn bred_animals_grants_breed_an_animal() {
        let (world_fixture, player) = fixture("triggers_bred_animals");
        let cow = entity(&world_fixture.world, 2, &vanilla_entities::COW);
        let bull = entity(&world_fixture.world, 3, &vanilla_entities::COW);
        assert!(raise_bred_animals(&player, cow.as_ref(), bull.as_ref(), None) >= 1);
        assert!(criterion_granted(
            &player,
            "husbandry/breed_an_animal",
            "bred"
        ));
    }

    #[test]
    fn player_killed_entity_grants_kill_a_mob() {
        let (world_fixture, player) = fixture("triggers_player_killed_entity");
        let zombie = entity(&world_fixture.world, 2, &vanilla_entities::ZOMBIE);
        let blow = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK);
        assert!(raise_player_killed_entity(&player, zombie.as_ref(), &blow) >= 1);
        assert!(criterion_granted(
            &player,
            "adventure/kill_a_mob",
            "minecraft:zombie"
        ));
    }

    #[test]
    fn entity_killed_player_grants_adventure_root() {
        let (world_fixture, player) = fixture("triggers_entity_killed_player");
        let zombie = entity(&world_fixture.world, 2, &vanilla_entities::ZOMBIE);
        let blow = DamageSource::environment(&vanilla_damage_types::MOB_ATTACK);
        assert!(raise_entity_killed_player(&player, zombie.as_ref(), &blow) >= 1);
        assert!(criterion_granted(
            &player,
            "adventure/root",
            "killed_by_something"
        ));
    }

    #[test]
    fn entity_hurt_player_grants_deflect_arrow() {
        let (_world, player) = fixture("triggers_entity_hurt_player");
        let source = DamageSource::environment(&vanilla_damage_types::ARROW);
        assert!(raise_entity_hurt_player(&player, &source, 4.0, 4.0, true) >= 1);
        assert!(criterion_granted(
            &player,
            "story/deflect_arrow",
            "deflected_projectile"
        ));
    }

    #[test]
    fn player_hurt_entity_grants_shot_arrow() {
        let (world_fixture, player) = fixture("triggers_player_hurt_entity");
        let zombie = entity(&world_fixture.world, 2, &vanilla_entities::ZOMBIE);
        let arrow = entity(&world_fixture.world, 3, &vanilla_entities::ARROW);
        let source =
            DamageSource::environment(&vanilla_damage_types::ARROW).with_direct_entity(arrow);
        assert!(raise_player_hurt_entity(&player, zombie.as_ref(), &source, 4.0, 4.0, false) >= 1);
        // The `shot_arrow` criterion lives on `adventure/shoot_arrow` (the
        // `PLAYER_HURT_ENTITY` trigger with an arrow as the direct entity).
        assert!(criterion_granted(
            &player,
            "adventure/shoot_arrow",
            "shot_arrow"
        ));
    }
}

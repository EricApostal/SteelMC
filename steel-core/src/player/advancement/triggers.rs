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

use crate::entity::{Entity, entity_loot_ref, living_loot_equipment};
use crate::player::Player;
use crate::world::World;
use steel_registry::advancement::predicates::{
    EffectInstanceView, EntityComponentsPredicate, EntityView, EntityViewFlags, LevelView,
    PlayerView,
};
use steel_registry::advancement::triggers::{EventBase, TriggerEvent};
use steel_registry::{REGISTRY, RegistryExt};
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
        Some(&state.fluid_type().key)
    }

    fn light_at(&self, position: (f64, f64, f64)) -> i32 {
        i32::from(self.world.get_max_local_raw_brightness(block_pos(position)))
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
                    effect: &effect.effect.key,
                    amplifier: effect.amplifier,
                    duration: effect.duration,
                    ambient: effect.ambient,
                    visible: effect.visible,
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
        equipment: loot_ref.equipment,
        effects,
        components: EntityComponentsPredicate {
            cat_variant: None,
            frog_variant: None,
            wolf_variant: None,
            sheep_color: loot_ref.sheep_color,
            sheep_sheared: loot_ref.sheep_sheared,
            chicken_variant: loot_ref.chicken_variant,
        },
        vehicle: None,
        player: None,
        lightning: None,
        held_item: entity
            .as_living_entity()
            .map(|living| living.main_hand_item()),
    }
}

/// Builds the [`EntityView`] for a player, including its `type_specific/player`
/// facts.
#[must_use]
pub fn player_view<'a>(
    player: &'a Player,
    level: &'a dyn LevelView,
    equipment: Option<&'a steel_registry::loot_table::EntityEquipmentSlots>,
) -> EntityView<'a> {
    let mut view = entity_view(player, level, equipment);
    view.player = Some(PlayerView {
        looking_at: None,
        unlocked_recipes: player.recipe_book().unlocked_identifiers(),
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
    let mut guard = player.advancements.lock();
    let mut granted = 0;
    for (advancement, name) in criteria {
        let Some(criterion) = advancement.get_criterion(name) else {
            continue;
        };
        if criterion.test(event) && guard.award(player, advancement, name) {
            granted += 1;
        }
    }
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
    let view = player_view(player, level, Some(&equipment));
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
    let view = player_view(player, &level, Some(&equipment));
    raise(
        player,
        &Identifier::vanilla_static("location"),
        &TriggerEvent::Player(event_base(&view, &level, origin)),
    );
}

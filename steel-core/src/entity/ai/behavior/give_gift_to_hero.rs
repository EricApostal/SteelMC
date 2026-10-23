//! Vanilla `GiveGiftToHero`.

use std::collections::HashMap;
use std::sync::LazyLock;

use rand::SeedableRng as _;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::{
    EntityEquipmentRef, EntityRef, EntityRefFlags, LootContext, LootTableRef,
};
use steel_registry::{vanilla_loot_tables, vanilla_mob_effects};

use super::utils;
use super::villager;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::entity::{Entity as _, SharedEntity};

/// Vanilla `GiveGiftToHero.THROW_GIFT_AT_DISTANCE`, `closerThan(..., 5.0)`.
const THROW_GIFT_DISTANCE_SQR: f64 = 25.0;
/// Vanilla `GiveGiftToHero.MIN_TIME_BETWEEN_GIFTS`.
const MIN_TIME_BETWEEN_GIFTS: i32 = 600;
/// Vanilla `GiveGiftToHero.MAX_TIME_BETWEEN_GIFTS`, reachable `600 + nextInt(6001)`.
const COOLDOWN_JITTER: i32 = 6001;
/// Vanilla `GiveGiftToHero.TIME_TO_DELAY_FOR_HEAD_TO_FINISH_TURNING`.
const HEAD_TURN_DELAY: i64 = 20;
/// Vanilla `GiveGiftToHero.SPEED_MODIFIER`.
const SPEED_MODIFIER: f32 = 0.5;
/// Vanilla `BehaviorUtils.setWalkAndLookTargetMemories(..., 5)` close-enough distance.
const WALK_CLOSE_ENOUGH: i32 = 5;

/// Vanilla `GiveGiftToHero.GIFTS`, keyed by the profession's registry path.
///
/// Steel's vendored tables only carry the `GAMEPLAY_HERO_OF_THE_VILLAGE_*`
/// states (the armorer table is named `GAMEPLAY_HERO_OF_THE_VILLAGE_ARMORER_GIFT`,
/// not `ARMORER_GIFT`); the map keys are the profession key paths instead of the
/// vanilla `BuiltInLootTables` keys.
static GIFTS: LazyLock<HashMap<&'static str, LootTableRef>> = LazyLock::new(|| {
    HashMap::from([
        (
            "armorer",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_ARMORER_GIFT,
        ),
        (
            "butcher",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_BUTCHER_GIFT,
        ),
        (
            "cartographer",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_CARTOGRAPHER_GIFT,
        ),
        (
            "cleric",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_CLERIC_GIFT,
        ),
        (
            "farmer",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_FARMER_GIFT,
        ),
        (
            "fisherman",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_FISHERMAN_GIFT,
        ),
        (
            "fletcher",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_FLETCHER_GIFT,
        ),
        (
            "leatherworker",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_LEATHERWORKER_GIFT,
        ),
        (
            "librarian",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_LIBRARIAN_GIFT,
        ),
        (
            "mason",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_MASON_GIFT,
        ),
        (
            "shepherd",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_SHEPHERD_GIFT,
        ),
        (
            "toolsmith",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_TOOLSMITH_GIFT,
        ),
        (
            "weaponsmith",
            &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_WEAPONSMITH_GIFT,
        ),
    ])
});

/// Vanilla `GiveGiftToHero`: walk to a hero-of-the-village player and throw a
/// profession gift at them after the head-turn delay.
pub struct GiveGiftToHero {
    state: BehaviorState,
    time_until_next_gift: i32,
    gift_given_during_this_run: bool,
    time_since_start: i64,
}

impl GiveGiftToHero {
    /// Vanilla `GiveGiftToHero(int timeout)`.
    #[must_use]
    pub fn new(timeout: i32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::WALK_TARGET, MemoryStatus::Registered),
                    (&mem::LOOK_TARGET, MemoryStatus::Registered),
                    (&mem::INTERACTION_TARGET, MemoryStatus::Registered),
                    (&mem::NEAREST_VISIBLE_PLAYER, MemoryStatus::ValuePresent),
                ],
                timeout,
                timeout,
            ),
            time_until_next_gift: MIN_TIME_BETWEEN_GIFTS,
            gift_given_during_this_run: false,
            time_since_start: 0,
        }
    }

    /// Vanilla `GiveGiftToHero.getNearestTargetableHero`.
    fn nearest_targetable_hero(ctx: &BehaviorContext<'_>) -> Option<SharedEntity> {
        ctx.get_memory(&mem::NEAREST_VISIBLE_PLAYER)
            .filter(|player| is_hero(player))
    }

    /// Vanilla `GiveGiftToHero.throwGift`.
    fn throw_gift(&self, ctx: &mut BehaviorContext<'_>, target: &SharedEntity) {
        let Some(villager) = villager::as_villager(ctx.body) else {
            return;
        };

        for item in roll_gift(ctx, villager) {
            utils::throw_item(ctx.body, item, target.position());
        }
    }
}

/// Vanilla `GiveGiftToHero.isHero`.
fn is_hero(player: &SharedEntity) -> bool {
    player
        .as_living_entity()
        .is_some_and(|living| living.has_mob_effect(&vanilla_mob_effects::HERO_OF_THE_VILLAGE))
}

/// Vanilla `GiveGiftToHero.isWithinThrowingDistance`: `BlockPos.closerThan(other, 5.0)`.
fn is_within_throwing_distance(villager: &SharedEntity, player: &SharedEntity) -> bool {
    let villager_pos = villager.block_position().0;
    let player_pos = player.block_position().0;
    let dx = f64::from(villager_pos.x - player_pos.x);
    let dy = f64::from(villager_pos.y - player_pos.y);
    let dz = f64::from(villager_pos.z - player_pos.z);

    dx * dx + dy * dy + dz * dz <= THROW_GIFT_DISTANCE_SQR
}

/// Vanilla `GiveGiftToHero.getLootTableToThrow`.
fn loot_table_to_throw(villager: &VillagerEntity) -> LootTableRef {
    if villager
        .as_living_entity()
        .is_some_and(|living| living.is_baby())
    {
        return &vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_BABY_GIFT;
    }

    let path = villager.profession().key.path.as_ref();
    GIFTS
        .get(path)
        .copied()
        .unwrap_or(&vanilla_loot_tables::GAMEPLAY_HERO_OF_THE_VILLAGE_UNEMPLOYED_GIFT)
}

/// Vanilla `GiveGiftToHero.calculateTimeUntilNextGift`.
fn time_until_next_gift() -> i32 {
    MIN_TIME_BETWEEN_GIFTS + rand::random_range(0..COOLDOWN_JITTER)
}

/// Vanilla `LivingEntity.dropFromGiftLootTable`: rolls `table` with only the
/// origin and thrower parameters and returns the items.
///
/// `entity_loot_ref`/`living_loot_equipment` are `pub(crate)`, so the thrower
/// reference is built inline from the public `Entity`/`LivingEntity` accessors.
fn roll_gift(ctx: &BehaviorContext<'_>, villager: &VillagerEntity) -> Vec<ItemStack> {
    let equipment: [ItemStack; EquipmentSlot::ALL.len()] = std::array::from_fn(|index| {
        let mut stack = ItemStack::empty();
        ctx.body.as_living_entity().map(|living| {
            living.with_equipment_slot(EquipmentSlot::ALL[index], &mut |slot| {
                stack = slot.clone();
            });
        });
        stack
    });

    let body: &dyn crate::entity::Entity = ctx.body.as_ref();
    let entity_ref = EntityRef {
        entity_type: Some(&body.entity_type().key),
        flags: EntityRefFlags {
            is_on_fire: body.is_on_fire(),
            is_sneaking: body.is_crouching(),
            is_sprinting: body
                .as_living_entity()
                .is_some_and(crate::entity::LivingEntity::is_sprinting),
            is_swimming: body.is_swimming(),
            is_baby: body
                .as_living_entity()
                .is_some_and(crate::entity::LivingEntity::is_baby),
        },
        equipment: Some(EntityEquipmentRef::new(&equipment)),
        custom_name: None,
        sheep_color: None,
        sheep_sheared: None,
        chicken_variant: None,
    };

    let position = ctx.body.position();
    // Vanilla `dropFromGiftLootTable` rolls with vanilla's level random; Steel
    // has no per-level RNG visible from a behavior (`LegacyRandom` reseeds on
    // every `EntityBase` from the wall clock), so this seeds a throwaway from
    // the OS just like the seedable-path fallback in the loot pipeline.
    let mut rng = rand::rngs::StdRng::try_from_rng(&mut rand::rngs::SysRng).unwrap_or_else(|_| {
        rand::rngs::StdRng::seed_from_u64(crate::entity::next_entity_id() as u64)
    });
    let mut context = LootContext::new(&mut rng)
        .with_origin(position.x, position.y, position.z)
        .with_this_entity(entity_ref);

    loot_table_to_throw(villager).get_random_items(&mut context)
}

impl Behavior for GiveGiftToHero {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        Self::nearest_targetable_hero(ctx).is_some() && self.time_until_next_gift <= 0
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.gift_given_during_this_run = false;
        self.time_since_start = ctx.timestamp;

        if let Some(player) = Self::nearest_targetable_hero(ctx) {
            ctx.set_memory(&mem::INTERACTION_TARGET, player.clone());
            utils::look_at_entity(ctx, &player);
        }
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        Self::nearest_targetable_hero(ctx).is_some() && !self.gift_given_during_this_run
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        // Vanilla `GiveGiftToHero.checkExtraStartConditions` decrements
        // `timeUntilNextGift` on every failed activation; Steel's check is
        // `&self`, so the decrement runs here, once per active tick.
        if self.time_until_next_gift > 0 {
            self.time_until_next_gift -= 1;
        }

        let Some(player) = Self::nearest_targetable_hero(ctx) else {
            return;
        };

        utils::look_at_entity(ctx, &player);
        if is_within_throwing_distance(ctx.body, &player) {
            if ctx.timestamp - self.time_since_start > HEAD_TURN_DELAY {
                self.throw_gift(ctx, &player);
                self.gift_given_during_this_run = true;
            }
        } else {
            utils::set_walk_and_look_target_to_entity(
                ctx,
                &player,
                SPEED_MODIFIER,
                WALK_CLOSE_ENOUGH,
            );
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.time_until_next_gift = time_until_next_gift();
        ctx.erase_memory(&mem::INTERACTION_TARGET);
        utils::clear_walk_target(ctx);
        utils::clear_look_target(ctx);
    }
}

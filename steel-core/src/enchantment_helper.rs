mod item_in_use;

use std::sync::Arc;

use item_in_use::EnchantedItemInUse;

use glam::IVec3;
use std::any::try_as_dyn;
use steel_registry::enchantment::EnchantmentRef;
use steel_registry::enchantment_effect::{
    BlockPredicateRef, DamageSourcePredicate, EnchantmentEffectComponent,
    EnchantmentEffectRequirements, EnchantmentEntityEffect, EnchantmentEntityTarget,
    EnchantmentTarget, EntityLocationPredicate, EntityPredicate, EntityTypePredicate,
    EntityTypeSet, EntityTypeSpecificPredicate, EntityVehiclePredicate, LevelBasedValue,
    MobEffectSelection,
};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::feature::BlockStateData;
use steel_registry::game_events::GameEventRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::{REGISTRY, RegistryExt, TaggedRegistryExt, vanilla_entities, vanilla_items};

use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::{BlockPos, Identifier};

use crate::block_state_provider::BlockStateProviderEvaluator;
use crate::entity::attribute::AttributeModifier;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, LivingEntity, MobEffectInstance, SharedEntity};
use crate::inventory::equipment::EquipmentSlot;
use crate::world::World;
use crate::world::game_event::GameEventContext;
use steel_registry::attribute::AttributeRef;
use steel_utils::types::UpdateFlags;

/// A snapshot of the world's weather taken once per enchantment fold so that
/// requirement evaluation never re-locks the weather mutex per term.
///
/// Fold sites without a world keep `None`, which leaves `WeatherCheck` unknown
/// (fail-closed) exactly like the other context-less predicates.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WeatherSnapshot {
    raining: bool,
    thundering: bool,
}

impl WeatherSnapshot {
    /// Reads the world's rain/thunder state while holding the weather lock once.
    pub(crate) fn from_world(world: &World) -> Self {
        let weather = world.weather.lock();
        Self {
            raining: world.is_raining_with_guard(&weather),
            thundering: world.is_thundering_with_guard(&weather),
        }
    }

    /// Vanilla `WeatherCheck.test`: every present flag must equal the level state.
    fn matches(self, raining: Option<bool>, thundering: Option<bool>) -> bool {
        raining.is_none_or(|raining| raining == self.raining)
            && thundering.is_none_or(|thundering| thundering == self.thundering)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EnchantmentDamageContext<'a> {
    this_entity_type: EntityTypeRef,
    attacker_entity_type: Option<EntityTypeRef>,
    direct_attacker_entity_type: Option<EntityTypeRef>,
    this_entity: Option<&'a dyn Entity>,
    attacker_entity: Option<&'a SharedEntity>,
    direct_attacker_entity: Option<&'a SharedEntity>,
    damage_source: &'a DamageSource,
}

impl<'a> EnchantmentDamageContext<'a> {
    #[must_use]
    pub(crate) const fn new(
        this_entity_type: EntityTypeRef,
        attacker_entity_type: Option<EntityTypeRef>,
        direct_attacker_entity_type: Option<EntityTypeRef>,
        damage_source: &'a DamageSource,
    ) -> Self {
        Self {
            this_entity_type,
            attacker_entity_type,
            direct_attacker_entity_type,
            this_entity: None,
            attacker_entity: None,
            direct_attacker_entity: None,
            damage_source,
        }
    }

    /// Attaches the concrete entities behind each target so predicates that need
    /// world state (e.g. `location.can_see_sky`) can be evaluated.
    #[must_use]
    pub(crate) const fn with_entities(
        mut self,
        this_entity: Option<&'a dyn Entity>,
        attacker_entity: Option<&'a SharedEntity>,
        direct_attacker_entity: Option<&'a SharedEntity>,
    ) -> Self {
        self.this_entity = this_entity;
        self.attacker_entity = attacker_entity;
        self.direct_attacker_entity = direct_attacker_entity;
        self
    }

    #[must_use]
    pub(crate) fn from_damage_source(
        this_entity_type: EntityTypeRef,
        damage_source: &'a DamageSource,
    ) -> Self {
        let attacker_entity_type = damage_source
            .causing_entity()
            .map(|entity| entity.entity_type());
        let direct_attacker_entity_type = damage_source
            .direct_entity()
            .map(|entity| entity.entity_type());
        Self::new(
            this_entity_type,
            attacker_entity_type,
            direct_attacker_entity_type,
            damage_source,
        )
    }

    const fn entity_type(self, target: EnchantmentEntityTarget) -> Option<EntityTypeRef> {
        match target {
            EnchantmentEntityTarget::This => Some(self.this_entity_type),
            EnchantmentEntityTarget::Attacker => self.attacker_entity_type,
            EnchantmentEntityTarget::DirectAttacker => self.direct_attacker_entity_type,
        }
    }

    fn entity(self, target: EnchantmentEntityTarget) -> Option<&'a dyn Entity> {
        match target {
            EnchantmentEntityTarget::This => self.this_entity,
            EnchantmentEntityTarget::Attacker => self.attacker_entity.map(AsRef::as_ref),
            EnchantmentEntityTarget::DirectAttacker => {
                self.direct_attacker_entity.map(AsRef::as_ref)
            }
        }
    }
}

pub(crate) struct EnchantmentPostAttackContext<'a> {
    victim: &'a SharedEntity,
    attacker: Option<&'a SharedEntity>,
    direct_attacker: Option<&'a SharedEntity>,
    damage_source: &'a DamageSource,
}

impl<'a> EnchantmentPostAttackContext<'a> {
    #[must_use]
    pub(crate) const fn new(victim: &'a SharedEntity, damage_source: &'a DamageSource) -> Self {
        Self {
            victim,
            attacker: damage_source.causing_entity(),
            direct_attacker: damage_source.direct_entity(),
            damage_source,
        }
    }

    fn damage_context(&self) -> EnchantmentDamageContext<'a> {
        EnchantmentDamageContext::new(
            self.victim.entity_type(),
            self.attacker.map(|entity| entity.entity_type()),
            self.direct_attacker.map(|entity| entity.entity_type()),
            self.damage_source,
        )
        .with_entities(
            Some(self.victim.as_ref()),
            self.attacker,
            self.direct_attacker,
        )
    }

    fn affected_entity(&self, target: EnchantmentTarget) -> Option<&'a SharedEntity> {
        match target {
            EnchantmentTarget::Attacker => self.attacker,
            EnchantmentTarget::DamagingEntity => self.direct_attacker,
            EnchantmentTarget::Victim => Some(self.victim),
        }
    }
}

pub(crate) fn modify_damage(
    item: &ItemStack,
    context: &EnchantmentDamageContext<'_>,
    damage: f32,
) -> f32 {
    apply_value_effects(item, EnchantmentEffectComponent::Damage, context, damage)
}

/// Vanilla `EnchantmentHelper.forEachModifier(ItemStack, EquipmentSlot, BiConsumer)`:
/// visits every attribute modifier the stack's `minecraft:attributes`
/// enchantment effects grant while the stack is worn in `slot`.
///
/// Vanilla applies these as *transient* modifiers when equipment changes
/// (`LivingEntity.detectEquipmentUpdates`), which is where Steel calls this
/// (see `LivingBase::refresh_equipment_attribute_modifiers`); the modifier id
/// carries the slot suffix, exactly like `EnchantmentAttributeEffect.idForSlot`.
pub(crate) fn for_each_enchantment_attribute_modifier(
    item: &ItemStack,
    slot: EquipmentSlot,
    mut consumer: impl FnMut(AttributeRef, AttributeModifier),
) {
    if item.is_empty() {
        return;
    }
    let Some(enchantments) = item.get_enchantments() else {
        return;
    };

    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        if !enchantment.matching_slot(slot) {
            continue;
        }

        for effect in enchantment.effects.attributes {
            consumer(
                effect.attribute,
                AttributeModifier {
                    id: Identifier::new(
                        effect.id.namespace.clone(),
                        format!("{}/{}", effect.id.path, slot.name()),
                    ),
                    amount: f64::from(effect.amount.calculate(*level as i32)),
                    operation: effect.operation,
                },
            );
        }
    }
}

/// Vanilla `EnchantmentHelper.getEnchantmentLevel`: the best level of
/// `enchantment` across the slots the enchantment supports. No caller yet;
/// keeping it for the enchantment checks (noiseless XP tiers, silent armor
/// faces, strider warmth gating) that all delegate to this single scan.
#[expect(
    dead_code,
    reason = "vanilla API surface; no caller yet — all future level scans should use this"
)]
pub(crate) fn get_enchantment_level<V: LivingEntity + ?Sized>(
    enchantment: EnchantmentRef,
    entity: &V,
) -> i32 {
    let mut best_level = 0;
    for slot in EquipmentSlot::ALL {
        if !enchantment.matching_slot(slot) {
            continue;
        }
        entity.with_equipment_slot(slot, &mut |item| {
            best_level = best_level.max(item.get_enchantment_level(&enchantment.key));
        });
    }

    best_level
}

/// Vanilla `EnchantmentHelper.processMobExperience`: folds every equipped
/// `minecraft:mob_experience` effect of `killer` over the dropped amount.
pub(crate) fn process_mob_experience(killer: &dyn Entity, victim: &dyn Entity, amount: i32) -> i32 {
    let Some(living_killer) = killer.as_living_entity() else {
        return amount;
    };

    let mut value = amount as f32;
    for slot in EquipmentSlot::ALL {
        living_killer.with_equipment_slot(slot, &mut |item| {
            value = apply_entity_value_effects(
                item,
                EnchantmentEffectComponent::MobExperience,
                victim,
                value,
            );
        });
    }

    value as i32
}

/// Vanilla `EnchantmentHelper.processEquipmentDropChance`: folds the
/// `minecraft:equipment_drops` `add`/`multiply` effects over the equipment drop
/// chance.
///
/// Vanilla's `entity` argument is the *killer* — `Mob.dropCustomDeathLoot`
/// passes `source.getEntity()`, not the dying mob — and this method iterates that
/// entity's equipment twice: once requiring `enchanted = victim`, `affected =
/// victim`, then again with `enchanted = attacker`, `affected = victim`. Steel
/// mirrors both passes (the second reads the killer back out of `damage_source`,
/// which is the same entity at vanilla's call site).
pub(crate) fn modify_equipment_drop_chance(
    entity: &dyn LivingEntity,
    damage_source: &DamageSource,
    chance: f32,
) -> f32 {
    let context = EnchantmentDamageContext::from_damage_source(entity.entity_type(), damage_source);
    // The equipment-drops fold has no level handle; derive the weather from the
    // victim's level, keeping `WeatherCheck` unknown (fail-closed) when offline.
    let weather = entity
        .level()
        .map(|world| WeatherSnapshot::from_world(&world));
    let mut value = chance;
    // Vanilla threads the victim's `RandomSource` through every effect.
    let mut random = WorldgenRandom::from_seed(rand::random());

    for slot in EquipmentSlot::ALL {
        entity.with_equipment_slot(slot, &mut |item| {
            value = apply_equipment_drop_effects(
                item,
                slot,
                EnchantmentTarget::Victim,
                &context,
                weather,
                &mut random,
                value,
            );
        });
    }

    if let Some(attacker) = damage_source.causing_entity()
        && let Some(living_attacker) = attacker.as_living_entity()
    {
        for slot in EquipmentSlot::ALL {
            living_attacker.with_equipment_slot(slot, &mut |item| {
                value = apply_equipment_drop_effects(
                    item,
                    slot,
                    EnchantmentTarget::Attacker,
                    &context,
                    weather,
                    &mut random,
                    value,
                );
            });
        }
    }

    value
}

/// Folds the `minecraft:equipment_drops` effects of one item whose `enchanted`
/// target is `enchanted` and whose `affected` target is the victim (vanilla
/// `EnchantmentHelper.processEquipmentDropChance`).
fn apply_equipment_drop_effects(
    item: &ItemStack,
    slot: EquipmentSlot,
    enchanted: EnchantmentTarget,
    context: &EnchantmentDamageContext<'_>,
    weather: Option<WeatherSnapshot>,
    random: &mut WorldgenRandom,
    input: f32,
) -> f32 {
    if item.is_empty() {
        return input;
    }
    let Some(enchantments) = item.get_enchantments() else {
        return input;
    };

    let mut value = input;
    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        if !enchantment.matching_slot(slot) {
            continue;
        }

        for effect in enchantment.effects.equipment_drops {
            if effect.enchanted != enchanted
                || effect.affected != EnchantmentTarget::Victim
                || !post_attack_requirements_match(
                    effect.requirements,
                    context,
                    *level as i32,
                    weather,
                )
            {
                continue;
            }
            value = effect
                .effect
                .process_with_random(*level as i32, random, value);
        }
    }

    value
}

/// Vanilla `Enchantment.modifyEntityFilteredValue` for one item: folds the
/// component's conditional value effects over `input` using `entity` predicates.
fn apply_entity_value_effects(
    item: &ItemStack,
    component: EnchantmentEffectComponent,
    entity: &dyn Entity,
    input: f32,
) -> f32 {
    if item.is_empty() {
        return input;
    }
    let Some(enchantments) = item.get_enchantments() else {
        return input;
    };

    let weather = entity
        .level()
        .map(|world| WeatherSnapshot::from_world(&world));
    let mut value = input;
    let mut random = WorldgenRandom::from_seed(rand::random());
    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        let level = *level as i32;

        for effect in enchantment.effects.value_effects(component) {
            if !entity_requirements_match(effect.requirements, entity, level, weather) {
                continue;
            }
            value = effect.effect.process_with_random(level, &mut random, value);
        }
    }

    value
}

pub(crate) fn modify_knockback(
    item: &ItemStack,
    context: &EnchantmentDamageContext<'_>,
    knockback: f32,
) -> f32 {
    apply_value_effects(
        item,
        EnchantmentEffectComponent::Knockback,
        context,
        knockback,
    )
}

pub(crate) fn modify_armor_effectiveness(
    item: &ItemStack,
    context: &EnchantmentDamageContext<'_>,
    armor_fraction: f32,
) -> f32 {
    apply_value_effects(
        item,
        EnchantmentEffectComponent::ArmorEffectiveness,
        context,
        armor_fraction,
    )
}

pub(crate) fn modify_smash_damage_per_fallen_block(
    item: &ItemStack,
    context: &EnchantmentDamageContext<'_>,
    damage: f32,
) -> f32 {
    apply_value_effects(
        item,
        EnchantmentEffectComponent::SmashDamagePerFallenBlock,
        context,
        damage,
    )
}

pub(crate) fn is_immune_to_damage<V: LivingEntity + ?Sized>(
    victim: &V,
    damage_source: &DamageSource,
) -> bool {
    let context = EnchantmentDamageContext::from_damage_source(victim.entity_type(), damage_source);

    for slot in EquipmentSlot::ALL {
        let mut slot_matches = false;
        victim.with_equipment_slot(slot, &mut |item| {
            slot_matches = item_damage_immunity_matches(item, slot, &context);
        });
        if slot_matches {
            return true;
        }
    }

    false
}

pub(crate) fn get_damage_protection<V: LivingEntity + ?Sized>(
    victim: &V,
    damage_source: &DamageSource,
) -> f32 {
    let context = EnchantmentDamageContext::from_damage_source(victim.entity_type(), damage_source);
    let mut protection = 0.0;

    for slot in EquipmentSlot::ALL {
        victim.with_equipment_slot(slot, &mut |item| {
            protection = apply_value_effects_for_slot(
                item,
                Some(slot),
                EnchantmentEffectComponent::DamageProtection,
                &context,
                protection,
            );
        });
    }

    protection
}

pub(crate) fn do_post_attack_effects_from_item(
    world: &World,
    item: &ItemStack,
    context: &EnchantmentPostAttackContext<'_>,
) {
    let mut item = item.copy_with_count(item.count());
    let _ = apply_post_attack_effects(
        world,
        &mut EnchantedItemInUse::Stack(&mut item),
        Some(EquipmentSlot::MainHand),
        EnchantmentTarget::Attacker,
        context,
    );
}

pub(crate) fn do_post_attack_effects_with_item_source(
    world: &World,
    victim: &dyn Entity,
    source: &ItemStack,
    context: &EnchantmentPostAttackContext<'_>,
) {
    if let Some(living_victim) = victim.as_living_entity() {
        for slot in EquipmentSlot::ALL {
            let mut item = EnchantedItemInUse::Equipped {
                owner: living_victim,
                slot,
            };
            let mut item_ref = &*vanilla_items::AIR;
            item.with_item(&mut |stack| item_ref = stack.item);
            let item_broke = apply_post_attack_effects(
                world,
                &mut item,
                Some(slot),
                EnchantmentTarget::Victim,
                context,
            );
            if item_broke {
                living_victim.on_equipped_item_broken(item_ref, slot);
            }
        }
    }

    let mut source = source.copy_with_count(source.count());
    let _ = apply_post_attack_effects(
        world,
        &mut EnchantedItemInUse::Stack(&mut source),
        Some(EquipmentSlot::MainHand),
        EnchantmentTarget::Attacker,
        context,
    );
}

fn apply_value_effects(
    item: &ItemStack,
    component: EnchantmentEffectComponent,
    context: &EnchantmentDamageContext<'_>,
    input: f32,
) -> f32 {
    apply_value_effects_for_slot(item, None, component, context, input)
}

fn apply_value_effects_for_slot(
    item: &ItemStack,
    slot: Option<EquipmentSlot>,
    component: EnchantmentEffectComponent,
    context: &EnchantmentDamageContext<'_>,
    input: f32,
) -> f32 {
    if slot.is_some() && item.is_empty() {
        return input;
    }
    let Some(enchantments) = item.get_enchantments() else {
        return input;
    };

    let mut value = input;
    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        if slot.is_some_and(|slot| !enchantment.matching_slot(slot)) {
            continue;
        }
        let level = *level as i32;

        for effect in enchantment.effects.value_effects(component) {
            if !requirements_match(effect.requirements, context) {
                continue;
            }
            if let Some(updated) = effect.effect.process_without_random(level, value) {
                value = updated;
            }
        }

        let Some(effect) = enchantment.effects.single_value_effect(component) else {
            continue;
        };
        if let Some(updated) = effect.process_without_random(level, value) {
            value = updated;
        }
    }

    value
}

fn item_damage_immunity_matches(
    item: &ItemStack,
    slot: EquipmentSlot,
    context: &EnchantmentDamageContext<'_>,
) -> bool {
    if item.is_empty() {
        return false;
    }
    let Some(enchantments) = item.get_enchantments() else {
        return false;
    };

    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        if !enchantment.matching_slot(slot) {
            continue;
        }
        if enchantment
            .effects
            .damage_immunity
            .iter()
            .any(|effect| requirements_match(effect.requirements, context))
        {
            return true;
        }
    }

    false
}

fn apply_post_attack_effects(
    world: &World,
    item: &mut EnchantedItemInUse<'_>,
    slot: Option<EquipmentSlot>,
    enchanted_target: EnchantmentTarget,
    context: &EnchantmentPostAttackContext<'_>,
) -> bool {
    let mut enchantments = None;
    item.with_item(&mut |stack| {
        if slot.is_none() || !stack.is_empty() {
            enchantments = stack.get_enchantments().cloned();
        }
    });
    let Some(enchantments) = enchantments else {
        return false;
    };
    let damage_context = context.damage_context();
    let weather = Some(WeatherSnapshot::from_world(world));
    let mut enchanted_item_broke = false;

    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        if slot.is_some_and(|slot| !enchantment.matching_slot(slot)) {
            continue;
        }
        let level = *level as i32;

        for effect in enchantment.effects.post_attack {
            if effect.enchanted != enchanted_target {
                continue;
            }
            let Some(enchanted_entity) = context.affected_entity(enchanted_target) else {
                continue;
            };
            let requirements_match = post_attack_requirements_match(
                effect.requirements,
                &damage_context,
                level,
                weather,
            );
            if !requirements_match {
                continue;
            }
            let Some(affected_entity) = context.affected_entity(effect.affected) else {
                continue;
            };
            enchanted_item_broke |= apply_entity_effect(
                world,
                &effect.effect,
                level,
                affected_entity.as_ref(),
                Some(enchanted_entity),
                item,
            );
        }
    }

    enchanted_item_broke
}

/// Mirrors vanilla `EnchantmentHelper.onProjectileSpawned` after the projectile
/// has been handed to the world.
pub(crate) fn on_projectile_spawned(
    world: &World,
    weapon: &mut ItemStack,
    projectile: &dyn Entity,
    owner: Option<&SharedEntity>,
) {
    let Some(enchantments) = weapon.get_enchantments() else {
        return;
    };
    let enchantments = enchantments
        .iter()
        .filter_map(|(key, level)| {
            if *level == 0 {
                return None;
            }
            REGISTRY
                .enchantments
                .by_key(key)
                .map(|enchantment| (enchantment, *level))
        })
        .collect::<Vec<_>>();

    let weather = Some(WeatherSnapshot::from_world(world));
    for (enchantment, level) in enchantments {
        let level = level as i32;
        for effect in enchantment.effects.projectile_spawned {
            if entity_requirements_match(effect.requirements, projectile, level, weather) {
                let _ = apply_entity_effect(
                    world,
                    &effect.effect,
                    level,
                    projectile,
                    owner,
                    &mut EnchantedItemInUse::Stack(weapon),
                );
            }
        }
    }
}

/// Vanilla `EnchantmentHelper.runLocationChangedEffects(ServerLevel, LivingEntity)`:
/// whenever a living entity steps into a new block, every `minecraft:location_changed`
/// effect of its worn equipment runs. Frost Walker is the shipped consumer.
///
/// Vanilla keeps the matching effects of each slot in a set so it can call
/// `onDeactivated` on the ones that stop matching. Steel has no stateful location
/// effect yet: the only one vanilla ships (`ReplaceDisk`) is stateless — the
/// frosted ice it places reverts through `FrostedIceBlock`'s own scheduled ticks —
/// so a slot that stops matching simply runs no effect. A future stateful
/// location effect must add that per-slot set (reported gap).
pub(crate) fn run_location_changed_effects<V: LivingEntity + ?Sized>(
    entity: &V,
    world: &Arc<World>,
) {
    let Some(entity) = try_as_dyn::<V, dyn LivingEntity>(entity) else {
        return;
    };
    let weather = Some(WeatherSnapshot::from_world(world));
    for slot in EquipmentSlot::ALL {
        let mut enchantments = None;
        entity.with_equipment_slot(slot, &mut |stack| {
            if !stack.is_empty() {
                enchantments = stack.get_enchantments().cloned();
            }
        });
        let Some(enchantments) = enchantments else {
            continue;
        };

        let mut item = EnchantedItemInUse::Equipped {
            owner: entity,
            slot,
        };
        for (key, level) in enchantments.iter() {
            if *level == 0 {
                continue;
            }
            let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
                continue;
            };
            if !enchantment.matching_slot(slot) {
                continue;
            }

            let level = *level as i32;
            for effect in enchantment.effects.location_changed {
                if !entity_requirements_match(effect.requirements, entity, level, weather) {
                    continue;
                }
                let _ = apply_entity_effect(world, &effect.effect, level, entity, None, &mut item);
            }
        }
    }
}

pub(crate) fn do_post_piercing_attack_effects(world: &World, user: &dyn LivingEntity) {
    let mut item_stack = ItemStack::empty();
    user.with_equipment_slot(EquipmentSlot::MainHand, &mut |stack| {
        item_stack = stack.copy_with_count(stack.count());
    });
    let enchantments = item_stack.get_enchantments().cloned();
    let Some(enchantments) = enchantments else {
        return;
    };

    let item_ref = item_stack.item;
    let weather = Some(WeatherSnapshot::from_world(world));
    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        if !enchantment.matching_slot(EquipmentSlot::MainHand) {
            continue;
        }

        let level = *level as i32;
        for effect in enchantment.effects.post_piercing_attack {
            if !entity_requirements_match(effect.requirements, user, level, weather) {
                continue;
            }
            if apply_post_piercing_entity_effect(world, &effect.effect, level, user) {
                user.on_equipped_item_broken(item_ref, EquipmentSlot::MainHand);
            }
        }
    }
}

fn apply_entity_effect(
    world: &World,
    effect: &EnchantmentEntityEffect,
    level: i32,
    entity: &dyn Entity,
    enchanted_entity: Option<&SharedEntity>,
    enchanted_item: &mut EnchantedItemInUse<'_>,
) -> bool {
    if !entity_effect_is_supported(effect) {
        return false;
    }

    apply_supported_entity_effect(
        world,
        effect,
        level,
        entity,
        enchanted_entity,
        enchanted_item,
    )
}

fn entity_effect_is_supported(effect: &EnchantmentEntityEffect) -> bool {
    match effect {
        EnchantmentEntityEffect::AllOf(effects) => effects
            .iter()
            .all(|effect| entity_effect_is_supported(effect)),
        EnchantmentEntityEffect::ApplyExhaustion { .. }
        | EnchantmentEntityEffect::ApplyImpulse { .. }
        | EnchantmentEntityEffect::SpawnParticles { .. }
        | EnchantmentEntityEffect::Attribute(_)
        | EnchantmentEntityEffect::Unsupported { .. } => false,
        EnchantmentEntityEffect::PlaySound { sounds, .. } => !sounds.is_empty(),
        EnchantmentEntityEffect::ChangeItemDamage { .. }
        | EnchantmentEntityEffect::DamageEntity { .. }
        | EnchantmentEntityEffect::Ignite { .. }
        | EnchantmentEntityEffect::ReplaceDisk { .. }
        | EnchantmentEntityEffect::SummonEntity { .. } => true,
        EnchantmentEntityEffect::ApplyMobEffect { to_apply, .. } => {
            matches!(to_apply, MobEffectSelection::Single(_))
        }
    }
}

/// Vanilla `EnchantmentHelper.applySupportedEntityEffect`: dispatch over every
/// supported entity effect.
#[expect(
    clippy::too_many_lines,
    reason = "single dispatch over every vanilla entity effect, mirroring one vanilla `applySupportedEntityEffect`; splitting would scatter the effect table"
)]
fn apply_supported_entity_effect(
    world: &World,
    effect: &EnchantmentEntityEffect,
    level: i32,
    entity: &dyn Entity,
    enchanted_entity: Option<&SharedEntity>,
    enchanted_item: &mut EnchantedItemInUse<'_>,
) -> bool {
    match effect {
        EnchantmentEntityEffect::AllOf(effects) => {
            let mut enchanted_item_broke = false;
            for effect in *effects {
                enchanted_item_broke |= apply_supported_entity_effect(
                    world,
                    effect,
                    level,
                    entity,
                    enchanted_entity,
                    enchanted_item,
                );
            }
            enchanted_item_broke
        }
        EnchantmentEntityEffect::ChangeItemDamage { amount } => {
            let amount = amount.calculate(level) as i32;
            let has_infinite_materials = enchanted_entity
                .and_then(|entity| entity.as_living_entity())
                .is_some_and(LivingEntity::has_infinite_materials);
            enchanted_item.hurt_and_break(amount, has_infinite_materials)
        }
        EnchantmentEntityEffect::DamageEntity {
            min_damage,
            max_damage,
            damage_type,
        } => {
            let min_damage = min_damage.calculate(level);
            let max_damage = max_damage.calculate(level);
            let damage = random_between(min_damage, max_damage);
            let source = match enchanted_entity {
                Some(entity) => DamageSource::direct(damage_type, Arc::clone(entity)),
                None => DamageSource::environment(damage_type),
            };
            entity.hurt(world, &source, damage);
            false
        }
        EnchantmentEntityEffect::Ignite { duration } => {
            let ticks = (duration.calculate(level) * 20.0).floor() as i32;
            entity.ignite_for_ticks(ticks);
            false
        }
        EnchantmentEntityEffect::SummonEntity {
            entity_types,
            join_team,
        } => {
            summon_entity_effect(entity, entity_types, *join_team, enchanted_entity);
            false
        }
        EnchantmentEntityEffect::PlaySound {
            sounds,
            volume,
            pitch,
        } => {
            if entity.is_silent() || sounds.is_empty() {
                return false;
            }
            // Vanilla `PlaySoundEffect.apply` clamps `level - 1` into the sound list.
            let index = (level - 1).clamp(0, sounds.len() as i32 - 1) as usize;
            let mut random = WorldgenRandom::from_seed(rand::random());
            let volume = volume.sample(&mut random);
            let pitch = pitch.sample(&mut random);
            world.play_sound_at(
                sounds[index],
                entity.sound_source(),
                entity.position(),
                volume,
                pitch,
                None,
            );
            false
        }
        EnchantmentEntityEffect::ApplyMobEffect {
            to_apply: MobEffectSelection::Single(effect),
            min_duration,
            max_duration,
            min_amplifier,
            max_amplifier,
        } => {
            let Some(living) = entity.as_living_entity() else {
                return false;
            };
            let min_duration = min_duration.calculate(level);
            let max_duration = max_duration.calculate(level);
            let min_amplifier = min_amplifier.calculate(level);
            let max_amplifier = max_amplifier.calculate(level);
            let duration_seconds = random_between(min_duration, max_duration);
            let amplifier = random_between(min_amplifier, max_amplifier);
            let duration_ticks = java_round(duration_seconds * 20.0);
            let amplifier = java_round(amplifier).max(0);
            living.add_mob_effect(MobEffectInstance::with_duration(
                effect,
                duration_ticks,
                amplifier,
            ));
            false
        }
        EnchantmentEntityEffect::ReplaceDisk {
            radius,
            height,
            offset,
            predicate,
            block_state,
            trigger_game_event,
        } => {
            apply_replace_disk(
                level,
                entity,
                radius,
                height,
                *offset,
                *predicate,
                block_state,
                *trigger_game_event,
            );
            false
        }
        EnchantmentEntityEffect::ApplyMobEffect { .. }
        | EnchantmentEntityEffect::ApplyExhaustion { .. }
        | EnchantmentEntityEffect::ApplyImpulse { .. }
        | EnchantmentEntityEffect::SpawnParticles { .. }
        | EnchantmentEntityEffect::Attribute(_)
        | EnchantmentEntityEffect::Unsupported { .. } => false,
    }
}

/// Vanilla `ReplaceDisk.apply`: replaces the blocks in a disk of `radius`
/// around the origin (already shifted by `offset`) with the resolved
/// `block_state`, gated on `predicate`. Frost Walker is the shipped consumer.
///
/// `height` extends the disk vertically from the center: only the
/// `0..=(height - 1).min(0)` layers below are touched (vanilla
/// `Math.min(height - 1, 0)`).
#[expect(
    clippy::too_many_arguments,
    reason = "single vanilla effect application; the arguments map one-to-one onto the six ReplaceDisk record fields"
)]
fn apply_replace_disk(
    enchantment_level: i32,
    entity: &dyn Entity,
    radius: &'static LevelBasedValue,
    height: &'static LevelBasedValue,
    offset: IVec3,
    predicate: Option<BlockPredicateRef>,
    block_state: &BlockStateData,
    trigger_game_event: Option<GameEventRef>,
) {
    let Some(level) = entity.level() else {
        return;
    };
    let origin = entity.position();
    let center =
        BlockPos::containing(origin.x, origin.y, origin.z).offset(offset.x, offset.y, offset.z);
    let radius = radius.calculate(enchantment_level) as i32;
    let max_layer = (height.calculate(enchantment_level) as i32 - 1).min(0);
    let state = BlockStateProviderEvaluator::block_state_from_data(&REGISTRY, block_state);

    for dx in -radius..=radius {
        for dy in 0..=max_layer {
            for dz in -radius..=radius {
                let pos = center.offset(dx, dy, dz);
                let dx = f64::from(pos.x()) + 0.5 - origin.x;
                let dz = f64::from(pos.z()) + 0.5 - origin.z;
                // Vanilla `p.distToCenterSqr(origin.x, p.y + 0.5, origin.z) <
                // Mth.square(radius)`: the y terms cancel, leaving the
                // horizontal circle around the entity.
                if dx.mul_add(dx, dz * dz) >= f64::from(radius * radius) {
                    continue;
                }
                if predicate.is_some_and(|predicate| {
                    !BlockStateProviderEvaluator::test_block_predicate(
                        &level, &REGISTRY, predicate, pos,
                    )
                }) {
                    continue;
                }
                if level.set_block(pos, state, UpdateFlags::UPDATE_ALL)
                    && let Some(event) = trigger_game_event
                {
                    level.game_event(event, pos, &GameEventContext::new(Some(entity), None));
                }
            }
        }
    }
}

fn apply_post_piercing_entity_effect(
    world: &World,
    effect: &EnchantmentEntityEffect,
    level: i32,
    user: &dyn LivingEntity,
) -> bool {
    if !post_piercing_entity_effect_is_supported(effect) {
        return false;
    }

    apply_supported_post_piercing_entity_effect(world, effect, level, user)
}

fn post_piercing_entity_effect_is_supported(effect: &EnchantmentEntityEffect) -> bool {
    match effect {
        EnchantmentEntityEffect::AllOf(effects) => effects
            .iter()
            .all(|effect| post_piercing_entity_effect_is_supported(effect)),
        EnchantmentEntityEffect::ChangeItemDamage { .. }
        | EnchantmentEntityEffect::ApplyExhaustion { .. }
        | EnchantmentEntityEffect::Ignite { .. } => true,
        EnchantmentEntityEffect::PlaySound { sounds, .. } => !sounds.is_empty(),
        EnchantmentEntityEffect::ApplyImpulse { direction, .. } => {
            direction.x == 0.0 && direction.y == 0.0
        }
        EnchantmentEntityEffect::ApplyMobEffect { to_apply, .. } => {
            matches!(to_apply, MobEffectSelection::Single(_))
        }
        EnchantmentEntityEffect::DamageEntity { .. }
        | EnchantmentEntityEffect::SummonEntity { .. }
        | EnchantmentEntityEffect::SpawnParticles { .. }
        | EnchantmentEntityEffect::ReplaceDisk { .. }
        | EnchantmentEntityEffect::Attribute(_)
        | EnchantmentEntityEffect::Unsupported { .. } => false,
    }
}

fn apply_supported_post_piercing_entity_effect(
    world: &World,
    effect: &EnchantmentEntityEffect,
    level: i32,
    user: &dyn LivingEntity,
) -> bool {
    match effect {
        EnchantmentEntityEffect::AllOf(effects) => {
            let mut enchanted_item_broke = false;
            for effect in *effects {
                enchanted_item_broke |=
                    apply_supported_post_piercing_entity_effect(world, effect, level, user);
            }
            enchanted_item_broke
        }
        EnchantmentEntityEffect::ChangeItemDamage { amount } => {
            let amount = amount.calculate(level) as i32;
            let has_infinite_materials = user.has_infinite_materials();
            let mut item_broke = false;
            user.with_equipment_slot_mut(EquipmentSlot::MainHand, &mut |stack| {
                item_broke = stack.hurt_and_break(amount, has_infinite_materials);
            });
            item_broke
        }
        EnchantmentEntityEffect::ApplyExhaustion { amount } => {
            if let Some(player) = user.as_player() {
                player.cause_food_exhaustion(amount.calculate(level));
            }
            false
        }
        EnchantmentEntityEffect::ApplyImpulse {
            direction,
            coordinate_scale,
            magnitude,
        } => {
            let impulse = (user.look_angle() * direction.z)
                * *coordinate_scale
                * f64::from(magnitude.calculate(level));
            user.push_impulse(impulse);
            user.apply_post_impulse_grace_time(10);
            false
        }
        EnchantmentEntityEffect::PlaySound {
            sounds,
            volume,
            pitch,
        } => {
            let index = (level - 1).clamp(0, sounds.len() as i32 - 1) as usize;
            let mut random = WorldgenRandom::from_seed(rand::random());
            let volume = volume.sample(&mut random);
            let pitch = pitch.sample(&mut random);
            user.play_sound(sounds[index], volume, pitch);
            false
        }
        EnchantmentEntityEffect::Ignite { .. } | EnchantmentEntityEffect::ApplyMobEffect { .. } => {
            let mut ignored_item = ItemStack::empty();
            apply_supported_entity_effect(
                world,
                effect,
                level,
                user,
                None,
                &mut EnchantedItemInUse::Stack(&mut ignored_item),
            )
        }
        EnchantmentEntityEffect::DamageEntity { .. }
        | EnchantmentEntityEffect::SummonEntity { .. }
        | EnchantmentEntityEffect::SpawnParticles { .. }
        | EnchantmentEntityEffect::ReplaceDisk { .. }
        | EnchantmentEntityEffect::Attribute(_)
        | EnchantmentEntityEffect::Unsupported { .. } => false,
    }
}

fn random_between(min: f32, max: f32) -> f32 {
    min + rand::random::<f32>() * (max - min)
}

fn java_round(value: f32) -> i32 {
    (value + 0.5).floor() as i32
}

/// Vanilla `SummonEntityEffect.apply`.
///
/// Only `minecraft:lightning_bolt` can be spawned: Steel has no generic
/// `EntityType.spawn` equivalent for arbitrary types, so a data set that
/// requests anything else spawns nothing and is reported as a named limitation
/// rather than faked. Rejects positions outside vanilla's spawn bounds and
/// resolves the set element exactly as `HolderSet.getRandomElement` does.
fn summon_entity_effect(
    entity: &dyn Entity,
    entity_types: &EntityTypeSet,
    join_team: bool,
    enchanted_entity: Option<&SharedEntity>,
) {
    let Some(world) = entity.level() else {
        return;
    };
    let pos = entity.block_position();
    if !World::is_in_spawnable_bounds(pos) {
        return;
    }
    let mut random = WorldgenRandom::from_seed(rand::random());
    let Some(entity_type) = entity_types.get_random_element(&mut random) else {
        return;
    };
    if entity_type != &vanilla_entities::LIGHTNING_BOLT {
        // Named limitation, not a silent drop.
        log::warn!(
            "summon_entity cannot spawn `{}` yet; only minecraft:lightning_bolt is supported",
            entity_type.key
        );
        return;
    }
    if join_team {
        // Vanilla adds the summoned entity to the enchanted item owner's team.
        // Steel's scoreboard supports manual team editing but entities carry no
        // team binding yet, so the flag is a documented no-op applied only when
        // data asks for it (no shipped data sets it today).
        log::debug!(
            "summon_entity joined the summoned lightning bolt to no team: join_team is not implemented"
        );
    }
    // Vanilla `LightningBolt.setCause(ServerPlayer)`.
    let cause = enchanted_entity
        .and_then(|owner| owner.as_player())
        .map(Entity::uuid);
    if let Some(bolt) = world.spawn_lightning_bolt_with_cause(pos, false, cause) {
        // Vanilla `Entity.snapTo` the effect position, keeping the fresh bolt's yaw/pitch.
        let (yaw, pitch) = bolt.rotation();
        bolt.snap_to(entity.position(), yaw, pitch);
    }
}

fn requirements_match(
    requirements: Option<&'static EnchantmentEffectRequirements>,
    context: &EnchantmentDamageContext<'_>,
) -> bool {
    let Some(requirements) = requirements else {
        return true;
    };

    matches!(requirements_state(requirements, context), Some(true))
}

fn post_attack_requirements_match(
    requirements: Option<&'static EnchantmentEffectRequirements>,
    context: &EnchantmentDamageContext<'_>,
    level: i32,
    weather: Option<WeatherSnapshot>,
) -> bool {
    let Some(requirements) = requirements else {
        return true;
    };

    matches!(
        requirements_state_with_random(requirements, context, level, weather),
        Some(true)
    )
}

fn requirements_state(
    requirements: &'static EnchantmentEffectRequirements,
    context: &EnchantmentDamageContext<'_>,
) -> Option<bool> {
    match requirements {
        EnchantmentEffectRequirements::AllOf(terms) => {
            let mut has_unknown = false;
            for term in *terms {
                match requirements_state(term, context) {
                    Some(true) => {}
                    Some(false) => return Some(false),
                    None => has_unknown = true,
                }
            }
            if has_unknown { None } else { Some(true) }
        }
        EnchantmentEffectRequirements::AnyOf(terms) => {
            let mut has_unknown = false;
            for term in *terms {
                match requirements_state(term, context) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => has_unknown = true,
                }
            }
            if has_unknown { None } else { Some(false) }
        }
        EnchantmentEffectRequirements::Inverted(term) => {
            requirements_state(term, context).map(|matched| !matched)
        }
        EnchantmentEffectRequirements::EntityProperties { entity, predicate } => {
            context.entity_type(*entity).and_then(|entity_type| {
                entity_predicate_matches_type(predicate, entity_type, context.entity(*entity))
            })
        }
        EnchantmentEffectRequirements::DamageSourceProperties(predicate) => Some(
            damage_source_predicate_matches(predicate, context.damage_source),
        ),
        EnchantmentEffectRequirements::RandomChance { .. }
        | EnchantmentEffectRequirements::WeatherCheck { .. }
        | EnchantmentEffectRequirements::MatchTool { .. }
        | EnchantmentEffectRequirements::Unsupported { .. } => None,
    }
}

fn requirements_state_with_random(
    requirements: &'static EnchantmentEffectRequirements,
    context: &EnchantmentDamageContext<'_>,
    level: i32,
    weather: Option<WeatherSnapshot>,
) -> Option<bool> {
    match requirements {
        EnchantmentEffectRequirements::AllOf(terms) => {
            let mut has_unknown = false;
            for term in *terms {
                match requirements_state_with_random(term, context, level, weather) {
                    Some(true) => {}
                    Some(false) => return Some(false),
                    None => has_unknown = true,
                }
            }
            if has_unknown { None } else { Some(true) }
        }
        EnchantmentEffectRequirements::AnyOf(terms) => {
            let mut has_unknown = false;
            for term in *terms {
                match requirements_state_with_random(term, context, level, weather) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => has_unknown = true,
                }
            }
            if has_unknown { None } else { Some(false) }
        }
        EnchantmentEffectRequirements::Inverted(term) => {
            requirements_state_with_random(term, context, level, weather).map(|matched| !matched)
        }
        EnchantmentEffectRequirements::EntityProperties { entity, predicate } => {
            context.entity_type(*entity).and_then(|entity_type| {
                entity_predicate_matches_type(predicate, entity_type, context.entity(*entity))
            })
        }
        EnchantmentEffectRequirements::DamageSourceProperties(predicate) => Some(
            damage_source_predicate_matches(predicate, context.damage_source),
        ),
        EnchantmentEffectRequirements::WeatherCheck {
            raining,
            thundering,
        } => weather.map(|weather| weather.matches(*raining, *thundering)),
        EnchantmentEffectRequirements::RandomChance { chance } => {
            Some(rand::random::<f32>() < chance.calculate(level))
        }
        EnchantmentEffectRequirements::MatchTool { .. }
        | EnchantmentEffectRequirements::Unsupported { .. } => None,
    }
}

fn entity_requirements_match(
    requirements: Option<&'static EnchantmentEffectRequirements>,
    entity: &dyn Entity,
    level: i32,
    weather: Option<WeatherSnapshot>,
) -> bool {
    let Some(requirements) = requirements else {
        return true;
    };

    matches!(
        entity_requirements_state(requirements, entity, level, weather),
        Some(true)
    )
}

fn entity_requirements_state(
    requirements: &'static EnchantmentEffectRequirements,
    entity: &dyn Entity,
    level: i32,
    weather: Option<WeatherSnapshot>,
) -> Option<bool> {
    match requirements {
        EnchantmentEffectRequirements::AllOf(terms) => {
            let mut has_unknown = false;
            for term in *terms {
                match entity_requirements_state(term, entity, level, weather) {
                    Some(true) => {}
                    Some(false) => return Some(false),
                    None => has_unknown = true,
                }
            }
            if has_unknown { None } else { Some(true) }
        }
        EnchantmentEffectRequirements::AnyOf(terms) => {
            let mut has_unknown = false;
            for term in *terms {
                match entity_requirements_state(term, entity, level, weather) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => has_unknown = true,
                }
            }
            if has_unknown { None } else { Some(false) }
        }
        EnchantmentEffectRequirements::Inverted(term) => {
            entity_requirements_state(term, entity, level, weather).map(|matched| !matched)
        }
        EnchantmentEffectRequirements::EntityProperties {
            entity: EnchantmentEntityTarget::This,
            predicate,
        } => entity_predicate_matches_entity(predicate, entity),
        EnchantmentEffectRequirements::WeatherCheck {
            raining,
            thundering,
        } => weather.map(|weather| weather.matches(*raining, *thundering)),
        EnchantmentEffectRequirements::RandomChance { chance } => {
            Some(rand::random::<f32>() < chance.calculate(level))
        }
        EnchantmentEffectRequirements::EntityProperties { .. }
        | EnchantmentEffectRequirements::DamageSourceProperties(_)
        | EnchantmentEffectRequirements::MatchTool { .. }
        | EnchantmentEffectRequirements::Unsupported { .. } => None,
    }
}

fn entity_predicate_matches_type(
    predicate: &EntityPredicate,
    entity_type: EntityTypeRef,
    entity: Option<&dyn Entity>,
) -> Option<bool> {
    if predicate.unsupported
        || !matches!(predicate.vehicle, EntityVehiclePredicate::Any)
        || predicate.flags.has_constraints()
    {
        return None;
    }

    let type_matches = entity_type_predicate_matches(&predicate.entity_type, entity_type)?;
    if !type_matches {
        return Some(false);
    }

    if let Some(location) = &predicate.location {
        let entity = entity?;
        if !entity_location_predicate_matches(location, entity)? {
            return Some(false);
        }
    }

    match &predicate.type_specific {
        EntityTypeSpecificPredicate::Any => Some(true),
        EntityTypeSpecificPredicate::Unsupported => None,
        EntityTypeSpecificPredicate::Player(player_predicate) => {
            if entity_type != &vanilla_entities::PLAYER {
                return Some(false);
            }
            if player_predicate.unsupported
                || !player_predicate.game_modes.is_empty()
                || player_predicate.food_level_min.is_some()
            {
                None
            } else {
                Some(true)
            }
        }
    }
}

fn entity_predicate_matches_entity(
    predicate: &EntityPredicate,
    entity: &dyn Entity,
) -> Option<bool> {
    if predicate.unsupported {
        return None;
    }

    if !entity_type_predicate_matches(&predicate.entity_type, entity.entity_type())? {
        return Some(false);
    }

    if predicate.flags.unsupported {
        return None;
    }
    if let Some(expected) = predicate.flags.is_on_ground
        && entity.on_ground() != expected
    {
        return Some(false);
    }
    if let Some(expected) = predicate.flags.is_fall_flying
        && entity
            .as_living_entity()
            .is_none_or(|living| living.is_fall_flying() != expected)
    {
        return Some(false);
    }
    if let Some(expected) = predicate.flags.is_in_water
        && entity.is_in_water() != expected
    {
        return Some(false);
    }

    match predicate.vehicle {
        EntityVehiclePredicate::Any => {}
        EntityVehiclePredicate::Present => {
            if entity.vehicle().is_none() {
                return Some(false);
            }
        }
        EntityVehiclePredicate::Unsupported => return None,
    }

    if predicate.flags.unsupported {
        return None;
    }
    if let Some(expected) = predicate.flags.is_fall_flying {
        let is_fall_flying = entity
            .as_living_entity()
            .is_some_and(LivingEntity::is_fall_flying);
        if is_fall_flying != expected {
            return Some(false);
        }
    }
    if let Some(expected) = predicate.flags.is_in_water
        && entity.is_in_water() != expected
    {
        return Some(false);
    }

    if let Some(location) = &predicate.location
        && !entity_location_predicate_matches(location, entity)?
    {
        return Some(false);
    }

    match &predicate.type_specific {
        EntityTypeSpecificPredicate::Any => Some(true),
        EntityTypeSpecificPredicate::Unsupported => None,
        EntityTypeSpecificPredicate::Player(player_predicate) => {
            let Some(player) = entity.as_player() else {
                return Some(false);
            };
            if player_predicate.unsupported {
                return None;
            }
            if !player_predicate.game_modes.is_empty()
                && !player_predicate
                    .game_modes
                    .iter()
                    .any(|game_mode| *game_mode == player.game_mode())
            {
                return Some(false);
            }
            if let Some(min_food_level) = player_predicate.food_level_min
                && player.food_data.lock().food_level < min_food_level
            {
                return Some(false);
            }
            Some(true)
        }
    }
}

/// Vanilla `EntityPredicate.test` location side: only `can_see_sky` is
/// supported yet. A constraint that cannot be evaluated against the entity's
/// live world stays unknown (`None`) so the fold keeps its fail-closed
/// three-valued semantics.
fn entity_location_predicate_matches(
    location: &EntityLocationPredicate,
    entity: &dyn Entity,
) -> Option<bool> {
    if location.unsupported {
        return None;
    }
    if let Some(expected) = location.can_see_sky {
        let world = entity.level()?;
        if world.can_see_sky(entity.block_position()) != expected {
            return Some(false);
        }
    }
    Some(true)
}

fn entity_type_predicate_matches(
    predicate: &EntityTypePredicate,
    entity_type: EntityTypeRef,
) -> Option<bool> {
    match predicate {
        EntityTypePredicate::Any => Some(true),
        EntityTypePredicate::Type(expected) => Some(entity_type.key == *expected),
        EntityTypePredicate::Tag(tag) => Some(REGISTRY.entity_types.is_in_tag(entity_type, tag)),
        EntityTypePredicate::Unsupported => None,
    }
}

fn damage_source_predicate_matches(
    predicate: &DamageSourcePredicate,
    damage_source: &DamageSource,
) -> bool {
    if let Some(is_direct) = predicate.is_direct
        && damage_source.is_direct() != is_direct
    {
        return false;
    }

    predicate
        .tags
        .iter()
        .all(|tag| damage_source.is(&tag.tag) == tag.expected)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::entity_type::EntityTypeRef;
    use steel_registry::items::ItemRef;
    use steel_registry::{
        init_vanilla_registry, vanilla_blocks, vanilla_damage_types, vanilla_enchantments,
        vanilla_entities, vanilla_items, vanilla_mob_effects,
    };
    use steel_utils::Identifier;
    use steel_utils::locks::SyncMutex;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::{
        EntityBase, LivingEntity, LivingEntityBase, entities::FireworkRocketEntity,
    };
    use crate::player::Player;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, enchanted_item, fresh_test_world, insert_ready_full_chunk,
        test_world,
    };
    use steel_registry::blocks::properties::BlockStateProperties;
    use steel_registry::blocks::properties::IntProperty;
    use steel_registry::blocks::{BlockRef, block_state_ext::BlockStateExt as _};
    use steel_utils::{ChunkPos, types::UpdateFlags};

    /// Frost Walker's `age` property.
    const AGE_3: &IntProperty = &BlockStateProperties::AGE_3;

    struct TestLivingEntity {
        base: EntityBase,
        living_base: LivingEntityBase,
        health: SyncMutex<f32>,
        broken_slots: SyncMutex<Vec<EquipmentSlot>>,
        entity_type: EntityTypeRef,
    }

    impl TestLivingEntity {
        fn new(id: i32, entity_type: EntityTypeRef) -> Self {
            Self::new_at(test_world(), id, entity_type, DVec3::ZERO)
        }

        fn new_at(
            world: &Arc<World>,
            id: i32,
            entity_type: EntityTypeRef,
            position: DVec3,
        ) -> Self {
            Self {
                base: EntityBase::new(id, position, entity_type.dimensions, Arc::downgrade(world)),
                living_base: LivingEntityBase::new(entity_type),
                health: SyncMutex::new(20.0),
                broken_slots: SyncMutex::new(Vec::new()),
                entity_type,
            }
        }

        fn equip(&self, slot: EquipmentSlot, stack: ItemStack) {
            self.living_base.equipment().lock().set(slot, stack);
        }

        fn broken_slots(&self) -> Vec<EquipmentSlot> {
            self.broken_slots.lock().clone()
        }
    }

    crate::entity::impl_test_downcast_type!(TestLivingEntity);

    impl Entity for TestLivingEntity {
        fn base(&self) -> &EntityBase {
            &self.base
        }

        fn entity_type(&self) -> EntityTypeRef {
            self.entity_type
        }

        fn with_weapon_item(&self, visitor: &mut dyn FnMut(Option<&ItemStack>)) {
            let equipment = self.living_base.equipment().try_lock().expect(
                "entity effects must release equipment locks before inspecting source weapons",
            );
            visitor(Some(equipment.get_ref(EquipmentSlot::MainHand)));
        }
    }

    impl LivingEntity for TestLivingEntity {
        fn living_base(&self) -> &LivingEntityBase {
            &self.living_base
        }

        fn get_health(&self) -> f32 {
            *self.health.lock()
        }

        fn set_health(&self, health: f32) {
            *self.health.lock() = health.clamp(0.0, self.get_max_health());
        }

        fn get_absorption_amount(&self) -> f32 {
            0.0
        }

        fn set_absorption_amount(&self, _amount: f32) {}

        fn on_equipped_item_broken(&self, _item: ItemRef, slot: EquipmentSlot) {
            self.broken_slots.lock().push(slot);
        }
    }

    fn assert_f32_eq(actual: f32, expected: f32) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "actual: {actual}, expected: {expected}"
        );
    }

    #[test]
    fn unsupported_requirement_does_not_match_through_inversion() {
        static UNSUPPORTED: EnchantmentEffectRequirements =
            EnchantmentEffectRequirements::Unsupported {
                condition: Identifier::vanilla_static("match_tool"),
            };
        static INVERTED: EnchantmentEffectRequirements =
            EnchantmentEffectRequirements::Inverted(&UNSUPPORTED);

        let damage_source = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK);
        let context = EnchantmentDamageContext::new(
            &vanilla_entities::PLAYER,
            Some(&vanilla_entities::PLAYER),
            Some(&vanilla_entities::PLAYER),
            &damage_source,
        );

        assert!(!requirements_match(Some(&UNSUPPORTED), &context));
        assert!(!requirements_match(Some(&INVERTED), &context));
    }

    #[test]
    fn damage_enchantments_match_target_entity_tags() {
        init_vanilla_registry();

        let stack = enchanted_item(
            &vanilla_items::DIAMOND_SWORD,
            Identifier::vanilla_static("smite"),
            5,
        );
        let damage_source = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK);
        let zombie_context = EnchantmentDamageContext::new(
            &vanilla_entities::ZOMBIE,
            Some(&vanilla_entities::PLAYER),
            Some(&vanilla_entities::PLAYER),
            &damage_source,
        );
        let spider_context = EnchantmentDamageContext::new(
            &vanilla_entities::SPIDER,
            Some(&vanilla_entities::PLAYER),
            Some(&vanilla_entities::PLAYER),
            &damage_source,
        );

        assert_f32_eq(modify_damage(&stack, &zombie_context, 7.0), 19.5);
        assert_f32_eq(modify_damage(&stack, &spider_context, 7.0), 7.0);
    }

    #[test]
    fn breach_modifies_armor_effectiveness() {
        init_vanilla_registry();

        let stack = enchanted_item(
            &vanilla_items::MACE,
            Identifier::vanilla_static("breach"),
            2,
        );
        let damage_source = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK);
        let context = EnchantmentDamageContext::new(
            &vanilla_entities::PLAYER,
            Some(&vanilla_entities::PLAYER),
            Some(&vanilla_entities::PLAYER),
            &damage_source,
        );

        assert_f32_eq(
            modify_armor_effectiveness(&stack, &context, 0.4),
            0.4_f32 - 0.15_f32 * 2.0,
        );
    }

    #[test]
    fn projectile_knockback_checks_direct_attacker_entity_tag() {
        init_vanilla_registry();

        let stack = enchanted_item(&vanilla_items::BOW, Identifier::vanilla_static("punch"), 2);
        let damage_source = DamageSource::environment(&vanilla_damage_types::ARROW);
        let melee_context = EnchantmentDamageContext::new(
            &vanilla_entities::ZOMBIE,
            Some(&vanilla_entities::PLAYER),
            Some(&vanilla_entities::PLAYER),
            &damage_source,
        );
        let arrow_context = EnchantmentDamageContext::new(
            &vanilla_entities::ZOMBIE,
            Some(&vanilla_entities::PLAYER),
            Some(&vanilla_entities::ARROW),
            &damage_source,
        );

        assert_f32_eq(modify_knockback(&stack, &melee_context, 0.0), 0.0);
        assert_f32_eq(modify_knockback(&stack, &arrow_context, 0.0), 2.0);
    }

    #[test]
    fn damage_source_properties_match_damage_type_tags() {
        init_vanilla_registry();

        let stack = enchanted_item(
            &vanilla_items::DIAMOND_SWORD,
            Identifier::vanilla_static("fire_protection"),
            4,
        );
        let fire_source = DamageSource::environment(&vanilla_damage_types::IN_FIRE);
        let fall_source = DamageSource::environment(&vanilla_damage_types::FALL);
        let fire_context =
            EnchantmentDamageContext::new(&vanilla_entities::PLAYER, None, None, &fire_source);
        let fall_context =
            EnchantmentDamageContext::new(&vanilla_entities::PLAYER, None, None, &fall_source);

        assert_f32_eq(
            apply_value_effects(
                &stack,
                EnchantmentEffectComponent::DamageProtection,
                &fire_context,
                0.0,
            ),
            8.0,
        );
        assert_f32_eq(
            apply_value_effects(
                &stack,
                EnchantmentEffectComponent::DamageProtection,
                &fall_context,
                0.0,
            ),
            0.0,
        );
    }

    #[test]
    fn damage_protection_accumulates_across_equipment() {
        init_vanilla_registry();

        let victim = TestLivingEntity::new(1, &vanilla_entities::PLAYER);
        victim.equip(
            EquipmentSlot::Feet,
            enchanted_item(
                &vanilla_items::DIAMOND_BOOTS,
                Identifier::vanilla_static("protection"),
                2,
            ),
        );
        victim.equip(
            EquipmentSlot::Chest,
            enchanted_item(
                &vanilla_items::DIAMOND_CHESTPLATE,
                Identifier::vanilla_static("blast_protection"),
                3,
            ),
        );
        let source = DamageSource::environment(&vanilla_damage_types::FIREWORKS);

        assert_f32_eq(get_damage_protection(&victim, &source), 8.0);
    }

    #[test]
    fn damage_protection_ignores_enchantment_in_non_matching_slot() {
        init_vanilla_registry();

        let victim = TestLivingEntity::new(1, &vanilla_entities::PLAYER);
        victim.equip(
            EquipmentSlot::MainHand,
            enchanted_item(
                &vanilla_items::DIAMOND_BOOTS,
                Identifier::vanilla_static("protection"),
                4,
            ),
        );
        let source = DamageSource::environment(&vanilla_damage_types::GENERIC);

        assert_f32_eq(get_damage_protection(&victim, &source), 0.0);
    }

    #[test]
    fn damage_protection_ignores_broken_equipment() {
        init_vanilla_registry();

        let mut broken_chestplate = enchanted_item(
            &vanilla_items::DIAMOND_CHESTPLATE,
            Identifier::vanilla_static("protection"),
            4,
        );
        broken_chestplate.set_count(0);
        let victim = TestLivingEntity::new(1, &vanilla_entities::PLAYER);
        victim.equip(EquipmentSlot::Chest, broken_chestplate);
        let source = DamageSource::environment(&vanilla_damage_types::GENERIC);

        assert_f32_eq(get_damage_protection(&victim, &source), 0.0);
    }

    #[test]
    fn damage_immunity_matches_equipment_slot_and_requirements() {
        init_vanilla_registry();

        let boots = enchanted_item(
            &vanilla_items::LEATHER_BOOTS,
            Identifier::vanilla_static("frost_walker"),
            1,
        );
        let victim = TestLivingEntity::new(1, &vanilla_entities::PLAYER);
        victim.equip(EquipmentSlot::Feet, boots);

        assert!(is_immune_to_damage(
            &victim,
            &DamageSource::environment(&vanilla_damage_types::HOT_FLOOR)
        ));
        assert!(!is_immune_to_damage(
            &victim,
            &DamageSource::environment(&vanilla_damage_types::IN_FIRE)
        ));

        let helmet = enchanted_item(
            &vanilla_items::LEATHER_HELMET,
            Identifier::vanilla_static("frost_walker"),
            1,
        );
        let wrong_slot_victim = TestLivingEntity::new(2, &vanilla_entities::PLAYER);
        wrong_slot_victim.equip(EquipmentSlot::Head, helmet);

        assert!(!is_immune_to_damage(
            &wrong_slot_victim,
            &DamageSource::environment(&vanilla_damage_types::HOT_FLOOR)
        ));
    }

    #[test]
    fn post_attack_ignite_applies_to_direct_melee_victim() {
        init_vanilla_registry();

        let attacker = Arc::new(TestLivingEntity::new(1, &vanilla_entities::PLAYER));
        let victim = Arc::new(TestLivingEntity::new(2, &vanilla_entities::ZOMBIE));
        let stack = enchanted_item(
            &vanilla_items::DIAMOND_SWORD,
            Identifier::vanilla_static("fire_aspect"),
            2,
        );
        let damage_source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::<TestLivingEntity>::clone(&attacker),
        );
        let victim_entity: SharedEntity = Arc::<TestLivingEntity>::clone(&victim);
        let context = EnchantmentPostAttackContext::new(&victim_entity, &damage_source);

        do_post_attack_effects_from_item(test_world(), &stack, &context);

        assert_eq!(victim.remaining_fire_ticks(), 160);
    }

    #[test]
    fn projectile_spawned_effect_ignites_firework() {
        init_vanilla_registry();

        let projectile = FireworkRocketEntity::new(
            &vanilla_entities::FIREWORK_ROCKET,
            2,
            DVec3::ZERO,
            Weak::new(),
        );
        let mut rocket = enchanted_item(
            &vanilla_items::FIREWORK_ROCKET,
            Identifier::vanilla_static("flame"),
            1,
        );

        on_projectile_spawned(test_world(), &mut rocket, &projectile, None);

        assert_eq!(projectile.remaining_fire_ticks(), 2_000);
    }

    #[test]
    fn post_attack_effects_match_enchantment_slot() {
        init_vanilla_registry();

        let attacker = Arc::new(TestLivingEntity::new(1, &vanilla_entities::PLAYER));
        let victim = Arc::new(TestLivingEntity::new(2, &vanilla_entities::ZOMBIE));
        let mut stack = enchanted_item(
            &vanilla_items::DIAMOND_SWORD,
            Identifier::vanilla_static("fire_aspect"),
            1,
        );
        let damage_source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::<TestLivingEntity>::clone(&attacker),
        );
        let victim_entity: SharedEntity = Arc::<TestLivingEntity>::clone(&victim);
        let context = EnchantmentPostAttackContext::new(&victim_entity, &damage_source);

        apply_post_attack_effects(
            test_world(),
            &mut EnchantedItemInUse::Stack(&mut stack),
            Some(EquipmentSlot::Head),
            EnchantmentTarget::Attacker,
            &context,
        );
        assert_eq!(victim.remaining_fire_ticks(), 0);

        apply_post_attack_effects(
            test_world(),
            &mut EnchantedItemInUse::Stack(&mut stack),
            Some(EquipmentSlot::MainHand),
            EnchantmentTarget::Attacker,
            &context,
        );
        assert_eq!(victim.remaining_fire_ticks(), 80);
    }

    #[test]
    fn post_attack_change_item_damage_calls_equipped_break_hook() {
        init_vanilla_registry();

        let attacker = Arc::new(TestLivingEntity::new(1, &vanilla_entities::ZOMBIE));
        let victim = Arc::new(TestLivingEntity::new(2, &vanilla_entities::PLAYER));
        let mut chestplate = enchanted_item(
            &vanilla_items::DIAMOND_CHESTPLATE,
            Identifier::vanilla_static("thorns"),
            10,
        );
        chestplate.set_damage_value(chestplate.get_max_damage() - 1);
        victim.equip(EquipmentSlot::Chest, chestplate);

        let damage_source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::<TestLivingEntity>::clone(&attacker),
        );
        let victim_entity: SharedEntity = Arc::<TestLivingEntity>::clone(&victim);
        let context = EnchantmentPostAttackContext::new(&victim_entity, &damage_source);
        let source = ItemStack::empty();

        do_post_attack_effects_with_item_source(test_world(), victim.as_ref(), &source, &context);

        let mut chestplate_broke = false;
        victim.with_equipment_slot(EquipmentSlot::Chest, &mut |stack| {
            chestplate_broke = stack.is_empty();
        });
        assert!(chestplate_broke);
        assert_eq!(victim.broken_slots(), vec![EquipmentSlot::Chest]);
    }

    #[test]
    fn lunge_post_piercing_requirements_and_effect_are_supported() {
        init_vanilla_registry();

        let user = TestLivingEntity::new(1, &vanilla_entities::ZOMBIE);
        let effects = vanilla_enchantments::LUNGE.effects.post_piercing_attack;
        assert_eq!(effects.len(), 1);
        assert!(entity_requirements_match(
            effects[0].requirements,
            &user,
            1,
            None
        ));
        assert!(post_piercing_entity_effect_is_supported(&effects[0].effect));
    }

    #[test]
    fn post_attack_play_sound_fires_unless_entity_is_silent() {
        use steel_registry::sound_event::SoundEventRef;
        use steel_registry::sound_events;
        use steel_utils::ChunkPos;
        use steel_utils::value_providers::FloatProvider;

        use crate::player::ResetReason;
        use crate::player::connection::PlayerConnection;
        use crate::test_support::{
            TestConnection, TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk,
        };

        static SOUNDS: &[SoundEventRef] = &[&sound_events::ITEM_TRIDENT_THUNDER];

        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("post_attack_play_sound");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let connection = TestConnection::default();
        let recorded = connection.sent_handle();
        let listener = TestPlayerBuilder::new(Arc::clone(world), "SoundListener", 1)
            .connection(Arc::new(PlayerConnection::Other(Box::new(connection))))
            .build();
        assert!(world.add_player(Arc::clone(&listener), ResetReason::InitialJoin));
        let sent_since_join = || recorded.lock().len();
        let packets_before_action = sent_since_join();

        let effect = EnchantmentEntityEffect::PlaySound {
            sounds: SOUNDS,
            volume: FloatProvider::Constant(5.0),
            pitch: FloatProvider::Constant(1.0),
        };
        assert!(entity_effect_is_supported(&effect));

        let mut item = ItemStack::empty();
        let mut enchanted_item = EnchantedItemInUse::Stack(&mut item);
        let source = TestLivingEntity::new_at(world, 2, &vanilla_entities::ZOMBIE, DVec3::ZERO);
        apply_supported_entity_effect(world, &effect, 1, &source, None, &mut enchanted_item);
        assert!(
            sent_since_join() > packets_before_action,
            "a non-silent entity should broadcast the enchantment sound"
        );

        let packets_before_silence = sent_since_join();
        source.set_silent(true);
        apply_supported_entity_effect(world, &effect, 1, &source, None, &mut enchanted_item);
        assert_eq!(
            sent_since_join(),
            packets_before_silence,
            "a silent entity should suppress the enchantment sound"
        );
    }

    #[test]
    fn weather_check_matches_each_flag_against_the_snapshot() {
        static REQUIREMENTS: EnchantmentEffectRequirements =
            EnchantmentEffectRequirements::WeatherCheck {
                raining: None,
                thundering: Some(true),
            };

        let snapshot = Some(WeatherSnapshot {
            raining: false,
            thundering: true,
        });
        assert!(post_attack_requirements_match(
            Some(&REQUIREMENTS),
            &EnchantmentDamageContext::new(
                &vanilla_entities::TRIDENT,
                None,
                None,
                &DamageSource::environment(&vanilla_damage_types::TRIDENT)
            ),
            1,
            snapshot
        ));
        let calm = None;
        assert!(!post_attack_requirements_match(
            Some(&REQUIREMENTS),
            &EnchantmentDamageContext::new(
                &vanilla_entities::TRIDENT,
                None,
                None,
                &DamageSource::environment(&vanilla_damage_types::TRIDENT)
            ),
            1,
            calm
        ));
    }

    #[test]
    fn channeling_summons_lightning_with_thrower_as_cause_only_while_thundering() {
        use steel_utils::ChunkPos;
        use uuid::Uuid;

        use steel_utils::Downcast as _;

        use crate::entity::entities::LightningBoltEntity;
        use crate::entity::init_entities;
        use crate::player::Player;
        use crate::test_support::{
            TestEntity, TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk,
        };
        use steel_utils::WorldAabb;

        init_vanilla_registry();
        init_behaviors();
        init_entities();

        for thundering in [true, false] {
            let world_fixture = fresh_test_world(if thundering {
                "channeling_thunder"
            } else {
                "channeling_calm"
            });
            let world = &world_fixture.world;
            insert_ready_full_chunk(world, ChunkPos::new(0, 0));

            {
                let mut weather = world.weather.lock();
                weather.rain_level = 1.0;
                weather.thunder_level = if thundering { 1.0 } else { 0.0 };
            }

            let thrower =
                TestPlayerBuilder::new(Arc::clone(world), format!("Channeler{thundering}"), 10)
                    .uuid(Uuid::new_v4())
                    .build();
            let thrower_uuid = thrower.uuid();
            let thrower_entity: SharedEntity = Arc::<Player>::clone(&thrower);
            let victim = Arc::new(TestLivingEntity::new_at(
                world,
                11,
                &vanilla_entities::ZOMBIE,
                DVec3::new(8.0, 65.0, 8.0),
            ));
            let victim_entity: SharedEntity = Arc::<TestLivingEntity>::clone(&victim);
            let trident = TestEntity::shared(
                12,
                DVec3::new(8.0, 65.0, 8.0),
                Arc::downgrade(world),
                &vanilla_entities::TRIDENT,
            );
            let damage_source = DamageSource::environment(&vanilla_damage_types::TRIDENT)
                .with_causing_entity(thrower_entity)
                .with_direct_entity(trident);
            let context = EnchantmentPostAttackContext::new(&victim_entity, &damage_source);

            let trident_stack = enchanted_item(
                &vanilla_items::TRIDENT,
                Identifier::vanilla_static("channeling"),
                1,
            );
            do_post_attack_effects_with_item_source(
                world,
                victim.as_ref(),
                &trident_stack,
                &context,
            );

            let bounds = WorldAabb::of_size(DVec3::new(8.0, 65.0, 8.0), 4.0, 4.0, 4.0);
            let bolts = world.get_entities_in_aabb_matching(&bounds, |entity| {
                entity.entity_type() == &vanilla_entities::LIGHTNING_BOLT
            });
            if thundering {
                assert_eq!(bolts.len(), 1, "channeling should strike during thunder");
                let Some(bolt) = bolts[0].as_ref().downcast_ref::<LightningBoltEntity>() else {
                    panic!("summoned entity should be a lightning bolt");
                };
                assert_eq!(bolt.cause(), Some(thrower_uuid));
            } else {
                assert!(
                    bolts.is_empty(),
                    "the weather_check gate must suppress channeling without thunder"
                );
            }
        }
    }

    #[test]
    fn post_attack_ignite_skips_indirect_damage_source() {
        init_vanilla_registry();

        let attacker = Arc::new(TestLivingEntity::new(1, &vanilla_entities::PLAYER));
        let direct_entity = Arc::new(TestLivingEntity::new(2, &vanilla_entities::PLAYER));
        let victim = Arc::new(TestLivingEntity::new(3, &vanilla_entities::ZOMBIE));
        let stack = enchanted_item(
            &vanilla_items::DIAMOND_SWORD,
            Identifier::vanilla_static("fire_aspect"),
            2,
        );
        let damage_source = DamageSource::environment(&vanilla_damage_types::ARROW)
            .with_causing_entity(Arc::<TestLivingEntity>::clone(&attacker))
            .with_direct_entity(Arc::<TestLivingEntity>::clone(&direct_entity));
        let victim_entity: SharedEntity = Arc::<TestLivingEntity>::clone(&victim);
        let context = EnchantmentPostAttackContext::new(&victim_entity, &damage_source);

        do_post_attack_effects_from_item(test_world(), &stack, &context);

        assert_eq!(victim.remaining_fire_ticks(), 0);
    }

    #[test]
    fn post_attack_mob_effect_matches_victim_predicate() {
        init_vanilla_registry();
        init_behaviors();

        let attacker = Arc::new(TestLivingEntity::new(1, &vanilla_entities::PLAYER));
        let spider = Arc::new(TestLivingEntity::new(2, &vanilla_entities::SPIDER));
        let zombie = Arc::new(TestLivingEntity::new(3, &vanilla_entities::ZOMBIE));
        let stack = enchanted_item(
            &vanilla_items::DIAMOND_SWORD,
            Identifier::vanilla_static("bane_of_arthropods"),
            1,
        );
        let damage_source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::<TestLivingEntity>::clone(&attacker),
        );
        let spider_entity: SharedEntity = Arc::<TestLivingEntity>::clone(&spider);
        let spider_context = EnchantmentPostAttackContext::new(&spider_entity, &damage_source);
        let zombie_entity: SharedEntity = Arc::<TestLivingEntity>::clone(&zombie);
        let zombie_context = EnchantmentPostAttackContext::new(&zombie_entity, &damage_source);

        do_post_attack_effects_from_item(test_world(), &stack, &spider_context);
        do_post_attack_effects_from_item(test_world(), &stack, &zombie_context);

        let Some(slowness) = spider.mob_effect(vanilla_mob_effects::SLOWNESS) else {
            panic!("bane of arthropods should apply slowness to spiders");
        };
        assert_eq!(slowness.duration(), 30);
        assert_eq!(slowness.amplifier(), 3);
        assert!(zombie.mob_effect(vanilla_mob_effects::SLOWNESS).is_none());
    }

    #[test]
    fn looting_raises_the_equipment_drop_chance_of_the_killer() {
        init_vanilla_registry();

        let killer = Arc::new(TestLivingEntity::new(1, &vanilla_entities::PLAYER));
        killer.equip(
            EquipmentSlot::MainHand,
            enchanted_item(
                &vanilla_items::DIAMOND_SWORD,
                Identifier::vanilla_static("looting"),
                3,
            ),
        );
        let source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::<TestLivingEntity>::clone(&killer),
        );

        // Vanilla `looting`: `add` of `0.01 + 0.01 * (level - 1)`, so level 3 adds 0.03.
        assert_f32_eq(
            modify_equipment_drop_chance(killer.as_ref(), &source, 0.1),
            0.13,
        );
    }

    #[test]
    fn equipment_drop_chance_ignores_the_victims_own_enchantments() {
        init_vanilla_registry();

        // Vanilla `Mob.dropCustomDeathLoot` hands `EnchantmentHelper`
        // `processEquipmentDropChance` the *killer*, so only the killer's
        // equipment is ever folded; the dying mob's looting sword changes nothing.
        let victim = Arc::new(TestLivingEntity::new(2, &vanilla_entities::ZOMBIE));
        victim.equip(
            EquipmentSlot::MainHand,
            enchanted_item(
                &vanilla_items::DIAMOND_SWORD,
                Identifier::vanilla_static("looting"),
                3,
            ),
        );
        let killer = Arc::new(TestLivingEntity::new(1, &vanilla_entities::PLAYER));
        let source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::<TestLivingEntity>::clone(&killer),
        );

        assert_f32_eq(
            modify_equipment_drop_chance(killer.as_ref(), &source, 0.1),
            0.1,
        );
    }

    #[test]
    fn equipment_drop_chance_without_a_living_attacker_is_unchanged() {
        init_vanilla_registry();

        let victim = Arc::new(TestLivingEntity::new(2, &vanilla_entities::ZOMBIE));
        let source = DamageSource::environment(&vanilla_damage_types::GENERIC);

        assert_f32_eq(
            modify_equipment_drop_chance(victim.as_ref(), &source, 0.25),
            0.25,
        );
    }

    #[test]
    fn mob_experience_is_unchanged_by_vanilla_enchantments() {
        init_vanilla_registry();

        // No built-in enchantment carries a `minecraft:mob_experience` effect, so
        // vanilla's `EnchantmentHelper.processMobExperience` is the identity for
        // them; datapack enchantments may change it.
        let killer = Arc::new(TestLivingEntity::new(1, &vanilla_entities::PLAYER));
        killer.equip(
            EquipmentSlot::MainHand,
            enchanted_item(
                &vanilla_items::DIAMOND_SWORD,
                Identifier::vanilla_static("looting"),
                3,
            ),
        );
        let victim = Arc::new(TestLivingEntity::new(2, &vanilla_entities::ZOMBIE));

        assert_eq!(
            process_mob_experience(killer.as_ref(), victim.as_ref(), 5),
            5
        );

        // A non-living killer never folds anything (vanilla returns `amount`).
        let rocket = FireworkRocketEntity::new(
            &vanilla_entities::FIREWORK_ROCKET,
            3,
            DVec3::ZERO,
            Weak::new(),
        );
        assert_eq!(process_mob_experience(&rocket, victim.as_ref(), 7), 7);
    }

    /// Frost Walker's vanilla disk is measured from the *entity position*, not
    /// the block center: a player standing at `(8.0, 64.0, 8.0)` has the disk
    /// centered on that corner, so radius `r` covers the block offsets whose
    /// `(dx + 0.5)² + (dz + 0.5)² < r²` (vanilla
    /// `ReplaceDisk.apply`'s `distToCenterSqr(origin.x, p.y + 0.5, origin.z)
    /// < Mth.square(radius)`).
    ///
    /// The ground is passed as a `BlockRef`, never a `BlockStateId`: resolving a
    /// default state needs the published registry, and an argument is evaluated
    /// before this function runs `init_vanilla_registry`.
    fn frost_walker_world(
        key: &'static str,
        ground: BlockRef,
    ) -> (TestWorld, Arc<Player>, BlockPos) {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world(key);
        let world = &world_fixture.world;
        let center = BlockPos::new(8, 63, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(center));
        let ground = ground.default_state();
        for dx in -6..=6 {
            for dz in -6..=6 {
                let pos = center.offset(dx, 0, dz);
                assert!(
                    world.set_block(pos, ground, UpdateFlags::UPDATE_NONE),
                    "set_block at {pos:?} (ground layer)"
                );
                // An empty section already reads as air, and rewriting a state
                // into an all-air section reports "nothing changed" (vanilla
                // `LevelChunk.setBlockState` returns null there), so the layer
                // above is asserted by read instead of written.
                assert_eq!(
                    world.get_block_state(pos.above()),
                    vanilla_blocks::AIR.default_state(),
                    "the block above {pos:?} must be air for the Frost Walker predicate"
                );
            }
        }

        let player = TestPlayerBuilder::new(Arc::clone(world), "FrostWalker", 1).build();
        assert!(
            player.try_set_position(DVec3::new(8.0, 64.0, 8.0)).is_ok(),
            "test player should stand on the water surface"
        );
        player.set_on_ground(true);

        (world_fixture, player, center)
    }

    fn equip_frost_walker(player: &Player, level: u32) {
        player.living_base().equipment().lock().set(
            EquipmentSlot::Feet,
            enchanted_item(
                &vanilla_items::DIAMOND_BOOTS,
                Identifier::vanilla_static("frost_walker"),
                level,
            ),
        );
    }

    fn is_water(world: &Arc<World>, pos: BlockPos) -> bool {
        world.get_block_state(pos) == vanilla_blocks::WATER.default_state()
    }

    fn frosted_ice_age(world: &Arc<World>, pos: BlockPos) -> Option<u8> {
        let state = world.get_block_state(pos);
        (state.get_block() == &vanilla_blocks::FROSTED_ICE).then(|| state.get_value(AGE_3))
    }

    #[test]
    fn frost_walker_replaces_water_in_the_vanilla_disk() {
        let (world_fixture, player, center) =
            frost_walker_world("frost_walker_disk", &vanilla_blocks::WATER);
        let world = &world_fixture.world;
        equip_frost_walker(&player, 1);

        // The predicate needs air above the water: a covered water block stays
        // water even inside the disk (offset (2, 1) is inside the level I circle).
        let covered = center.offset(2, 0, 1);
        assert!(world.set_block(
            covered.above(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE
        ));

        run_location_changed_effects(player.as_ref(), world);

        // Radius 3 at level I: offsets -3..=2 survive the `(dx + 0.5)² +
        // (dz + 0.5)² < 9` circle, and every replaced block is fresh ice.
        for (dx, dz) in [(0, 0), (-3, 0), (2, 0), (0, -3), (0, 2), (-2, -2), (1, 1)] {
            let pos = center.offset(dx, 0, dz);
            assert_eq!(
                frosted_ice_age(world, pos),
                Some(0),
                "water at offset ({dx}, {dz}) should become fresh frosted ice"
            );
            // Vanilla `FrostedIceBlock.onPlace` arms the 60..120 tick revert.
            assert!(
                world.has_scheduled_block_tick(pos, &vanilla_blocks::FROSTED_ICE),
                "placed ice at offset ({dx}, {dz}) should have its melt tick armed"
            );
        }

        for (dx, dz) in [(-4, 0), (3, 0), (0, -4), (0, 3), (2, 2), (3, 1)] {
            assert!(
                is_water(world, center.offset(dx, 0, dz)),
                "water at offset ({dx}, {dz}) is outside the level I disk"
            );
        }

        // The covered water block is inside the disk but keeps its water.
        assert!(
            is_water(world, covered),
            "water with a block above it must not be frozen"
        );
    }

    #[test]
    fn frost_walker_disk_grows_with_level() {
        let (world_fixture, player, center) =
            frost_walker_world("frost_walker_disk_level", &vanilla_blocks::WATER);
        let world = &world_fixture.world;
        equip_frost_walker(&player, 2);

        run_location_changed_effects(player.as_ref(), world);

        // Level II radius is `linear(base 3, per_level_above_first 1)` = 4:
        // offset -4 is inside `20.25 + 0.25 < 16`... no: (-3.5)² + 0.25 = 12.5.
        assert_eq!(frosted_ice_age(world, center.offset(-4, 0, 0)), Some(0));
        assert!(is_water(world, center.offset(-5, 0, 0)));
    }

    #[test]
    fn frost_walker_does_nothing_on_land() {
        let (world_fixture, player, center) =
            frost_walker_world("frost_walker_land", &vanilla_blocks::STONE);
        let world = &world_fixture.world;
        equip_frost_walker(&player, 1);

        run_location_changed_effects(player.as_ref(), world);

        for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, 1)] {
            let pos = center.offset(dx, 0, dz);
            assert_eq!(
                world.get_block_state(pos),
                vanilla_blocks::STONE.default_state(),
                "stone at offset ({dx}, {dz}) must be untouched"
            );
        }
    }

    #[test]
    fn frost_walker_requires_the_wearer_to_be_on_ground() {
        let (world_fixture, player, center) =
            frost_walker_world("frost_walker_airborne", &vanilla_blocks::WATER);
        let world = &world_fixture.world;
        equip_frost_walker(&player, 1);
        player.set_on_ground(false);

        run_location_changed_effects(player.as_ref(), world);

        assert!(
            is_water(world, center),
            "an airborne wearer must not freeze any water"
        );
    }

    #[test]
    fn frost_walker_requires_the_boots_to_be_worn_in_the_feet_slot() {
        let (world_fixture, player, center) =
            frost_walker_world("frost_walker_slot", &vanilla_blocks::WATER);
        let world = &world_fixture.world;
        player.living_base().equipment().lock().set(
            EquipmentSlot::Head,
            enchanted_item(
                &vanilla_items::DIAMOND_HELMET,
                Identifier::vanilla_static("frost_walker"),
                1,
            ),
        );

        run_location_changed_effects(player.as_ref(), world);

        assert!(is_water(world, center));
    }

    #[test]
    fn location_changed_valve_fires_only_after_moving_into_a_new_block() {
        let (_world_fixture, player, center) =
            frost_walker_world("frost_walker_valve", &vanilla_blocks::WATER);

        assert!(
            player.living_base().update_last_changed_block_pos(center),
            "the first step always runs the effects (vanilla `lastPos` starts null)"
        );
        assert!(
            !player.living_base().update_last_changed_block_pos(center),
            "standing in the same block must not re-run the effects"
        );
        assert!(
            player
                .living_base()
                .update_last_changed_block_pos(center.offset(1, 0, 0)),
            "stepping into a new block re-arms the effects"
        );
    }
}

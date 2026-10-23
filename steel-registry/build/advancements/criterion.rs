//! Build-time parser for advancement criteria.
//!
//! Every vanilla criterion is turned into a typed
//! [`CriterionTriggerInstance`](crate::advancement::criterion::CriterionTriggerInstance)
//! emitted next to its advancement. The JSON model uses `deny_unknown_fields`
//! everywhere, so an unmodelled predicate fails the build instead of silently
//! degrading to `minecraft:impossible`.

use heck::{ToShoutySnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use steel_utils::Identifier;

/// One criterion of an advancement, as stored in the advancement JSON.
#[derive(Deserialize)]
pub(crate) struct CriterionJson {
    pub(crate) trigger: Identifier,
    #[serde(default)]
    pub(crate) conditions: Value,
}

/// The generated tokens for one advancement's criteria.
pub(crate) struct GeneratedCriteria {
    /// Module-level statics referenced by the criteria (arrays and sub-predicates).
    pub(crate) items: TokenStream,
    /// The `BTreeMap<String, Box<dyn AnyCriterion>>` expression.
    pub(crate) map: TokenStream,
}

impl CriterionJson {
    /// Emits the criterion's statics into `items` and returns its map value.
    fn generate(&self, adv_const: &Ident, name: &str, items: &mut TokenStream) -> TokenStream {
        let mut generator = CriterionGenerator::new(format!(
            "CRITERION_{}_{}",
            adv_const,
            name.to_shouty_snake_case()
        ));
        let value = self.generate_value(&mut generator);
        items.extend(generator.items);
        value
    }

    /// The `conditions` object; vanilla omits it (JSON `null`) when a criterion
    /// has no conditions.
    fn conditions_value(&self) -> Value {
        if self.conditions.is_null() {
            Value::Object(Map::new())
        } else {
            self.conditions.clone()
        }
    }

    fn generate_value(&self, generator: &mut CriterionGenerator) -> TokenStream {
        validate_conditions(&self.conditions);
        if self.trigger.path.as_ref() == "impossible" {
            assert_eq!(
                self.trigger.namespace.as_ref(),
                "minecraft",
                "impossible trigger must be a vanilla id"
            );
            return quote! {
                Box::new(Criterion::impossible()) as Box<dyn AnyCriterion>
            };
        }

        let trigger = trigger_static(&self.trigger);
        let (instance_type, instance) = self.instance(generator);
        quote! {
            Box::new(Criterion::<#instance_type, _>::new(&#trigger, #instance))
                as Box<dyn AnyCriterion>
        }
    }

    /// Parses this criterion's `conditions` into the trigger's instance type and
    /// returns the type path plus the constructor expression.
    fn instance(&self, generator: &mut CriterionGenerator) -> (TokenStream, TokenStream) {
        let trigger = self.trigger.path.as_ref();
        macro_rules! conditions {
            ($ty:ty) => {{
                let conditions: $ty = serde_json::from_value(self.conditions_value())
                    .unwrap_or_else(|error| {
                        panic!(
                            "advancement criterion with trigger `{}` has conditions Steel cannot \
                             model: {error}\nconditions: {}",
                            self.trigger, self.conditions
                        )
                    });
                conditions
            }};
        }

        match trigger {
            "inventory_changed" => {
                let json: InventoryChangedJson = conditions!(InventoryChangedJson);
                let player = generator.player(json.player);
                let slots = json.slots.map_or_else(
                    || {
                        quote! {
                            InventoryChangedSlots {
                                occupied: IntBounds::ANY,
                                full: IntBounds::ANY,
                                empty: IntBounds::ANY,
                            }
                        }
                    },
                    |slots| {
                        let occupied = generator.int_bounds(slots.occupied);
                        let full = generator.int_bounds(slots.full);
                        let empty = generator.int_bounds(slots.empty);
                        quote! {
                            InventoryChangedSlots { occupied: #occupied, full: #full, empty: #empty }
                        }
                    },
                );
                let items = generator.item_predicates(json.items);
                (
                    quote! { InventoryChangedTriggerInstance },
                    quote! {
                        InventoryChangedTriggerInstance {
                            player: #player,
                            slots: #slots,
                            items: #items,
                        }
                    },
                )
            }
            "recipe_unlocked" => {
                let json: RecipeUnlockedJson = conditions!(RecipeUnlockedJson);
                let player = generator.player(json.player);
                let recipes = generator.holder_set(&json.recipes);
                (
                    quote! { RecipeUnlockedTriggerInstance },
                    quote! {
                        RecipeUnlockedTriggerInstance { player: #player, recipes: #recipes }
                    },
                )
            }
            "recipe_crafted" | "crafter_recipe_crafted" => {
                let json: RecipeCraftedJson = conditions!(RecipeCraftedJson);
                let player = generator.player(json.player);
                let recipes = generator.holder_set(&json.recipes);
                let ingredients = generator.item_predicates(json.ingredients);
                (
                    quote! { RecipeCraftedTriggerInstance },
                    quote! {
                        RecipeCraftedTriggerInstance {
                            player: #player,
                            recipes: #recipes,
                            ingredients: #ingredients,
                        }
                    },
                )
            }
            "player_killed_entity" | "entity_killed_player" | "kill_mob_near_sculk_catalyst" => {
                let json: KilledJson = conditions!(KilledJson);
                let player = generator.player(json.player);
                let entity = generator.optional_condition(json.entity);
                let killing_blow = generator.optional_damage_source(json.killing_blow);
                (
                    quote! { KilledTriggerInstance },
                    quote! {
                        KilledTriggerInstance {
                            player: #player,
                            entity: #entity,
                            killing_blow: #killing_blow,
                        }
                    },
                )
            }
            "location" | "slept_in_bed" | "tick" | "hero_of_the_village" | "voluntary_exile"
            | "avoid_vibration" => {
                let json: PlayerOnlyJson = conditions!(PlayerOnlyJson);
                let player = generator.player(json.player);
                (
                    quote! { PlayerTriggerInstance },
                    quote! { PlayerTriggerInstance { player: #player } },
                )
            }
            "started_riding" => {
                let json: PlayerOnlyJson = conditions!(PlayerOnlyJson);
                let player = generator.player(json.player);
                (
                    quote! { StartRidingTriggerInstance },
                    quote! { StartRidingTriggerInstance { player: #player } },
                )
            }
            "consume_item" | "filled_bucket" | "used_totem" | "using_item" | "shot_crossbow" => {
                let json: ItemJson = conditions!(ItemJson);
                let player = generator.player(json.player);
                let item = generator.optional_item_predicate(json.item);
                (
                    quote! { ItemTriggerInstance },
                    quote! { ItemTriggerInstance { player: #player, item: #item } },
                )
            }
            "bred_animals" => {
                let json: BredAnimalsJson = conditions!(BredAnimalsJson);
                let player = generator.player(json.player);
                let parent = generator.optional_condition(json.parent);
                let partner = generator.optional_condition(json.partner);
                let child = generator.optional_condition(json.child);
                (
                    quote! { BredAnimalsTriggerInstance },
                    quote! {
                        BredAnimalsTriggerInstance {
                            player: #player,
                            parent: #parent,
                            partner: #partner,
                            child: #child,
                        }
                    },
                )
            }
            "tame_animal" | "summoned_entity" => {
                let json: EntityConditionJson = conditions!(EntityConditionJson);
                let player = generator.player(json.player);
                let entity = generator.optional_condition(json.entity);
                (
                    quote! { EntityConditionTriggerInstance },
                    quote! { EntityConditionTriggerInstance { player: #player, entity: #entity } },
                )
            }
            "placed_block" | "item_used_on_block" | "allay_drop_item_on_block" => {
                let json: ItemUsedOnLocationJson = conditions!(ItemUsedOnLocationJson);
                let player = generator.player(json.player);
                let location = generator.optional_condition(json.location);
                (
                    quote! { ItemUsedOnBlockTriggerInstance },
                    quote! {
                        ItemUsedOnBlockTriggerInstance { player: #player, location: #location }
                    },
                )
            }
            "any_block_use" | "default_block_use" => {
                let json: ItemUsedOnLocationJson = conditions!(ItemUsedOnLocationJson);
                let player = generator.player(json.player);
                let location = generator.optional_condition(json.location);
                (
                    quote! { BlockUseTriggerInstance },
                    quote! { BlockUseTriggerInstance { player: #player, location: #location } },
                )
            }
            "enter_block" => {
                let json: BlockStateJson = conditions!(BlockStateJson);
                let player = generator.player(json.player);
                let blocks = generator.optional_holder_set(json.blocks);
                let state = generator.optional_state(json.state);
                (
                    quote! { EnterBlockTriggerInstance },
                    quote! {
                        EnterBlockTriggerInstance { player: #player, blocks: #blocks, state: #state }
                    },
                )
            }
            "slide_down_block" => {
                let json: BlockStateJson = conditions!(BlockStateJson);
                let player = generator.player(json.player);
                let blocks = generator.optional_holder_set(json.blocks);
                let state = generator.optional_state(json.state);
                (
                    quote! { SlideDownBlockTriggerInstance },
                    quote! {
                        SlideDownBlockTriggerInstance {
                            player: #player,
                            blocks: #blocks,
                            state: #state,
                        }
                    },
                )
            }
            "bee_nest_destroyed" => {
                let json: BeeNestJson = conditions!(BeeNestJson);
                let player = generator.player(json.player);
                let blocks = generator.optional_holder_set(json.blocks);
                let state = generator.optional_state(json.state);
                let item = generator.optional_item_predicate(json.item);
                let bees = generator.int_bounds(json.num_bees_inside);
                (
                    quote! { BeeNestDestroyedTriggerInstance },
                    quote! {
                        BeeNestDestroyedTriggerInstance {
                            player: #player,
                            blocks: #blocks,
                            state: #state,
                            item: #item,
                            num_bees_inside: #bees,
                        }
                    },
                )
            }
            "player_generates_container_loot" => {
                let json: LootTableJson = conditions!(LootTableJson);
                let player = generator.player(json.player);
                let loot_tables = generator.holder_set(&json.loot_tables);
                (
                    quote! { LootTableTriggerInstance },
                    quote! {
                        LootTableTriggerInstance { player: #player, loot_tables: #loot_tables }
                    },
                )
            }
            "player_interacted_with_entity" | "player_sheared_equipment" => {
                let json: InteractJson = conditions!(InteractJson);
                let player = generator.player(json.player);
                let item = generator.optional_item_predicate(json.item);
                let entity = generator.optional_condition(json.entity);
                (
                    quote! { PlayerInteractedWithEntityTriggerInstance },
                    quote! {
                        PlayerInteractedWithEntityTriggerInstance {
                            player: #player,
                            item: #item,
                            entity: #entity,
                        }
                    },
                )
            }
            "fishing_rod_hooked" => {
                let json: FishingJson = conditions!(FishingJson);
                let player = generator.player(json.player);
                let rod = generator.optional_item_predicate(json.rod);
                let entity = generator.optional_condition(json.entity);
                let item = generator.optional_item_predicate(json.item);
                (
                    quote! { FishingRodHookedTriggerInstance },
                    quote! {
                        FishingRodHookedTriggerInstance {
                            player: #player,
                            rod: #rod,
                            entity: #entity,
                            item: #item,
                        }
                    },
                )
            }
            "changed_dimension" => {
                let json: ChangedDimensionJson = conditions!(ChangedDimensionJson);
                let player = generator.player(json.player);
                let from = generator.optional_identifier(json.from.as_deref());
                let to = generator.optional_identifier(json.to.as_deref());
                (
                    quote! { ChangedDimensionTriggerInstance },
                    quote! {
                        ChangedDimensionTriggerInstance { player: #player, from: #from, to: #to }
                    },
                )
            }
            "brewed_potion" => {
                let json: BrewedPotionJson = conditions!(BrewedPotionJson);
                let player = generator.player(json.player);
                let potion = generator.optional_identifier(json.potion.as_deref());
                (
                    quote! { BrewedPotionTriggerInstance },
                    quote! { BrewedPotionTriggerInstance { player: #player, potion: #potion } },
                )
            }
            "player_hurt_entity" => {
                let json: HurtEntityJson = conditions!(HurtEntityJson);
                let player = generator.player(json.player);
                let damage = generator.optional_damage(json.damage);
                let entity = generator.optional_condition(json.entity);
                (
                    quote! { PlayerHurtEntityTriggerInstance },
                    quote! {
                        PlayerHurtEntityTriggerInstance {
                            player: #player,
                            damage: #damage,
                            entity: #entity,
                        }
                    },
                )
            }
            "entity_hurt_player" => {
                let json: EntityHurtPlayerJson = conditions!(EntityHurtPlayerJson);
                let player = generator.player(json.player);
                let damage = generator.optional_damage(json.damage);
                (
                    quote! { EntityHurtPlayerTriggerInstance },
                    quote! { EntityHurtPlayerTriggerInstance { player: #player, damage: #damage } },
                )
            }
            "killed_by_arrow" => {
                let json: KilledByArrowJson = conditions!(KilledByArrowJson);
                let player = generator.player(json.player);
                let victims = generator.conditions(json.victims);
                let unique = generator.int_bounds(json.unique_entity_types);
                let weapon = generator.optional_item_predicate(json.fired_from_weapon);
                (
                    quote! { KilledByArrowTriggerInstance },
                    quote! {
                        KilledByArrowTriggerInstance {
                            player: #player,
                            victims: #victims,
                            unique_entity_types: #unique,
                            fired_from_weapon: #weapon,
                        }
                    },
                )
            }
            "channeled_lightning" => {
                let json: ChanneledLightningJson = conditions!(ChanneledLightningJson);
                let player = generator.player(json.player);
                let victims = generator.conditions(json.victims);
                (
                    quote! { ChanneledLightningTriggerInstance },
                    quote! { ChanneledLightningTriggerInstance { player: #player, victims: #victims } },
                )
            }
            "effects_changed" => {
                let json: EffectsChangedJson = conditions!(EffectsChangedJson);
                let player = generator.player(json.player);
                let effects = generator.optional_effects(json.effects);
                let source = generator.optional_condition(json.source);
                (
                    quote! { EffectsChangedTriggerInstance },
                    quote! {
                        EffectsChangedTriggerInstance {
                            player: #player,
                            effects: #effects,
                            source: #source,
                        }
                    },
                )
            }
            "villager_trade" => {
                let json: TradeJson = conditions!(TradeJson);
                let player = generator.player(json.player);
                let villager = generator.optional_condition(json.villager);
                let item = generator.optional_item_predicate(json.item);
                (
                    quote! { TradeTriggerInstance },
                    quote! {
                        TradeTriggerInstance { player: #player, villager: #villager, item: #item }
                    },
                )
            }
            "thrown_item_picked_up_by_entity" | "thrown_item_picked_up_by_player" => {
                let json: PickedUpJson = conditions!(PickedUpJson);
                let player = generator.player(json.player);
                let item = generator.optional_item_predicate(json.item);
                let entity = generator.optional_condition(json.entity);
                (
                    quote! { PickedUpItemTriggerInstance },
                    quote! {
                        PickedUpItemTriggerInstance { player: #player, item: #item, entity: #entity }
                    },
                )
            }
            "construct_beacon" => {
                let json: BeaconJson = conditions!(BeaconJson);
                let player = generator.player(json.player);
                let level = generator.int_bounds(json.level);
                (
                    quote! { ConstructBeaconTriggerInstance },
                    quote! { ConstructBeaconTriggerInstance { player: #player, level: #level } },
                )
            }
            "target_hit" => {
                let json: TargetHitJson = conditions!(TargetHitJson);
                let player = generator.player(json.player);
                let signal = optional_i32(json.signal_strength);
                let projectile = generator.optional_condition(json.projectile);
                (
                    quote! { TargetHitTriggerInstance },
                    quote! {
                        TargetHitTriggerInstance {
                            player: #player,
                            signal_strength: #signal,
                            projectile: #projectile,
                        }
                    },
                )
            }
            "spear_mobs" => {
                let json: SpearMobsJson = conditions!(SpearMobsJson);
                let player = generator.player(json.player);
                let count = optional_i32(json.count);
                (
                    quote! { SpearMobsTriggerInstance },
                    quote! { SpearMobsTriggerInstance { player: #player, count: #count } },
                )
            }
            "fall_from_height" | "nether_travel" | "ride_entity_in_lava" => {
                let json: DistanceTriggerJson = conditions!(DistanceTriggerJson);
                let player = generator.player(json.player);
                let start = generator.optional_location(json.start_position);
                let distance = generator.optional_distance(json.distance);
                (
                    quote! { DistanceTriggerInstance },
                    quote! {
                        DistanceTriggerInstance {
                            player: #player,
                            start_position: #start,
                            distance: #distance,
                        }
                    },
                )
            }
            "fall_after_explosion" => {
                let json: FallAfterExplosionJson = conditions!(FallAfterExplosionJson);
                let player = generator.player(json.player);
                let start = generator.optional_location(json.start_position);
                let distance = generator.optional_distance(json.distance);
                let cause = generator.optional_condition(json.cause);
                (
                    quote! { FallAfterExplosionTriggerInstance },
                    quote! {
                        FallAfterExplosionTriggerInstance {
                            player: #player,
                            start_position: #start,
                            distance: #distance,
                            cause: #cause,
                        }
                    },
                )
            }
            "levitation" => {
                let json: LevitationJson = conditions!(LevitationJson);
                let player = generator.player(json.player);
                let distance = generator.optional_distance(json.distance);
                let duration = generator.int_bounds(json.duration);
                (
                    quote! { LevitationTriggerInstance },
                    quote! {
                        LevitationTriggerInstance {
                            player: #player,
                            distance: #distance,
                            duration: #duration,
                        }
                    },
                )
            }
            "lightning_strike" => {
                let json: LightningStrikeJson = conditions!(LightningStrikeJson);
                let player = generator.player(json.player);
                let lightning = generator.optional_condition(json.lightning);
                let bystander = generator.optional_condition(json.bystander);
                (
                    quote! { LightningStrikeTriggerInstance },
                    quote! {
                        LightningStrikeTriggerInstance {
                            player: #player,
                            lightning: #lightning,
                            bystander: #bystander,
                        }
                    },
                )
            }
            "item_durability_changed" => {
                let json: DurabilityJson = conditions!(DurabilityJson);
                let player = generator.player(json.player);
                let item = generator.optional_item_predicate(json.item);
                let durability = generator.int_bounds(json.durability);
                let delta = generator.int_bounds(json.delta);
                (
                    quote! { ItemDurabilityChangedTriggerInstance },
                    quote! {
                        ItemDurabilityChangedTriggerInstance {
                            player: #player,
                            item: #item,
                            durability: #durability,
                            delta: #delta,
                        }
                    },
                )
            }
            "enchanted_item" => {
                let json: EnchantedJson = conditions!(EnchantedJson);
                let player = generator.player(json.player);
                let item = generator.optional_item_predicate(json.item);
                let levels = generator.int_bounds(json.levels);
                (
                    quote! { EnchantedItemTriggerInstance },
                    quote! {
                        EnchantedItemTriggerInstance { player: #player, item: #item, levels: #levels }
                    },
                )
            }
            "used_ender_eye" => {
                let json: UsedEnderEyeJson = conditions!(UsedEnderEyeJson);
                let player = generator.player(json.player);
                let distance = generator.double_bounds(json.distance);
                (
                    quote! { UsedEnderEyeTriggerInstance },
                    quote! { UsedEnderEyeTriggerInstance { player: #player, distance: #distance } },
                )
            }
            "cured_zombie_villager" => {
                let json: CuredJson = conditions!(CuredJson);
                let player = generator.player(json.player);
                let zombie = generator.optional_condition(json.zombie);
                let villager = generator.optional_condition(json.villager);
                (
                    quote! { CuredZombieVillagerTriggerInstance },
                    quote! {
                        CuredZombieVillagerTriggerInstance {
                            player: #player,
                            zombie: #zombie,
                            villager: #villager,
                        }
                    },
                )
            }
            other => panic!(
                "advancement trigger `{other}` is not modeled by the criterion generator; \
                 extend steel-registry/build/advancements/criterion.rs and \
                 steel-registry/src/advancement/triggers"
            ),
        }
    }
}

/// Emits the statics a criterion references and the `&'static` expressions.
struct CriterionGenerator {
    prefix: String,
    counter: usize,
    items: TokenStream,
}

impl CriterionGenerator {
    fn new(prefix: String) -> Self {
        Self {
            prefix,
            counter: 0,
            items: TokenStream::new(),
        }
    }

    /// Emits `static <prefix>__<n>: [T; N] = [...];` and returns `&<prefix>__<n>`.
    fn array(&mut self, element_type: TokenStream, elements: Vec<TokenStream>) -> TokenStream {
        let ident = self.next_ident();
        let len = elements.len();
        self.items.extend(quote! {
            static #ident: [#element_type; #len] = [#(#elements),*];
        });
        quote! { &#ident }
    }

    /// Emits a named static for a single value and returns its path.
    fn single(&mut self, ty: TokenStream, value: TokenStream) -> Ident {
        let ident = self.next_ident();
        self.items.extend(quote! { static #ident: #ty = #value; });
        ident
    }

    fn next_ident(&mut self) -> Ident {
        let ident = Ident::new(
            &format!("{}__{}", self.prefix, self.counter),
            Span::call_site(),
        );
        self.counter += 1;
        ident
    }

    fn player(&mut self, condition: Option<ConditionJson>) -> TokenStream {
        let condition = self.optional_condition(condition);
        quote! { PlayerCondition { player: #condition } }
    }

    fn conditions(&mut self, conditions: Vec<ConditionJson>) -> TokenStream {
        let elements: Vec<TokenStream> = conditions
            .iter()
            .map(|condition| self.condition(condition))
            .collect();
        self.array(quote! { Condition }, elements)
    }

    fn item_predicates(&mut self, items: Vec<ItemPredicateJson>) -> TokenStream {
        let elements: Vec<TokenStream> = items.iter().map(|item| self.item_predicate(item)).collect();
        self.array(quote! { ItemPredicate }, elements)
    }

    fn optional_item_predicate(&mut self, item: Option<ItemPredicateJson>) -> TokenStream {
        match item {
            Some(item) => {
                let item = self.item_predicate(&item);
                quote! { Some(#item) }
            }
            None => quote! { None },
        }
    }

    fn optional_condition(&mut self, condition: Option<ConditionJson>) -> TokenStream {
        match condition {
            Some(condition) => {
                let condition = self.condition(&condition);
                quote! { Some(#condition) }
            }
            None => quote! { None },
        }
    }

    fn optional_holder_set(&mut self, set: Option<HolderSetJson>) -> TokenStream {
        match set {
            Some(set) => {
                let set = self.holder_set(&set);
                quote! { Some(#set) }
            }
            None => quote! { None },
        }
    }

    fn holder_set(&mut self, set: &HolderSetJson) -> TokenStream {
        match set {
            HolderSetJson::Single(value) => {
                if let Some(tag) = value.strip_prefix('#') {
                    let tag = identifier(tag);
                    quote! { HolderSet::Tag(#tag) }
                } else {
                    let key = value
                        .strip_prefix("minecraft:")
                        .unwrap_or(value)
                        .to_owned();
                    let keys = self.array(quote! { &'static str }, vec![quote! { #key }]);
                    quote! { HolderSet::Keys(#keys) }
                }
            }
            HolderSetJson::List(values) => {
                let elements: Vec<TokenStream> = values
                    .iter()
                    .map(|value| {
                        assert!(
                            !value.starts_with('#'),
                            "a holder set cannot mix tags and direct keys: {value}"
                        );
                        let key = value
                            .strip_prefix("minecraft:")
                            .unwrap_or(value)
                            .to_owned();
                        quote! { #key }
                    })
                    .collect();
                let keys = self.array(quote! { &'static str }, elements);
                quote! { HolderSet::Keys(#keys) }
            }
        }
    }

    fn optional_state(
        &mut self,
        state: Option<BTreeMap<String, String>>,
    ) -> TokenStream {
        match state {
            Some(state) => {
                let elements: Vec<TokenStream> = state
                    .iter()
                    .map(|(name, value)| {
                        quote! { PropertyCheck { name: #name, value: #value } }
                    })
                    .collect();
                let properties = self.array(quote! { PropertyCheck }, elements);
                quote! { Some(StatePropertiesPredicate { properties: #properties }) }
            }
            None => quote! { None },
        }
    }

    fn optional_identifier(&mut self, value: Option<&str>) -> TokenStream {
        match value {
            Some(value) => {
                let value = identifier(value);
                quote! { Some(#value) }
            }
            None => quote! { None },
        }
    }

    fn int_bounds(&mut self, bounds: Option<IntBoundsJson>) -> TokenStream {
        match bounds {
            None => quote! { IntBounds::ANY },
            Some(IntBoundsJson::Exact(value)) => quote! { IntBounds::exactly(#value) },
            Some(IntBoundsJson::Bounds { min, max }) => match (min, max) {
                (Some(min), Some(max)) => quote! { IntBounds::range(#min, #max) },
                (Some(min), None) => quote! { IntBounds::at_least(#min) },
                (None, Some(max)) => quote! { IntBounds::at_most(#max) },
                (None, None) => quote! { IntBounds::ANY },
            },
        }
    }

    fn double_bounds(&mut self, bounds: Option<DoubleBoundsJson>) -> TokenStream {
        match bounds {
            None => quote! { DoubleBounds::ANY },
            Some(DoubleBoundsJson::Exact(value)) => quote! { DoubleBounds::exactly(#value) },
            Some(DoubleBoundsJson::Bounds { min, max }) => match (min, max) {
                (Some(min), Some(max)) => quote! { DoubleBounds::range(#min, #max) },
                (Some(min), None) => quote! { DoubleBounds::at_least(#min) },
                (None, Some(max)) => quote! { DoubleBounds::at_most(#max) },
                (None, None) => quote! { DoubleBounds::ANY },
            },
        }
    }

    fn optional_int_bounds(&mut self, bounds: Option<IntBoundsJson>) -> TokenStream {
        match bounds {
            Some(bounds) => {
                let bounds = self.int_bounds(Some(bounds));
                quote! { Some(#bounds) }
            }
            None => quote! { None },
        }
    }

    fn condition(&mut self, condition: &ConditionJson) -> TokenStream {
        match condition {
            ConditionJson::EntityProperties { entity, predicate } => {
                let entity = entity_target(entity);
                let predicate = self.entity_predicate(predicate);
                quote! {
                    Condition::EntityProperties { entity: #entity, predicate: #predicate }
                }
            }
            ConditionJson::LocationCheck {
                offset_x,
                offset_y,
                offset_z,
                predicate,
            } => {
                let predicate = self.location_predicate(predicate);
                quote! {
                    Condition::LocationCheck {
                        offset_x: #offset_x,
                        offset_y: #offset_y,
                        offset_z: #offset_z,
                        predicate: #predicate,
                    }
                }
            }
            ConditionJson::MatchBlock { blocks, state } => {
                let blocks = self.optional_holder_set(blocks.clone());
                let properties = self.properties(state);
                quote! {
                    Condition::MatchBlock { blocks: #blocks, properties: #properties }
                }
            }
            ConditionJson::MatchTool { predicate } => {
                let predicate = self.item_predicate(predicate);
                quote! { Condition::MatchTool(#predicate) }
            }
            ConditionJson::Inverted { term } => {
                let term = self.condition(term);
                let term = self.single(quote! { Condition }, term);
                quote! { Condition::Inverted(&#term) }
            }
            ConditionJson::AnyOf { terms } => {
                let terms = self.conditions(terms.clone());
                quote! { Condition::AnyOf(#terms) }
            }
            ConditionJson::AllOf { terms } => {
                let terms = self.conditions(terms.clone());
                quote! { Condition::AllOf(#terms) }
            }
        }
    }

    fn properties(&mut self, state: &BTreeMap<String, String>) -> TokenStream {
        let elements: Vec<TokenStream> = state
            .iter()
            .map(|(name, value)| quote! { PropertyCheck { name: #name, value: #value } })
            .collect();
        self.array(quote! { PropertyCheck }, elements)
    }

    fn entity_predicate(&mut self, predicate: &EntityPredicateJson) -> TokenStream {
        let entity_type = self.optional_holder_set(predicate.entity_type.clone());
        let location = self.optional_location(predicate.location.clone());
        let stepping_on = self.optional_location(predicate.stepping_on.clone());
        let distance = self.optional_distance(predicate.distance.clone());
        let effects = self.optional_effects(predicate.effects.clone());
        let flags = predicate.flags.as_ref().map_or_else(
            || quote! { None },
            |flags| {
                let is_on_ground = optional_bool(flags.is_on_ground);
                let is_on_fire = optional_bool(flags.is_on_fire);
                let is_sneaking = optional_bool(flags.is_sneaking);
                let is_sprinting = optional_bool(flags.is_sprinting);
                let is_swimming = optional_bool(flags.is_swimming);
                let is_flying = optional_bool(flags.is_flying);
                let is_baby = optional_bool(flags.is_baby);
                quote! {
                    Some(EntityFlagsPredicate {
                        is_on_ground: #is_on_ground,
                        is_on_fire: #is_on_fire,
                        is_sneaking: #is_sneaking,
                        is_sprinting: #is_sprinting,
                        is_swimming: #is_swimming,
                        is_flying: #is_flying,
                        is_baby: #is_baby,
                    })
                }
            },
        );
        let equipment = self.optional_equipment(predicate.equipment.clone());
        let components = predicate.components.as_ref().map_or_else(
            || quote! { None },
            |components| {
                let cat_variant = optional_identifier(components.cat_variant.as_deref());
                let frog_variant = optional_identifier(components.frog_variant.as_deref());
                let wolf_variant = optional_identifier(components.wolf_variant.as_deref());
                let chicken_variant = optional_identifier(components.chicken_variant.as_deref());
                let sheep_color = components.sheep_color.as_deref().map_or_else(
                    || quote! { None },
                    |color| {
                        let color = dye_color(color);
                        quote! { Some(#color) }
                    },
                );
                let sheep_sheared = optional_bool(components.sheep_sheared);
                quote! {
                    Some(EntityComponentsPredicate {
                        cat_variant: #cat_variant,
                        frog_variant: #frog_variant,
                        wolf_variant: #wolf_variant,
                        sheep_color: #sheep_color,
                        sheep_sheared: #sheep_sheared,
                        chicken_variant: #chicken_variant,
                    })
                }
            },
        );
        let vehicle = self.optional_boxed_entity(predicate.vehicle.as_deref());
        let passenger = self.optional_boxed_entity(predicate.passenger.as_deref());
        let player = predicate.player.as_ref().map_or_else(
            || quote! { None },
            |player| {
                let looking_at = self.optional_boxed_entity(player.looking_at.as_deref());
                let recipes = player.recipes.as_ref().map_or_else(
                    || quote! { None },
                    |recipes| {
                        let elements: Vec<TokenStream> = recipes
                            .iter()
                            .map(|recipe| {
                                let recipe = recipe
                                    .strip_prefix("minecraft:")
                                    .unwrap_or(recipe)
                                    .to_owned();
                                quote! { #recipe }
                            })
                            .collect();
                        let recipes = self.array(quote! { &'static str }, elements);
                        quote! { Some(#recipes) }
                    },
                );
                quote! { Some(PlayerPredicate { looking_at: #looking_at, recipes: #recipes }) }
            },
        );
        let lightning = predicate.lightning.as_ref().map_or_else(
            || quote! { None },
            |lightning| {
                let blocks = self.int_bounds(lightning.blocks_set_on_fire.clone());
                let struck = self.optional_boxed_entity(lightning.entity_struck.as_deref());
                quote! {
                    Some(LightningPredicate {
                        blocks_set_on_fire: #blocks,
                        entity_struck: #struck,
                    })
                }
            },
        );
        quote! {
            EntityPredicate {
                entity_type: #entity_type,
                location: #location,
                stepping_on: #stepping_on,
                distance: #distance,
                effects: #effects,
                flags: #flags,
                equipment: #equipment,
                components: #components,
                vehicle: #vehicle,
                passenger: #passenger,
                player: #player,
                lightning: #lightning,
            }
        }
    }

    fn optional_boxed_entity(&mut self, predicate: Option<&EntityPredicateJson>) -> TokenStream {
        match predicate {
            Some(predicate) => {
                let predicate = self.entity_predicate(predicate);
                let predicate = self.single(quote! { EntityPredicate }, predicate);
                quote! { Some(&#predicate) }
            }
            None => quote! { None },
        }
    }

    fn optional_location(&mut self, location: Option<LocationPredicateJson>) -> TokenStream {
        match location {
            Some(location) => {
                let location = self.location_predicate(&location);
                quote! { Some(#location) }
            }
            None => quote! { None },
        }
    }

    fn location_predicate(&mut self, location: &LocationPredicateJson) -> TokenStream {
        let position = location.position.as_ref().map_or_else(
            || quote! { None },
            |position| {
                let x = self.double_bounds(position.x.clone());
                let y = self.double_bounds(position.y.clone());
                let z = self.double_bounds(position.z.clone());
                quote! { Some(PositionPredicate { x: #x, y: #y, z: #z }) }
            },
        );
        let biomes = self.optional_holder_set(location.biomes.clone());
        let structures = self.optional_holder_set(location.structures.clone());
        let dimension = self.optional_identifier(location.dimension.as_deref());
        let light = self.optional_int_bounds(location.light.clone());
        let block = location.block.as_ref().map_or_else(
            || quote! { None },
            |block| {
                let blocks = self.optional_holder_set(block.blocks.clone());
                let state = self.optional_state(block.state.clone());
                quote! { Some(BlockPredicate { blocks: #blocks, state: #state }) }
            },
        );
        let fluid = self.optional_identifier(location.fluid.as_deref());
        let smokey = optional_bool(location.smokey);
        let can_see_sky = optional_bool(location.can_see_sky);
        quote! {
            LocationPredicate {
                position: #position,
                biomes: #biomes,
                structures: #structures,
                dimension: #dimension,
                light: #light,
                block: #block,
                fluid: #fluid,
                smokey: #smokey,
                can_see_sky: #can_see_sky,
            }
        }
    }

    fn optional_distance(&mut self, distance: Option<DistancePredicateJson>) -> TokenStream {
        match distance {
            Some(distance) => {
                let x = self.double_bounds(distance.x);
                let y = self.double_bounds(distance.y);
                let z = self.double_bounds(distance.z);
                let horizontal = self.double_bounds(distance.horizontal);
                let absolute = self.double_bounds(distance.absolute);
                quote! {
                    Some(DistancePredicate {
                        x: #x,
                        y: #y,
                        z: #z,
                        horizontal: #horizontal,
                        absolute: #absolute,
                    })
                }
            }
            None => quote! { None },
        }
    }

    fn optional_effects(
        &mut self,
        effects: Option<BTreeMap<String, EffectPredicateJson>>,
    ) -> TokenStream {
        match effects {
            Some(effects) => {
                let elements: Vec<TokenStream> = effects
                    .iter()
                    .map(|(key, effect)| {
                        let key = key.strip_prefix("minecraft:").unwrap_or(key).to_owned();
                        let amplifier = self.int_bounds(effect.amplifier.clone());
                        let duration = self.int_bounds(effect.duration.clone());
                        let ambient = optional_bool(effect.ambient);
                        let visible = optional_bool(effect.visible);
                        quote! {
                            (#key, MobEffectPredicateData {
                                amplifier: #amplifier,
                                duration: #duration,
                                ambient: #ambient,
                                visible: #visible,
                            })
                        }
                    })
                    .collect();
                let effects = self.array(quote! { (&'static str, MobEffectPredicateData) }, elements);
                quote! { Some(MobEffectsPredicate { effects: #effects }) }
            }
            None => quote! { None },
        }
    }

    fn optional_equipment(
        &mut self,
        equipment: Option<EntityEquipmentJson>,
    ) -> TokenStream {
        match equipment {
            Some(equipment) => {
                let head = self.optional_item_predicate(equipment.head);
                let chest = self.optional_item_predicate(equipment.chest);
                let legs = self.optional_item_predicate(equipment.legs);
                let feet = self.optional_item_predicate(equipment.feet);
                let body = self.optional_item_predicate(equipment.body);
                let mainhand = self.optional_item_predicate(equipment.mainhand);
                let offhand = self.optional_item_predicate(equipment.offhand);
                quote! {
                    Some(EntityEquipmentPredicate {
                        head: #head,
                        chest: #chest,
                        legs: #legs,
                        feet: #feet,
                        body: #body,
                        mainhand: #mainhand,
                        offhand: #offhand,
                    })
                }
            }
            None => quote! { None },
        }
    }

    fn item_predicate(&mut self, item: &ItemPredicateJson) -> TokenStream {
        let items = self.optional_holder_set(item.items.clone());
        let count = self.optional_int_bounds(item.count.clone());
        let mut exact = Vec::new();
        let mut has = Vec::new();
        if let Some(components) = &item.components {
            for (key, value) in components {
                match key.as_str() {
                    "minecraft:damage" => {
                        let value: i32 = serde_json::from_value(value.clone())
                            .unwrap_or_else(|error| panic!("invalid `minecraft:damage`: {error}"));
                        exact.push(quote! { ExactComponent::Damage(#value) });
                    }
                    "minecraft:banner_patterns" => {
                        let layers: Vec<BannerLayerJson> =
                            serde_json::from_value(value.clone()).unwrap_or_else(|error| {
                                panic!("invalid `minecraft:banner_patterns`: {error}")
                            });
                        let elements: Vec<TokenStream> = layers
                            .iter()
                            .map(|layer| {
                                let pattern = layer
                                    .pattern
                                    .strip_prefix("minecraft:")
                                    .unwrap_or(&layer.pattern)
                                    .to_owned();
                                let color = dye_color(&layer.color);
                                quote! { (#pattern, #color) }
                            })
                            .collect();
                        let layers = self.array(quote! { (&'static str, DyeColor) }, elements);
                        exact.push(quote! { ExactComponent::BannerPatterns(#layers) });
                    }
                    "minecraft:item_name" => {
                        let name: TextComponentJson = serde_json::from_value(value.clone())
                            .unwrap_or_else(|error| {
                                panic!("invalid `minecraft:item_name`: {error}")
                            });
                        let translate = name.translate;
                        exact.push(quote! {
                            ExactComponent::ItemName(TextComponent::translated(
                                TranslatedMessage::new(#translate, None),
                            ))
                        });
                    }
                    other => panic!(
                        "exact item component `{other}` is not modeled by the criterion \
                         generator; extend steel-registry/build/advancements/criterion.rs"
                    ),
                }
            }
        }
        let mut enchantments = Vec::new();
        let mut stored_enchantments = Vec::new();
        if let Some(predicates) = &item.predicates {
            for (key, value) in predicates {
                match key.as_str() {
                    "minecraft:enchantments" => {
                        enchantments = self.enchantment_predicates(value);
                    }
                    "minecraft:stored_enchantments" => {
                        stored_enchantments = self.enchantment_predicates(value);
                    }
                    other => {
                        assert!(
                            value.as_object().is_some_and(Map::is_empty),
                            "component predicate `{other}` carries conditions Steel cannot model"
                        );
                        let key = identifier(other);
                        has.push(quote! { #key });
                    }
                }
            }
        }
        let has_components = self.array(quote! { Identifier }, has);
        let exact_components = self.array(quote! { ExactComponent }, exact);
        let enchantments = self.array(quote! { EnchantmentPredicate }, enchantments);
        let stored_enchantments = self.array(quote! { EnchantmentPredicate }, stored_enchantments);
        quote! {
            ItemPredicate {
                items: #items,
                count: #count,
                has_components: #has_components,
                exact_components: #exact_components,
                enchantments: #enchantments,
                stored_enchantments: #stored_enchantments,
            }
        }
    }

    fn enchantment_predicates(&mut self, value: &Value) -> Vec<TokenStream> {
        let predicates: Vec<EnchantmentPredicateJson> = serde_json::from_value(value.clone())
            .unwrap_or_else(|error| panic!("invalid enchantment predicate: {error}"));
        predicates
            .iter()
            .map(|predicate| {
                let enchantments = self.optional_holder_set(predicate.enchantments.clone());
                let (min_level, max_level) = match &predicate.levels {
                    None => (quote! { None }, quote! { None }),
                    Some(IntBoundsJson::Exact(value)) => {
                        (quote! { Some(#value) }, quote! { Some(#value) })
                    }
                    Some(IntBoundsJson::Bounds { min, max }) => (
                        optional_i32(*min),
                        optional_i32(*max),
                    ),
                };
                quote! {
                    EnchantmentPredicate {
                        enchantments: #enchantments,
                        min_level: #min_level,
                        max_level: #max_level,
                    }
                }
            })
            .collect()
    }

    fn optional_damage(&mut self, damage: Option<DamagePredicateJson>) -> TokenStream {
        match damage {
            Some(damage) => {
                let dealt = self.optional_double_bounds(damage.dealt);
                let taken = self.optional_double_bounds(damage.taken);
                let blocked = optional_bool(damage.blocked);
                let source_entity = self.optional_entity_predicate(damage.source_entity);
                let damage_type = self.optional_damage_source(damage.damage_type);
                quote! {
                    Some(DamagePredicate {
                        dealt: #dealt,
                        taken: #taken,
                        source_entity: #source_entity,
                        blocked: #blocked,
                        damage_type: #damage_type,
                    })
                }
            }
            None => quote! { None },
        }
    }

    fn optional_double_bounds(&mut self, bounds: Option<DoubleBoundsJson>) -> TokenStream {
        match bounds {
            Some(bounds) => {
                let bounds = self.double_bounds(Some(bounds));
                quote! { Some(#bounds) }
            }
            None => quote! { None },
        }
    }

    fn optional_entity_predicate(
        &mut self,
        predicate: Option<EntityPredicateJson>,
    ) -> TokenStream {
        match predicate {
            Some(predicate) => {
                let predicate = self.entity_predicate(&predicate);
                quote! { Some(#predicate) }
            }
            None => quote! { None },
        }
    }

    fn optional_damage_source(
        &mut self,
        source: Option<DamageSourcePredicateJson>,
    ) -> TokenStream {
        match source {
            Some(source) => {
                let tags: Vec<TokenStream> = source
                    .tags
                    .iter()
                    .flatten()
                    .map(|damage_tag| {
                        let tag = damage_tag.id.strip_prefix('#').unwrap_or(&damage_tag.id);
                        let tag = tag.strip_prefix("minecraft:").unwrap_or(tag).to_owned();
                        let expected = damage_tag.expected;
                        quote! { (#tag, #expected) }
                    })
                    .collect();
                let tags = self.array(quote! { (&'static str, bool) }, tags);
                let source_entity = self.optional_entity_predicate(source.source_entity);
                let direct_entity = self.optional_entity_predicate(source.direct_entity);
                let is_direct = optional_bool(source.is_direct);
                quote! {
                    Some(DamageSourcePredicate {
                        tags: #tags,
                        source_entity: #source_entity,
                        direct_entity: #direct_entity,
                        is_direct: #is_direct,
                    })
                }
            }
            None => quote! { None },
        }
    }
}

/// Rejects condition objects carrying fields the model does not read, so a
/// datapack change cannot silently weaken a criterion.
fn validate_conditions(value: &Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(ty)) = map.get("type") {
                let allowed: &[&str] = match ty.as_str() {
                    "minecraft:entity_properties" => &["type", "entity", "predicate"],
                    "minecraft:location_check" => {
                        &["type", "predicate", "offsetX", "offsetY", "offsetZ"]
                    }
                    "minecraft:match_block" => &["type", "blocks", "state"],
                    "minecraft:match_tool" => &["type", "predicate"],
                    "minecraft:inverted" => &["type", "term"],
                    "minecraft:any_of" | "minecraft:all_of" => &["type", "terms"],
                    other => panic!(
                        "condition type `{other}` is not modeled by the criterion generator; \
                         extend steel-registry/build/advancements/criterion.rs"
                    ),
                };
                for key in map.keys() {
                    assert!(
                        allowed.contains(&key.as_str()),
                        "condition `{ty}` has a field Steel cannot model: `{key}`"
                    );
                }
            }
            for nested in map.values() {
                validate_conditions(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                validate_conditions(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Maps an advancement trigger id to its trigger singleton.
fn trigger_static(trigger: &Identifier) -> TokenStream {
    assert_eq!(
        trigger.namespace.as_ref(),
        "minecraft",
        "advancement triggers must be vanilla ids: {trigger}"
    );
    let path = trigger.path.as_ref();
    let ident = Ident::new(&path.to_shouty_snake_case(), Span::call_site());
    quote! { crate::advancement::triggers::#ident }
}

/// Emits a vanilla identifier.
fn identifier(value: &str) -> TokenStream {
    let (namespace, path) = match value.split_once(':') {
        Some((namespace, path)) => (namespace, path),
        None => ("minecraft", value),
    };
    if namespace == "minecraft" {
        quote! { Identifier::vanilla_static(#path) }
    } else {
        quote! {
            Identifier { namespace: Cow::Borrowed(#namespace), path: Cow::Borrowed(#path) }
        }
    }
}

fn optional_identifier(value: Option<&str>) -> TokenStream {
    match value {
        Some(value) => {
            let value = identifier(value);
            quote! { Some(#value) }
        }
        None => quote! { None },
    }
}

fn optional_bool(value: Option<bool>) -> TokenStream {
    match value {
        Some(value) => quote! { Some(#value) },
        None => quote! { None },
    }
}

fn optional_i32(value: Option<i32>) -> TokenStream {
    match value {
        Some(value) => quote! { Some(#value) },
        None => quote! { None },
    }
}

fn dye_color(value: &str) -> TokenStream {
    let ident = Ident::new(&value.to_upper_camel_case(), Span::call_site());
    quote! { crate::DyeColor::#ident }
}

fn entity_target(value: &str) -> TokenStream {
    match value {
        "this" => quote! { crate::loot_table::LootContextEntity::This },
        "killer" => quote! { crate::loot_table::LootContextEntity::Killer },
        "direct_killer" => quote! { crate::loot_table::LootContextEntity::DirectKiller },
        "killer_player" => quote! { crate::loot_table::LootContextEntity::KillerPlayer },
        other => panic!("entity target `{other}` is not modeled by the criterion generator"),
    }
}

/// Parses every criterion of one advancement, emitting the statics they need.
pub(crate) fn parse_criteria(
    adv_const: &Ident,
    list: &BTreeMap<String, CriterionJson>,
) -> GeneratedCriteria {
    let mut items = TokenStream::new();
    let mut entries = Vec::new();
    for (name, criterion) in list {
        let value = criterion.generate(adv_const, name, &mut items);
        entries.push(quote! { (#name.to_owned(), #value) });
    }
    GeneratedCriteria {
        items,
        map: quote! { BTreeMap::from([#(#entries),*]) },
    }
}

// ---------------------------------------------------------------------------
// JSON model
// ---------------------------------------------------------------------------

/// A holder set: one id/`#tag` or a list of ids.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum HolderSetJson {
    Single(String),
    List(Vec<String>),
}

/// Vanilla `MinMaxBounds.Ints`.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum IntBoundsJson {
    Exact(i32),
    Bounds {
        #[serde(default)]
        min: Option<i32>,
        #[serde(default)]
        max: Option<i32>,
    },
}

/// Vanilla `MinMaxBounds.Doubles`.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum DoubleBoundsJson {
    Exact(f64),
    Bounds {
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },
}

/// A `minecraft:translate` text component, the only shape vanilla item names use.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct TextComponentJson {
    translate: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct BannerLayerJson {
    pattern: String,
    color: String,
}

/// One `LootItemCondition`, tagged by its `type`.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum ConditionJson {
    #[serde(rename = "minecraft:entity_properties")]
    EntityProperties {
        entity: String,
        predicate: EntityPredicateJson,
    },
    #[serde(rename = "minecraft:location_check")]
    LocationCheck {
        #[serde(default, rename = "offsetX")]
        offset_x: i32,
        #[serde(default, rename = "offsetY")]
        offset_y: i32,
        #[serde(default, rename = "offsetZ")]
        offset_z: i32,
        predicate: LocationPredicateJson,
    },
    #[serde(rename = "minecraft:match_block")]
    MatchBlock {
        #[serde(default)]
        blocks: Option<HolderSetJson>,
        #[serde(default)]
        state: BTreeMap<String, String>,
    },
    #[serde(rename = "minecraft:match_tool")]
    MatchTool { predicate: ItemPredicateJson },
    #[serde(rename = "minecraft:inverted")]
    Inverted { term: Box<Self> },
    #[serde(rename = "minecraft:any_of")]
    AnyOf { terms: Vec<Self> },
    #[serde(rename = "minecraft:all_of")]
    AllOf { terms: Vec<Self> },
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct ItemPredicateJson {
    items: Option<HolderSetJson>,
    count: Option<IntBoundsJson>,
    components: Option<BTreeMap<String, Value>>,
    predicates: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EntityPredicateJson {
    #[serde(rename = "minecraft:entity_type")]
    entity_type: Option<HolderSetJson>,
    #[serde(rename = "minecraft:location")]
    location: Option<LocationPredicateJson>,
    #[serde(rename = "minecraft:stepping_on")]
    stepping_on: Option<LocationPredicateJson>,
    #[serde(rename = "minecraft:distance")]
    distance: Option<DistancePredicateJson>,
    #[serde(rename = "minecraft:effects")]
    effects: Option<BTreeMap<String, EffectPredicateJson>>,
    #[serde(rename = "minecraft:flags")]
    flags: Option<EntityFlagsJson>,
    #[serde(rename = "minecraft:equipment")]
    equipment: Option<EntityEquipmentJson>,
    #[serde(rename = "minecraft:components")]
    components: Option<EntityComponentsJson>,
    #[serde(rename = "minecraft:vehicle")]
    vehicle: Option<Box<Self>>,
    #[serde(rename = "minecraft:passenger")]
    passenger: Option<Box<Self>>,
    #[serde(rename = "minecraft:type_specific/player")]
    player: Option<PlayerPredicateJson>,
    #[serde(rename = "minecraft:type_specific/lightning")]
    lightning: Option<LightningPredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EntityComponentsJson {
    #[serde(rename = "minecraft:cat/variant")]
    cat_variant: Option<String>,
    #[serde(rename = "minecraft:frog/variant")]
    frog_variant: Option<String>,
    #[serde(rename = "minecraft:wolf/variant")]
    wolf_variant: Option<String>,
    #[serde(rename = "minecraft:chicken/variant")]
    chicken_variant: Option<String>,
    #[serde(rename = "minecraft:sheep/color")]
    sheep_color: Option<String>,
    #[serde(rename = "minecraft:sheep/sheared")]
    sheep_sheared: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct PlayerPredicateJson {
    looking_at: Option<Box<EntityPredicateJson>>,
    recipes: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct LightningPredicateJson {
    blocks_set_on_fire: Option<IntBoundsJson>,
    entity_struck: Option<Box<EntityPredicateJson>>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct LocationPredicateJson {
    position: Option<PositionPredicateJson>,
    biomes: Option<HolderSetJson>,
    structures: Option<HolderSetJson>,
    dimension: Option<String>,
    light: Option<IntBoundsJson>,
    block: Option<BlockPredicateJson>,
    fluid: Option<String>,
    smokey: Option<bool>,
    can_see_sky: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct PositionPredicateJson {
    x: Option<DoubleBoundsJson>,
    y: Option<DoubleBoundsJson>,
    z: Option<DoubleBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct BlockPredicateJson {
    blocks: Option<HolderSetJson>,
    state: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct DistancePredicateJson {
    x: Option<DoubleBoundsJson>,
    y: Option<DoubleBoundsJson>,
    z: Option<DoubleBoundsJson>,
    horizontal: Option<DoubleBoundsJson>,
    absolute: Option<DoubleBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EffectPredicateJson {
    amplifier: Option<IntBoundsJson>,
    duration: Option<IntBoundsJson>,
    ambient: Option<bool>,
    visible: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EntityFlagsJson {
    is_on_ground: Option<bool>,
    is_on_fire: Option<bool>,
    is_sneaking: Option<bool>,
    is_sprinting: Option<bool>,
    is_swimming: Option<bool>,
    is_flying: Option<bool>,
    is_baby: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EntityEquipmentJson {
    head: Option<ItemPredicateJson>,
    chest: Option<ItemPredicateJson>,
    legs: Option<ItemPredicateJson>,
    feet: Option<ItemPredicateJson>,
    body: Option<ItemPredicateJson>,
    mainhand: Option<ItemPredicateJson>,
    offhand: Option<ItemPredicateJson>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct EnchantmentPredicateJson {
    #[serde(default)]
    enchantments: Option<HolderSetJson>,
    #[serde(default)]
    levels: Option<IntBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct DamagePredicateJson {
    dealt: Option<DoubleBoundsJson>,
    taken: Option<DoubleBoundsJson>,
    source_entity: Option<EntityPredicateJson>,
    blocked: Option<bool>,
    #[serde(rename = "type")]
    damage_type: Option<DamageSourcePredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct DamageSourcePredicateJson {
    tags: Option<Vec<DamageTagJson>>,
    source_entity: Option<EntityPredicateJson>,
    direct_entity: Option<EntityPredicateJson>,
    is_direct: Option<bool>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct DamageTagJson {
    id: String,
    expected: bool,
}

// --- per-trigger condition shapes -----------------------------------------

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct PlayerOnlyJson {
    player: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct ItemJson {
    player: Option<ConditionJson>,
    item: Option<ItemPredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EntityConditionJson {
    player: Option<ConditionJson>,
    entity: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct KilledJson {
    player: Option<ConditionJson>,
    entity: Option<ConditionJson>,
    killing_blow: Option<DamageSourcePredicateJson>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct RecipeUnlockedJson {
    #[serde(default)]
    player: Option<ConditionJson>,
    recipes: HolderSetJson,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct RecipeCraftedJson {
    #[serde(default)]
    player: Option<ConditionJson>,
    recipes: HolderSetJson,
    #[serde(default)]
    ingredients: Vec<ItemPredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct InventoryChangedJson {
    player: Option<ConditionJson>,
    slots: Option<SlotsJson>,
    items: Vec<ItemPredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct SlotsJson {
    occupied: Option<IntBoundsJson>,
    full: Option<IntBoundsJson>,
    empty: Option<IntBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct BredAnimalsJson {
    player: Option<ConditionJson>,
    parent: Option<ConditionJson>,
    partner: Option<ConditionJson>,
    child: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct ItemUsedOnLocationJson {
    player: Option<ConditionJson>,
    location: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct BlockStateJson {
    player: Option<ConditionJson>,
    blocks: Option<HolderSetJson>,
    state: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct BeeNestJson {
    player: Option<ConditionJson>,
    blocks: Option<HolderSetJson>,
    state: Option<BTreeMap<String, String>>,
    item: Option<ItemPredicateJson>,
    num_bees_inside: Option<IntBoundsJson>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct LootTableJson {
    #[serde(default)]
    player: Option<ConditionJson>,
    loot_tables: HolderSetJson,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct InteractJson {
    player: Option<ConditionJson>,
    item: Option<ItemPredicateJson>,
    entity: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct FishingJson {
    player: Option<ConditionJson>,
    rod: Option<ItemPredicateJson>,
    entity: Option<ConditionJson>,
    item: Option<ItemPredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct ChangedDimensionJson {
    player: Option<ConditionJson>,
    from: Option<String>,
    to: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct BrewedPotionJson {
    player: Option<ConditionJson>,
    potion: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct HurtEntityJson {
    player: Option<ConditionJson>,
    damage: Option<DamagePredicateJson>,
    entity: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EntityHurtPlayerJson {
    player: Option<ConditionJson>,
    damage: Option<DamagePredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct KilledByArrowJson {
    player: Option<ConditionJson>,
    victims: Vec<ConditionJson>,
    unique_entity_types: Option<IntBoundsJson>,
    fired_from_weapon: Option<ItemPredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct ChanneledLightningJson {
    player: Option<ConditionJson>,
    victims: Vec<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EffectsChangedJson {
    player: Option<ConditionJson>,
    effects: Option<BTreeMap<String, EffectPredicateJson>>,
    source: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct TradeJson {
    player: Option<ConditionJson>,
    villager: Option<ConditionJson>,
    item: Option<ItemPredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct PickedUpJson {
    player: Option<ConditionJson>,
    item: Option<ItemPredicateJson>,
    entity: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct BeaconJson {
    player: Option<ConditionJson>,
    level: Option<IntBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct TargetHitJson {
    player: Option<ConditionJson>,
    signal_strength: Option<i32>,
    projectile: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct SpearMobsJson {
    player: Option<ConditionJson>,
    count: Option<i32>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct DistanceTriggerJson {
    player: Option<ConditionJson>,
    start_position: Option<LocationPredicateJson>,
    distance: Option<DistancePredicateJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct FallAfterExplosionJson {
    player: Option<ConditionJson>,
    start_position: Option<LocationPredicateJson>,
    distance: Option<DistancePredicateJson>,
    cause: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct LevitationJson {
    player: Option<ConditionJson>,
    distance: Option<DistancePredicateJson>,
    duration: Option<IntBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct LightningStrikeJson {
    player: Option<ConditionJson>,
    lightning: Option<ConditionJson>,
    bystander: Option<ConditionJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct DurabilityJson {
    player: Option<ConditionJson>,
    item: Option<ItemPredicateJson>,
    durability: Option<IntBoundsJson>,
    delta: Option<IntBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct EnchantedJson {
    player: Option<ConditionJson>,
    item: Option<ItemPredicateJson>,
    levels: Option<IntBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct UsedEnderEyeJson {
    player: Option<ConditionJson>,
    distance: Option<DoubleBoundsJson>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, default)]
struct CuredJson {
    player: Option<ConditionJson>,
    zombie: Option<ConditionJson>,
    villager: Option<ConditionJson>,
}

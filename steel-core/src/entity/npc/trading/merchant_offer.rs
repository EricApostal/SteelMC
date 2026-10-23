//! Vanilla `MerchantOffer` and `MerchantOffers`.

use std::io::{Cursor, Result as IoResult, Write};
use std::ops::{Deref, DerefMut};
use std::slice::Iter;
use std::vec::IntoIter;

use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::item_stack::ItemStack;
use steel_utils::codec::VarInt;
use steel_utils::nbt::NbtNumeric as _;
use steel_utils::serial::{PrefixedRead as _, ReadFrom, WriteTo};

use crate::entity::npc::trading::item_cost::ItemCost;

/// One trade offer, mirroring vanilla `MerchantOffer`.
///
/// Vanilla keeps every field private behind accessors and only the `demand`,
/// `specialPriceDiff`, `uses` and `rewardExp` fields are mutable after construction.
#[derive(Debug, Clone)]
pub struct MerchantOffer {
    buy_a: ItemCost,
    buy_b: Option<ItemCost>,
    result: ItemStack,
    uses: i32,
    max_uses: i32,
    reward_exp: bool,
    special_price_diff: i32,
    demand: i32,
    price_multiplier: f32,
    xp: i32,
}

impl MerchantOffer {
    /// Vanilla `MerchantOffer(ItemCost, ItemStack, int maxUses, int xp, float priceMultiplier)`.
    #[must_use]
    pub fn new(
        buy: ItemCost,
        result: ItemStack,
        max_uses: i32,
        xp: i32,
        price_multiplier: f32,
    ) -> Self {
        Self::with_second_cost(buy, None, result, max_uses, xp, price_multiplier)
    }

    /// Vanilla `MerchantOffer(ItemCost, Optional<ItemCost>, ItemStack, int, int, float)`.
    #[must_use]
    pub fn with_second_cost(
        buy_a: ItemCost,
        buy_b: Option<ItemCost>,
        result: ItemStack,
        max_uses: i32,
        xp: i32,
        price_multiplier: f32,
    ) -> Self {
        Self::with_uses(buy_a, buy_b, result, 0, max_uses, xp, price_multiplier)
    }

    /// Vanilla `MerchantOffer(..., int uses, int maxUses, int xp, float priceMultiplier)`.
    #[must_use]
    pub fn with_uses(
        buy_a: ItemCost,
        buy_b: Option<ItemCost>,
        result: ItemStack,
        uses: i32,
        max_uses: i32,
        xp: i32,
        price_multiplier: f32,
    ) -> Self {
        Self::with_demand(
            buy_a,
            buy_b,
            result,
            uses,
            max_uses,
            xp,
            price_multiplier,
            0,
        )
    }

    /// Vanilla `MerchantOffer(..., int uses, int maxUses, int xp, float priceMultiplier, int demand)`.
    ///
    /// The public vanilla constructors always set `rewardExp` to `true` and
    /// `specialPriceDiff` to `0`.
    #[must_use]
    pub fn with_demand(
        buy_a: ItemCost,
        buy_b: Option<ItemCost>,
        result: ItemStack,
        uses: i32,
        max_uses: i32,
        xp: i32,
        price_multiplier: f32,
        demand: i32,
    ) -> Self {
        Self::from_parts(
            buy_a,
            buy_b,
            result,
            uses,
            max_uses,
            true,
            0,
            demand,
            price_multiplier,
            xp,
        )
    }

    /// Vanilla's private full constructor, used by the codecs.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors vanilla's private full `MerchantOffer` constructor"
    )]
    fn from_parts(
        buy_a: ItemCost,
        buy_b: Option<ItemCost>,
        result: ItemStack,
        uses: i32,
        max_uses: i32,
        reward_exp: bool,
        special_price_diff: i32,
        demand: i32,
        price_multiplier: f32,
        xp: i32,
    ) -> Self {
        Self {
            buy_a,
            buy_b,
            result,
            uses,
            max_uses,
            reward_exp,
            special_price_diff,
            demand,
            price_multiplier,
            xp,
        }
    }

    /// Vanilla `MerchantOffer.getBaseCostA`.
    #[must_use]
    pub const fn base_cost_a(&self) -> &ItemStack {
        self.buy_a.item_stack()
    }

    /// Vanilla `MerchantOffer.getCostA`: the first cost with the demand and
    /// special-price adjustments applied.
    #[must_use]
    pub fn cost_a(&self) -> ItemStack {
        self.buy_a
            .item_stack()
            .copy_with_count(self.get_modified_cost_count(&self.buy_a))
    }

    /// Vanilla `MerchantOffer.getModifiedCostCount`.
    fn get_modified_cost_count(&self, cost: &ItemCost) -> i32 {
        let base_price = cost.count();
        let demand_diff = ((base_price * self.demand) as f32 * self.price_multiplier)
            .floor()
            .max(0.0) as i32;
        (base_price + demand_diff + self.special_price_diff)
            .clamp(1, cost.item_stack().max_stack_size())
    }

    /// Vanilla `MerchantOffer.getCostB`.
    #[must_use]
    pub fn cost_b(&self) -> ItemStack {
        self.buy_b
            .as_ref()
            .map_or_else(ItemStack::empty, |cost| cost.item_stack().clone())
    }

    /// Vanilla `MerchantOffer.getItemCostA`.
    #[must_use]
    pub const fn item_cost_a(&self) -> &ItemCost {
        &self.buy_a
    }

    /// Vanilla `MerchantOffer.getItemCostB`.
    #[must_use]
    pub const fn item_cost_b(&self) -> Option<&ItemCost> {
        self.buy_b.as_ref()
    }

    /// Vanilla `MerchantOffer.getResult`.
    #[must_use]
    pub const fn result(&self) -> &ItemStack {
        &self.result
    }

    /// Vanilla `MerchantOffer.assemble`.
    #[must_use]
    pub fn assemble(&self) -> ItemStack {
        self.result.clone()
    }

    /// Vanilla `MerchantOffer.updateDemand`.
    pub fn update_demand(&mut self) {
        self.demand = self.demand + self.uses - (self.max_uses - self.uses);
    }

    /// The number of times this offer was used, vanilla `MerchantOffer.getUses`.
    #[must_use]
    pub const fn uses(&self) -> i32 {
        self.uses
    }

    /// Vanilla `MerchantOffer.resetUses`.
    pub const fn reset_uses(&mut self) {
        self.uses = 0;
    }

    /// The maximum number of uses before restocking, vanilla `MerchantOffer.getMaxUses`.
    #[must_use]
    pub const fn max_uses(&self) -> i32 {
        self.max_uses
    }

    /// Vanilla `MerchantOffer.increaseUses`.
    pub const fn increase_uses(&mut self) {
        self.uses += 1;
    }

    /// The demand-driven price adjustment, vanilla `MerchantOffer.getDemand`.
    #[must_use]
    pub const fn demand(&self) -> i32 {
        self.demand
    }

    /// Vanilla `MerchantOffer.addToSpecialPriceDiff`.
    pub const fn add_to_special_price_diff(&mut self, add: i32) {
        self.special_price_diff += add;
    }

    /// Vanilla `MerchantOffer.resetSpecialPriceDiff`.
    pub const fn reset_special_price_diff(&mut self) {
        self.special_price_diff = 0;
    }

    /// The special price modifier applied on top of demand, vanilla `MerchantOffer.getSpecialPriceDiff`.
    #[must_use]
    pub const fn special_price_diff(&self) -> i32 {
        self.special_price_diff
    }

    /// Vanilla `MerchantOffer.setSpecialPriceDiff`.
    pub const fn set_special_price_diff(&mut self, value: i32) {
        self.special_price_diff = value;
    }

    /// The price multiplier applied to demand, vanilla `MerchantOffer.getPriceMultiplier`.
    #[must_use]
    pub const fn price_multiplier(&self) -> f32 {
        self.price_multiplier
    }

    /// The villager experience granted per trade, vanilla `MerchantOffer.getXp`.
    #[must_use]
    pub const fn xp(&self) -> i32 {
        self.xp
    }

    /// Vanilla `MerchantOffer.isOutOfStock`.
    #[must_use]
    pub const fn is_out_of_stock(&self) -> bool {
        self.uses >= self.max_uses
    }

    /// Vanilla `MerchantOffer.setToOutOfStock`.
    pub const fn set_to_out_of_stock(&mut self) {
        self.uses = self.max_uses;
    }

    /// Vanilla `MerchantOffer.needsRestock`.
    #[must_use]
    pub const fn needs_restock(&self) -> bool {
        self.uses > 0
    }

    /// Vanilla `MerchantOffer.shouldRewardExp`.
    #[must_use]
    pub const fn should_reward_exp(&self) -> bool {
        self.reward_exp
    }

    /// Vanilla `MerchantOffer.satisfiedBy`.
    #[must_use]
    pub fn satisfied_by(&self, buy_a: &ItemStack, buy_b: &ItemStack) -> bool {
        if !self.buy_a.test(buy_a) || buy_a.count() < self.get_modified_cost_count(&self.buy_a) {
            return false;
        }

        match &self.buy_b {
            None => buy_b.is_empty(),
            Some(cost_b) => cost_b.test(buy_b) && buy_b.count() >= cost_b.count(),
        }
    }

    /// Vanilla `MerchantOffer.take`.
    ///
    /// Takes vanilla's first cost, which is `cost_a` without materializing it.
    pub fn take(&self, buy_a: &mut ItemStack, buy_b: &mut ItemStack) -> bool {
        if !self.satisfied_by(buy_a, buy_b) {
            return false;
        }

        buy_a.shrink(self.get_modified_cost_count(&self.buy_a));
        if let Some(cost_b) = &self.buy_b {
            buy_b.shrink(cost_b.count());
        }

        true
    }

    /// Vanilla `MerchantOffer.copy`: a new result stack, shared costs.
    #[must_use]
    pub fn copy(&self) -> Self {
        Self::from_parts(
            self.buy_a.clone(),
            self.buy_b.clone(),
            self.result.clone(),
            self.uses,
            self.max_uses,
            self.reward_exp,
            self.special_price_diff,
            self.demand,
            self.price_multiplier,
            self.xp,
        )
    }

    /// Writes vanilla `MerchantOffer.CODEC`.
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.insert("buy", self.buy_a.to_nbt());
        if let Some(buy_b) = &self.buy_b {
            compound.insert("buyB", buy_b.to_nbt());
        }
        compound.insert("sell", self.result.to_nbt_tag_ref());
        if self.uses != 0 {
            compound.insert("uses", self.uses);
        }
        if self.max_uses != 4 {
            compound.insert("maxUses", self.max_uses);
        }
        if !self.reward_exp {
            compound.insert("rewardExp", false);
        }
        if self.special_price_diff != 0 {
            compound.insert("specialPrice", self.special_price_diff);
        }
        if self.demand != 0 {
            compound.insert("demand", self.demand);
        }
        if self.price_multiplier != 0.0 {
            compound.insert("priceMultiplier", self.price_multiplier);
        }
        if self.xp != 1 {
            compound.insert("xp", self.xp);
        }
        compound
    }

    /// Reads vanilla `MerchantOffer.CODEC`.
    #[must_use]
    pub fn from_nbt(nbt: &NbtCompoundView<'_, '_>) -> Option<Self> {
        let buy = ItemCost::from_nbt(&nbt.compound("buy")?)?;
        let buy_b = match nbt.compound("buyB") {
            Some(compound) => Some(ItemCost::from_nbt(&compound)?),
            None => None,
        };
        let result = ItemStack::from_borrowed_compound(&nbt.compound("sell")?)?;
        Some(Self::from_parts(
            buy,
            buy_b,
            result,
            int_or(nbt, "uses", 0),
            int_or(nbt, "maxUses", 4),
            bool_or(nbt, "rewardExp", true),
            int_or(nbt, "specialPrice", 0),
            int_or(nbt, "demand", 0),
            float_or(nbt, "priceMultiplier", 0.0),
            int_or(nbt, "xp", 1),
        ))
    }
}

/// Lenient optional field lookup, matching Mojang's `lenientOptionalFieldOf`: a
/// field that is present but not decodable falls back to its default.
fn int_or(nbt: &NbtCompoundView<'_, '_>, name: &str, default: i32) -> i32 {
    nbt.get(name)
        .and_then(|tag| tag.codec_i32())
        .unwrap_or(default)
}

fn float_or(nbt: &NbtCompoundView<'_, '_>, name: &str, default: f32) -> f32 {
    nbt.get(name)
        .and_then(|tag| tag.codec_f32())
        .unwrap_or(default)
}

fn bool_or(nbt: &NbtCompoundView<'_, '_>, name: &str, default: bool) -> bool {
    nbt.get(name)
        .and_then(|tag| tag.codec_bool())
        .unwrap_or(default)
}

impl WriteTo for MerchantOffer {
    /// Vanilla `MerchantOffer.STREAM_CODEC`.
    fn write(&self, writer: &mut impl Write) -> IoResult<()> {
        self.buy_a.write(writer)?;
        self.result.write(writer)?;
        self.buy_b.write(writer)?;
        self.is_out_of_stock().write(writer)?;
        self.uses.write(writer)?;
        self.max_uses.write(writer)?;
        self.xp.write(writer)?;
        self.special_price_diff.write(writer)?;
        self.price_multiplier.write(writer)?;
        self.demand.write(writer)
    }
}

impl ReadFrom for MerchantOffer {
    fn read(data: &mut Cursor<&[u8]>) -> IoResult<Self> {
        let buy = ItemCost::read(data)?;
        let sell = ItemStack::read(data)?;
        let buy_b = Option::<ItemCost>::read(data)?;
        let exhausted = bool::read(data)?;
        let uses = i32::read(data)?;
        let max_uses = i32::read(data)?;
        let xp = i32::read(data)?;
        let special_price_diff = i32::read(data)?;
        let price_multiplier = f32::read(data)?;
        let demand = i32::read(data)?;

        let mut offer = Self::with_demand(
            buy,
            buy_b,
            sell,
            uses,
            max_uses,
            xp,
            price_multiplier,
            demand,
        );
        if exhausted {
            offer.set_to_out_of_stock();
        }
        offer.set_special_price_diff(special_price_diff);
        Ok(offer)
    }
}

/// A villager's offer list, mirroring vanilla `MerchantOffers extends ArrayList<MerchantOffer>`.
///
/// Derefs to the underlying list, so iteration, indexing and mutation use the
/// standard slice/`Vec` API.
#[derive(Debug, Clone, Default)]
pub struct MerchantOffers(Vec<MerchantOffer>);

impl MerchantOffers {
    /// Creates an empty offer list.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Vanilla `MerchantOffers.getRecipeFor`.
    #[must_use]
    pub fn get_recipe_for(
        &self,
        buy_a: &ItemStack,
        buy_b: &ItemStack,
        selection_hint: i32,
    ) -> Option<&MerchantOffer> {
        self.get_recipe_index(buy_a, buy_b, selection_hint)
            .map(|index| &self.0[index])
    }

    /// Vanilla `MerchantOffers.getRecipeFor`, returning the canonical offer so the
    /// caller can mutate it in place before `Merchant::notify_trade`.
    #[must_use]
    pub fn get_recipe_for_mut(
        &mut self,
        buy_a: &ItemStack,
        buy_b: &ItemStack,
        selection_hint: i32,
    ) -> Option<&mut MerchantOffer> {
        self.get_recipe_index(buy_a, buy_b, selection_hint)
            .map(|index| &mut self.0[index])
    }

    fn get_recipe_index(
        &self,
        buy_a: &ItemStack,
        buy_b: &ItemStack,
        selection_hint: i32,
    ) -> Option<usize> {
        if selection_hint > 0
            && let Ok(hint) = usize::try_from(selection_hint)
            && hint < self.0.len()
            && self.0[hint].satisfied_by(buy_a, buy_b)
        {
            return Some(hint);
        }

        (0..self.0.len()).find(|&index| self.0[index].satisfied_by(buy_a, buy_b))
    }

    /// Vanilla `MerchantOffers.copy`.
    #[must_use]
    pub fn copy(&self) -> Self {
        Self(self.0.iter().map(MerchantOffer::copy).collect())
    }

    /// Writes vanilla `MerchantOffers.CODEC` (`{Recipes: [...]}`).
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        if !self.0.is_empty() {
            compound.insert(
                "Recipes",
                NbtList::Compound(self.0.iter().map(MerchantOffer::to_nbt).collect()),
            );
        }
        compound
    }

    /// Reads vanilla `MerchantOffers.CODEC`.
    #[must_use]
    pub fn from_nbt(nbt: &NbtCompoundView<'_, '_>) -> Option<Self> {
        let Some(list) = nbt.list("Recipes") else {
            return Some(Self::new());
        };
        let compounds = list.compounds()?;
        let offers = compounds
            .into_iter()
            .map(|compound| MerchantOffer::from_nbt(&compound))
            .collect::<Option<Vec<_>>>()?;
        Some(Self(offers))
    }
}

impl Deref for MerchantOffers {
    type Target = Vec<MerchantOffer>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MerchantOffers {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl FromIterator<MerchantOffer> for MerchantOffers {
    fn from_iter<T: IntoIterator<Item = MerchantOffer>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl IntoIterator for MerchantOffers {
    type Item = MerchantOffer;
    type IntoIter = IntoIter<MerchantOffer>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a MerchantOffers {
    type Item = &'a MerchantOffer;
    type IntoIter = Iter<'a, MerchantOffer>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl WriteTo for MerchantOffers {
    /// Vanilla `MerchantOffers.STREAM_CODEC`.
    fn write(&self, writer: &mut impl Write) -> IoResult<()> {
        self.0.write(writer)
    }
}

impl ReadFrom for MerchantOffers {
    fn read(data: &mut Cursor<&[u8]>) -> IoResult<Self> {
        Ok(Self(Vec::<MerchantOffer>::read_prefixed::<VarInt>(data)?))
    }
}

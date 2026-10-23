//! Clientbound merchant offers packet.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::{
    RegistryEntry, data_component_predicate::DataComponentExactPredicate, item_stack::ItemStack,
    items::ItemRef, packets::play::C_MERCHANT_OFFERS,
};
use steel_utils::{codec::VarInt, serial::WriteTo};

/// Wire form of vanilla `ItemCost`: the wanted item, its count, and the exact
/// component values it must carry.
#[derive(Clone, Debug)]
pub struct MerchantItemCost {
    /// Wanted item.
    pub item: ItemRef,
    /// Wanted count.
    pub count: i32,
    /// Exact component values the item must carry.
    pub components: DataComponentExactPredicate,
}

impl WriteTo for MerchantItemCost {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.item.id() as i32).write(writer)?;
        VarInt(self.count).write(writer)?;
        self.components.write(writer)
    }
}

/// Wire form of one vanilla `MerchantOffer`.
///
/// Mirrors vanilla's `MerchantOffer.STREAM_CODEC`: cost A, result, optional
/// cost B, then the out-of-stock flag and the pricing/usage counters.
#[derive(Clone, Debug)]
pub struct MerchantOffer {
    /// Primary cost.
    pub cost_a: MerchantItemCost,
    /// Result given to the player.
    pub result: ItemStack,
    /// Optional secondary cost.
    pub cost_b: Option<MerchantItemCost>,
    /// Whether the offer is exhausted (`uses >= max_uses`).
    pub out_of_stock: bool,
    /// Uses so far.
    pub uses: i32,
    /// Uses allowed before restocking.
    pub max_uses: i32,
    /// Experience granted per trade.
    pub xp: i32,
    /// Reputation/preference price adjustment.
    pub special_price_diff: i32,
    /// Demand-driven price multiplier.
    pub price_multiplier: f32,
    /// Accumulated demand.
    pub demand: i32,
}

impl WriteTo for MerchantOffer {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.cost_a.write(writer)?;
        self.result.write(writer)?;
        // Vanilla's `ItemCost.OPTIONAL_STREAM_CODEC` is `ByteBufCodecs.optional`.
        match &self.cost_b {
            Some(cost) => {
                writer.write_all(&[1])?;
                cost.write(writer)?;
            }
            None => writer.write_all(&[0])?,
        }
        writer.write_all(&[u8::from(self.out_of_stock)])?;
        self.uses.write(writer)?;
        self.max_uses.write(writer)?;
        self.xp.write(writer)?;
        self.special_price_diff.write(writer)?;
        self.price_multiplier.write(writer)?;
        self.demand.write(writer)
    }
}

/// Sends a merchant's offers to the client.
///
/// Matches vanilla's `ClientboundMerchantOffersPacket`.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_MERCHANT_OFFERS)]
pub struct CMerchantOffers {
    /// Container id of the open merchant menu.
    pub container_id: i32,
    /// Offers in display order.
    pub offers: Vec<MerchantOffer>,
    /// Villager level shown in the trading screen.
    pub villager_level: i32,
    /// Villager experience.
    pub villager_xp: i32,
    /// Whether the trading screen shows the level progress bar.
    pub show_progress: bool,
    /// Whether the merchant can restock (wandering traders cannot).
    pub can_restock: bool,
}

impl CMerchantOffers {
    /// Creates a new merchant offers packet.
    #[must_use]
    pub const fn new(
        container_id: i32,
        offers: Vec<MerchantOffer>,
        villager_level: i32,
        villager_xp: i32,
        show_progress: bool,
        can_restock: bool,
    ) -> Self {
        Self {
            container_id,
            offers,
            villager_level,
            villager_xp,
            show_progress,
            can_restock,
        }
    }
}

impl WriteTo for CMerchantOffers {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.container_id).write(writer)?;
        // Vanilla's `MerchantOffers.STREAM_CODEC` uses `ByteBufCodecs.collection`,
        // which prefixes the list with its element count as a VarInt.
        VarInt(self.offers.len() as i32).write(writer)?;
        for offer in &self.offers {
            offer.write(writer)?;
        }
        VarInt(self.villager_level).write(writer)?;
        VarInt(self.villager_xp).write(writer)?;
        writer.write_all(&[u8::from(self.show_progress)])?;
        writer.write_all(&[u8::from(self.can_restock)])
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, item_stack::ItemStack, vanilla_items};

    use super::*;

    /// Standard LEB128 VarInt encoding, written independently of `steel_utils`
    /// so the expected buffer is not tautological.
    fn varint(value: i32) -> Vec<u8> {
        let mut value = value as u32;
        let mut bytes = Vec::new();
        loop {
            if value & !0x7F == 0 {
                bytes.push(value as u8);
                return bytes;
            }
            bytes.push((value as u8 & 0x7F) | 0x80);
            value >>= 7;
        }
    }

    fn encode(packet: &CMerchantOffers) -> Vec<u8> {
        let mut bytes = Vec::new();
        packet.write(&mut bytes).expect("packet should encode");
        bytes
    }

    #[test]
    fn writes_one_out_of_stock_offer_byte_exactly() {
        init_vanilla_registry();

        let packet = CMerchantOffers::new(
            7,
            vec![MerchantOffer {
                cost_a: MerchantItemCost {
                    item: &vanilla_items::EMERALD,
                    count: 3,
                    components: DataComponentExactPredicate::EMPTY,
                },
                result: ItemStack::new(&vanilla_items::EMERALD),
                cost_b: None,
                out_of_stock: true,
                uses: 4,
                max_uses: 4,
                xp: 10,
                special_price_diff: -2,
                price_multiplier: 0.05,
                demand: 6,
            }],
            2,
            42,
            true,
            false,
        );

        let mut expected = Vec::new();
        expected.extend(varint(7)); // container id
        expected.extend(varint(1)); // offer count
        expected.extend(varint(vanilla_items::EMERALD.id() as i32)); // cost A item
        expected.extend(varint(3)); // cost A count
        expected.extend(varint(0)); // empty exact component predicate
        expected.extend(varint(1)); // result count
        expected.extend(varint(vanilla_items::EMERALD.id() as i32)); // result item
        expected.extend(varint(0)); // empty result component patch (added)
        expected.extend(varint(0)); // empty result component patch (removed)
        expected.push(0); // no second cost
        expected.push(1); // out of stock
        expected.extend(4i32.to_be_bytes()); // uses
        expected.extend(4i32.to_be_bytes()); // max uses
        expected.extend(10i32.to_be_bytes()); // xp
        expected.extend((-2i32).to_be_bytes()); // special price diff
        expected.extend(0.05f32.to_be_bytes()); // price multiplier
        expected.extend(6i32.to_be_bytes()); // demand
        expected.extend(varint(2)); // villager level
        expected.extend(varint(42)); // villager xp
        expected.push(1); // show progress
        expected.push(0); // can restock

        assert_eq!(encode(&packet), expected);
    }

    #[test]
    fn writes_second_cost_presence_and_payload() {
        init_vanilla_registry();

        let packet = CMerchantOffers::new(
            0,
            vec![MerchantOffer {
                cost_a: MerchantItemCost {
                    item: &vanilla_items::WHEAT,
                    count: 20,
                    components: DataComponentExactPredicate::EMPTY,
                },
                result: ItemStack::new(&vanilla_items::EMERALD),
                cost_b: Some(MerchantItemCost {
                    item: &vanilla_items::EMERALD,
                    count: 1,
                    components: DataComponentExactPredicate::EMPTY,
                }),
                out_of_stock: false,
                uses: 0,
                max_uses: 12,
                xp: 1,
                special_price_diff: 0,
                price_multiplier: 0.0,
                demand: 0,
            }],
            1,
            0,
            false,
            true,
        );

        let mut expected = Vec::new();
        expected.extend(varint(0)); // container id
        expected.extend(varint(1)); // offer count
        expected.extend(varint(vanilla_items::WHEAT.id() as i32)); // cost A item
        expected.extend(varint(20)); // cost A count
        expected.extend(varint(0)); // cost A empty components
        expected.extend(varint(1)); // result count
        expected.extend(varint(vanilla_items::EMERALD.id() as i32)); // result item
        expected.extend(varint(0)); // result component patch (added)
        expected.extend(varint(0)); // result component patch (removed)
        expected.push(1); // second cost present
        expected.extend(varint(vanilla_items::EMERALD.id() as i32)); // cost B item
        expected.extend(varint(1)); // cost B count
        expected.extend(varint(0)); // cost B empty components
        expected.push(0); // not out of stock
        expected.extend(0i32.to_be_bytes()); // uses
        expected.extend(12i32.to_be_bytes()); // max uses
        expected.extend(1i32.to_be_bytes()); // xp
        expected.extend(0i32.to_be_bytes()); // special price diff
        expected.extend(0.0f32.to_be_bytes()); // price multiplier
        expected.extend(0i32.to_be_bytes()); // demand
        expected.extend(varint(1)); // villager level
        expected.extend(varint(0)); // villager xp
        expected.push(0); // show progress
        expected.push(1); // can restock

        assert_eq!(encode(&packet), expected);
    }

    #[test]
    fn packet_id_is_vanilla_merchant_offers() {
        use steel_registry::packets::play::C_MERCHANT_OFFERS as ID;

        use crate::packet_traits::ClientPacket as _;
        use crate::utils::ConnectionProtocol;

        let packet = CMerchantOffers::new(0, Vec::new(), 0, 0, false, false);
        assert_eq!(packet.get_id(ConnectionProtocol::Play), Some(ID));
        assert_eq!(ID, 53);
    }
}

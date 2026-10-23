use std::io::Write;
use steel_macros::ClientPacket;
use steel_registry::packets::play::C_AWARD_STATS;
use steel_registry::stat::Stat;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

/// Clientbound packet that updates the client with the stats that have been
/// marked dirty.
///
/// Vanilla `ClientboundAwardStatsPacket` carries the whole dirty batch as one
/// map, so a single packet is sent per flush regardless of how many counters
/// changed. The wire layout is a `VarInt` entry count followed by
/// `stat`/`VarInt` count pairs.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_AWARD_STATS)]
pub struct CAwardStats {
    /// The dirty stats to send.
    pub stats: Vec<(Stat, i32)>,
}

impl CAwardStats {
    /// Creates a packet that awards a batch of stats.
    #[must_use]
    pub const fn new(stats: Vec<(Stat, i32)>) -> Self {
        Self { stats }
    }

    /// Creates a packet that awards one stat.
    #[must_use]
    pub fn single(stat: Stat, count: i32) -> Self {
        Self {
            stats: vec![(stat, count)],
        }
    }
}

impl WriteTo for CAwardStats {
    fn write(&self, writer: &mut impl Write) -> std::io::Result<()> {
        VarInt(self.stats.len() as i32).write(writer)?;
        for (stat, count) in &self.stats {
            stat.write(writer)?;
            VarInt(*count).write(writer)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::stat::vanilla_stat_types;
    use steel_registry::{
        RegistryEntry, init_vanilla_registry, vanilla_custom_stats, vanilla_items,
    };
    use steel_utils::codec::VarInt;
    use steel_utils::serial::WriteTo;

    use super::CAwardStats;

    fn encode(packet: &CAwardStats) -> Vec<u8> {
        let mut bytes = Vec::new();
        packet
            .write(&mut bytes)
            .expect("award stats packet should encode");
        bytes
    }

    fn varint(value: i32) -> Vec<u8> {
        let mut bytes = Vec::new();
        VarInt(value)
            .write(&mut bytes)
            .expect("varint should encode");
        bytes
    }

    #[test]
    fn single_stat_encoding_matches_vanilla_map_layout() {
        init_vanilla_registry();

        let stat = vanilla_stat_types::ITEM_USED.get(&vanilla_items::DIAMOND);
        let encoded = encode(&CAwardStats::single(stat, 3));

        let mut expected = varint(1);
        expected.extend(varint(
            vanilla_stat_types::ITEM_USED.stat_type_entry_ref().id() as i32,
        ));
        expected.extend(varint(vanilla_items::DIAMOND.id() as i32));
        expected.extend(varint(3));

        assert_eq!(encoded, expected);
    }

    #[test]
    fn multi_stat_batch_encodes_every_entry_after_one_length_prefix() {
        init_vanilla_registry();

        let jump = vanilla_stat_types::CUSTOM.get(&vanilla_custom_stats::JUMP);
        let diamond_used = vanilla_stat_types::ITEM_USED.get(&vanilla_items::DIAMOND);
        let packet = CAwardStats::new(vec![(jump, 7), (diamond_used, -1)]);
        let encoded = encode(&packet);

        let mut expected = varint(2);
        expected.extend(varint(
            vanilla_stat_types::CUSTOM.stat_type_entry_ref().id() as i32,
        ));
        expected.extend(varint(vanilla_custom_stats::JUMP.id() as i32));
        expected.extend(varint(7));
        expected.extend(varint(
            vanilla_stat_types::ITEM_USED.stat_type_entry_ref().id() as i32,
        ));
        expected.extend(varint(vanilla_items::DIAMOND.id() as i32));
        expected.extend(varint(-1));

        assert_eq!(encoded, expected);
    }

    #[test]
    fn empty_batch_still_writes_a_zero_length_prefix() {
        init_vanilla_registry();

        assert_eq!(encode(&CAwardStats::new(Vec::new())), varint(0));
    }
}

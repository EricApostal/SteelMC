use steel_macros::{ClientPacket, WriteTo};
use steel_registry::advancement::AdvancementProgressData;
use steel_registry::advancement::registry::AdvancementNodeRef;
use steel_registry::packets::play::C_UPDATE_ADVANCEMENTS;
use steel_utils::Identifier;

/// Updates the client advancement tree and progress.
///
/// Mirrors vanilla `ClientboundUpdateAdvancementsPacket`: `shouldReset`,
/// `added` (positioned advancements), `removed`, `progress` and
/// `showAdvancements`, in that wire order.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_UPDATE_ADVANCEMENTS)]
pub struct CUpdateAdvancements {
    /// Whether the client must clear every advancement it already knows.
    pub reset: bool,
    /// Advancements that became visible, with their tree position.
    pub added: Vec<AdvancementNodeRef>,
    /// Advancements that are no longer visible.
    pub removed: Vec<Identifier>,
    /// Criteria progress for advancements the player already had progress on.
    pub progress: Vec<AdvancementProgressData>,
    /// Whether the client should display the advancement toast.
    pub show_advancements: bool,
}

impl CUpdateAdvancements {
    /// Creates a new update advancement packet.
    ///
    /// The parameter order matches the wire order used by vanilla
    /// (`shouldReset`, `added`, `removed`, `progress`, `showAdvancements`).
    #[must_use]
    pub const fn new(
        reset: bool,
        added: Vec<AdvancementNodeRef>,
        removed: Vec<Identifier>,
        progress: Vec<AdvancementProgressData>,
        show_advancements: bool,
    ) -> Self {
        Self {
            reset,
            added,
            removed,
            progress,
            show_advancements,
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::advancement::Criteria;
    use steel_registry::advancement::registry::AdvancementNodeRef;
    use steel_registry::{REGISTRY, init_vanilla_registry};
    use steel_utils::Identifier;
    use steel_utils::codec::VarInt;
    use steel_utils::serial::WriteTo;

    use super::CUpdateAdvancements;

    fn encode(packet: &CUpdateAdvancements) -> Vec<u8> {
        let mut bytes = Vec::new();
        packet
            .write(&mut bytes)
            .expect("update advancements packet should encode");
        bytes
    }

    fn write(value: &impl WriteTo) -> Vec<u8> {
        let mut bytes = Vec::new();
        value
            .write(&mut bytes)
            .expect("value should encode into a vec");
        bytes
    }

    fn story_root() -> AdvancementNodeRef {
        let id = Identifier::vanilla_static("story/root");
        REGISTRY
            .advancements
            .by_key(&id)
            .expect("story/root should be registered")
    }

    #[test]
    fn writes_fields_in_vanilla_wire_order() {
        init_vanilla_registry();

        let node = story_root();
        let removed = Identifier::vanilla_static("story/obtain_armor");
        let progress = steel_registry::advancement::AdvancementProgressData {
            id: Identifier::vanilla_static("story/root"),
            progress: vec![Criteria {
                criterion_id: "crafting_table".into(),
                achieve_date: Some(1_700_000_000_000),
            }],
        };
        let packet = CUpdateAdvancements::new(
            true,
            vec![node],
            vec![removed.clone()],
            vec![progress],
            false,
        );
        let encoded = encode(&packet);

        let mut expected = Vec::new();
        expected.extend(write(&true));
        expected.extend(write(&vec![node]));
        expected.extend(write(&vec![removed]));
        expected.extend(write(&vec![progress]));
        expected.extend(write(&false));

        assert_eq!(encoded, expected);
    }

    #[test]
    fn added_nodes_write_the_holder_id_first_and_the_tree_position_last() {
        init_vanilla_registry();

        let node = story_root();
        let encoded = encode(&CUpdateAdvancements::new(
            true,
            vec![node],
            Vec::new(),
            Vec::new(),
            true,
        ));

        let (x, y) = node.get_location();
        let key = write(node.value().key());

        // `shouldReset` (1 byte) + `added` length (VarInt) + holder id + ...
        let after_reset_and_len = 1 + VarInt::written_size(1);
        assert_eq!(
            &encoded[after_reset_and_len..after_reset_and_len + key.len()],
            key.as_slice(),
            "a positioned advancement starts with its advancement holder id"
        );

        // Trailing fields after `added`: empty `removed`, empty `progress`, then
        // `showAdvancements`.
        let trailing = VarInt::written_size(0) + VarInt::written_size(0) + 1;
        let mut tail = x.to_be_bytes().to_vec();
        tail.extend(y.to_be_bytes());
        let start = encoded.len() - trailing - tail.len();
        assert_eq!(
            &encoded[start..start + tail.len()],
            tail.as_slice(),
            "a positioned advancement ends with x then y"
        );
    }
}

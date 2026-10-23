//! Serverbound packet for saving a jigsaw block's settings.

use std::io::Cursor;

use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::play::S_SET_JIGSAW_BLOCK;
use steel_registry::template_pool::JointType;
use steel_utils::{BlockPos, Identifier, codec::VarInt, serial::prefixed_read::read_utf};

/// Maximum UTF-16 code units for the plain strings (vanilla `readUtf()`).
pub const MAX_JIGSAW_STRING_LENGTH: usize = 32_767;

/// Sent when a player saves a jigsaw block from its edit screen.
///
/// Matches vanilla's `ServerboundSetJigsawBlockPacket`: the block position, the
/// name, target and pool identifiers, the final-state string, the joint type
/// as its serialized name string (`"rollable"` / `"aligned"`, falling back to
/// `Aligned` on unknown names), then the selection and placement priorities as
/// `VarInt`s (in that order).
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_SET_JIGSAW_BLOCK)]
pub struct SSetJigsawBlock {
    /// Position of the edited jigsaw block.
    pub pos: BlockPos,
    /// Name of this jigsaw connector.
    pub name: Identifier,
    /// Target connector name to attach to.
    pub target: Identifier,
    /// Template pool to draw pieces from.
    pub pool: Identifier,
    /// Block state to replace the jigsaw with after placement.
    pub final_state: String,
    /// How this jigsaw connects to its partner.
    pub joint: JointType,
    /// Priority for selecting this jigsaw among siblings (higher = tried first).
    pub selection_priority: i32,
    /// Priority for BFS queue ordering when placing children.
    pub placement_priority: i32,
}

impl steel_utils::serial::ReadFrom for SSetJigsawBlock {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let pos = BlockPos::read(data)?;
        let name = Identifier::read(data)?;
        let target = Identifier::read(data)?;
        let pool = Identifier::read(data)?;
        let final_state = read_utf(data, MAX_JIGSAW_STRING_LENGTH)?;
        // Vanilla `JigsawBlockEntity.JointType.CODEC.byName(name, ALIGNED)`.
        let joint_name = read_utf(data, MAX_JIGSAW_STRING_LENGTH)?;
        let joint = match joint_name.as_str() {
            "rollable" => JointType::Rollable,
            _ => JointType::Aligned,
        };
        let selection_priority = VarInt::read(data)?.0;
        let placement_priority = VarInt::read(data)?.0;

        Ok(Self {
            pos,
            name,
            target,
            pool,
            final_state,
            joint,
            selection_priority,
            placement_priority,
        })
    }
}

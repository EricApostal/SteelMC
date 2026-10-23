//! Vanilla biome-region fill command.

use steel_registry::{
    REGISTRY, RegistryEntry as _, RegistryExt as _, biome::BiomeRef,
    vanilla_game_rules::MAX_BLOCK_MODIFICATIONS,
};
use steel_utils::{BlockPos, BoundingBox, Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        BiomeOrTag, CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        argument, literal,
    },
    registration::CommandRegistration,
};
use super::execute::loaded_block_position;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("fillbiome"), |_| command())
}

fn command() -> Builder {
    literal("fillbiome").then(
        argument("from", SteelArgumentType::block_pos()).then(
            argument("to", SteelArgumentType::block_pos()).then(
                argument("biome", SteelArgumentType::biome_or_tag())
                    .executes(|context| execute_fill(context, None))
                    .then(
                        literal("replace").then(
                            argument("filter", SteelArgumentType::biome_or_tag())
                                .executes(|context| execute_fill(context, Some("filter"))),
                        ),
                    ),
            ),
        ),
    )
}

fn execute_fill(
    context: &SteelCommandContext<CommandSource>,
    filter_argument: Option<&'static str>,
) -> Result<i32, CommandSyntaxError> {
    let raw_from = loaded_block_position(context, "from")?;
    let raw_to = loaded_block_position(context, "to")?;
    let biome = concrete_biome(context, "biome")?;
    let filter = filter_argument
        .map(|name| context.biome_or_tag(name))
        .transpose()?;

    fill_biomes(context.source(), raw_from, raw_to, biome, filter)
}

/// Resolves a `biome` argument that vanilla parses with `ResourceArgument`, which
/// rejects biome tags.
fn concrete_biome(
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<BiomeRef, CommandSyntaxError> {
    match context.biome_or_tag(name)? {
        BiomeOrTag::Biome(biome) => Ok(biome),
        BiomeOrTag::Tag(tag) => Err(unknown_biome(&format!("#{tag}"))),
    }
}

fn unknown_biome(name: &str) -> CommandSyntaxError {
    let message = translations::ARGUMENT_RESOURCE_NOT_FOUND
        .message([name.to_owned(), "minecraft:biome".to_owned()])
        .component();
    CommandSyntaxError::dynamic(message)
}

/// Vanilla `QuartPos.toBlock(QuartPos.fromBlock(coord))`.
const fn quantize(block_coord: i32) -> i32 {
    (block_coord >> 2) << 2
}

/// Vanilla `FillBiomeCommand.fill`, including the biome resend to tracking clients.
fn fill_biomes(
    source: &CommandSource,
    raw_from: BlockPos,
    raw_to: BlockPos,
    biome: BiomeRef,
    filter: Option<&BiomeOrTag>,
) -> Result<i32, CommandSyntaxError> {
    let world = source.world();
    let from = BlockPos::new(
        quantize(raw_from.x()),
        quantize(raw_from.y()),
        quantize(raw_from.z()),
    );
    let to = BlockPos::new(
        quantize(raw_to.x()),
        quantize(raw_to.y()),
        quantize(raw_to.z()),
    );
    let region = BoundingBox::from_corners(from, to);

    let volume = i64::from(region.width()) * i64::from(region.height()) * i64::from(region.depth());
    let limit = world.get_game_rule(&MAX_BLOCK_MODIFICATIONS);
    if volume > i64::from(limit) {
        let message = translations::COMMANDS_FILLBIOME_TOOBIG
            .message([
                TextComponent::from(limit.to_string()),
                TextComponent::from(volume.to_string()),
            ])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }

    let target_id = biome
        .try_id()
        .and_then(|id| u16::try_from(id).ok())
        .ok_or_else(|| unknown_biome(&biome.key.to_string()))?;
    let Some(result) = world.chunk_map.fill_biomes(raw_from, raw_to, |current| {
        let Some(current_biome) = REGISTRY.biomes.by_id(usize::from(current)) else {
            return None;
        };
        if filter.is_some_and(|filter| !filter.matches(current_biome)) {
            return None;
        }
        Some(target_id)
    }) else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::ARGUMENT_POS_UNLOADED,
        )));
    };

    if result.changed == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_FILLBIOME_NO_CHANGES,
        )));
    }

    world.chunk_map.resend_biomes_for_chunks(&result.chunks);

    let message = translations::COMMANDS_FILLBIOME_SUCCESS_COUNT
        .message([
            TextComponent::from(result.changed.to_string()),
            TextComponent::from(region.min_x().to_string()),
            TextComponent::from(region.min_y().to_string()),
            TextComponent::from(region.min_z().to_string()),
            TextComponent::from(region.max_x().to_string()),
            TextComponent::from(region.max_y().to_string()),
            TextComponent::from(region.max_z().to_string()),
        ])
        .component();
    source.send_success(&message, true);

    i32::try_from(result.changed).map_err(|_| {
        CommandSyntaxError::dynamic("Changed biome count exceeds the command result range")
    })
}

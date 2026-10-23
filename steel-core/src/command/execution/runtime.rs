#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "custom runtime variants are reserved for future keyed command integrations"
    )
)]

use std::sync::Arc;

use super::{
    BiomeOrTag, BlockInput, BlockPredicate, ChainModifiers, CommandResultSuspension, CommandSource,
    Coordinates, ExecutionCommandSource, ExecutionControl, GameProfileArgument, IntRange,
    ItemPredicate, PermissionGroupName, ScoreHolderArgument, ScoreHolderWildcard,
    SteelArgumentType, StructureOrTagKey, WorldArgument,
    argument::{
        AngleValue, AttributeValue, ColumnPos, ColumnPosValue, ComponentValue, CoordinateAxes,
        DomainValue, EnchantmentValue, EntityTypeValue, FloatRange, GameModeValue, HexColorValue,
        IdentifierValue, ItemStackValue, MessageValue, MobEffectValue, NbtPathValue, NbtTagValue,
        ObjectiveCriteriaValue, ObjectiveValue, Operation, OperationValue, ParticleValue,
        ScoreboardSlotValue, SlotRange, SlotsValue, SteelArgumentValue, StyleValue,
        SwingAnimationValue, TeamColorValue, TeamValue, TemplateMirrorValue, TemplateRotationValue,
        TimeValue, TimelineValue, UuidValue, WorldClockValue, resolve_message,
    },
    selector::EntitySelector,
};
use crate::command::brigadier::{
    ArgumentSuggestionContext, CommandContext, CommandNodeBuilder, CommandRedirectTarget,
    CommandRuntime, CommandSyntaxError, ContextChain,
};
use crate::command::execution::argument::{AdvancementValue, DamageTypeValue};
use crate::command::incorrectly_typed_argument;
use crate::{
    chunk::heightmap::HeightmapType,
    entity::{EntityAnchor, SharedEntity},
    permission::{PermissionMetadataExpression, PermissionRuleExpression},
    player::Player,
    scoreboard::{DisplaySlot, ScoreHolder, TeamColor},
};
use simdnbt::owned::NbtTag;
use steel_registry::advancement::registry::AdvancementRef;
use steel_registry::attribute::AttributeRef;
use steel_registry::damage_type::DamageTypeRef;
use steel_registry::data_components::vanilla_components::SwingAnimationType;
use steel_registry::mob_effect::MobEffectRef;
use steel_registry::particle_type::ParticleTypeRef;
use steel_registry::{
    enchantment::EnchantmentRef, entity_type::EntityTypeRef, item_stack::ItemStack,
    timeline::TimelineRef, world_clock::WorldClockRef,
};
use steel_utils::{
    DowncastType, Identifier, Rotation, nbt::NbtPath, translations, types::GameType,
};
use steel_worldgen::structure::StructureMirror;
use text_components::TextComponent;
use text_components::format::Format;
use uuid::Uuid;

/// Runtime model interpreted by Steel's tick-owned command scheduler.
pub(crate) struct SteelCommandRuntime;

pub(crate) type SteelCommandContext<S> = CommandContext<S, SteelCommandRuntime>;
pub(crate) type SteelContextChain<S> = ContextChain<S, SteelCommandRuntime>;
pub(crate) type SteelSuggestionContext<'context, S> =
    ArgumentSuggestionContext<'context, S, SteelArgumentValue>;

type StandardExecutor<S> =
    dyn Fn(&SteelCommandContext<S>) -> Result<i32, CommandSyntaxError> + Send + Sync;
type SuspendedExecutor<S> = dyn Fn(&SteelCommandContext<S>) -> Result<Box<dyn CommandResultSuspension>, CommandSyntaxError>
    + Send
    + Sync;
type StandardModifier<S> =
    dyn Fn(&SteelCommandContext<S>) -> Result<Vec<S>, CommandSyntaxError> + Send + Sync;

/// A terminal executor stored in a Steel command graph.
pub(crate) enum SteelExecutor<S>
where
    S: ExecutionCommandSource,
{
    Standard(Box<StandardExecutor<S>>),
    Suspended(Box<SuspendedExecutor<S>>),
    Custom(Arc<dyn CustomCommandExecutor<S>>),
}

/// A redirect modifier stored in a Steel command graph.
pub(crate) enum SteelModifier<S>
where
    S: ExecutionCommandSource,
{
    Standard(Box<StandardModifier<S>>),
    Custom(Arc<dyn CustomModifierExecutor<S>>),
}

/// Special terminal behavior that controls command frames or queues more work.
pub(crate) trait CustomCommandExecutor<S>: Send + Sync
where
    S: ExecutionCommandSource,
{
    fn run(
        &self,
        source: Arc<S>,
        chain: &SteelContextChain<S>,
        modifiers: ChainModifiers,
        control: &mut ExecutionControl<'_, S>,
    );
}

/// Special redirect behavior that controls command frames or queues more work.
pub(crate) trait CustomModifierExecutor<S>: Send + Sync
where
    S: ExecutionCommandSource,
{
    fn apply(
        &self,
        original_source: Arc<S>,
        sources: Vec<Arc<S>>,
        chain: &SteelContextChain<S>,
        modifiers: ChainModifiers,
        control: &mut ExecutionControl<'_, S>,
    );
}

impl<S> CommandRuntime<S> for SteelCommandRuntime
where
    S: ExecutionCommandSource,
{
    type Argument = SteelArgumentType;
    type ArgumentValue = SteelArgumentValue;
    type Executor = SteelExecutor<S>;
    type Modifier = SteelModifier<S>;
}

/// Creates a literal backed by Steel's runtime model.
pub(crate) fn literal<S>(name: impl Into<Box<str>>) -> CommandNodeBuilder<S, SteelCommandRuntime>
where
    S: ExecutionCommandSource,
{
    CommandNodeBuilder::literal(name)
}

/// Creates an argument backed by Steel's runtime model.
pub(crate) fn argument<S>(
    name: impl Into<Box<str>>,
    argument_type: impl Into<SteelArgumentType>,
) -> CommandNodeBuilder<S, SteelCommandRuntime>
where
    S: ExecutionCommandSource,
{
    CommandNodeBuilder::argument(name, argument_type.into())
}

impl<S> CommandNodeBuilder<S, SteelCommandRuntime>
where
    S: ExecutionCommandSource,
{
    /// Attaches an ordinary synchronous executor.
    #[must_use]
    pub(crate) fn executes(
        self,
        executor: impl Fn(&SteelCommandContext<S>) -> Result<i32, CommandSyntaxError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.executes_with_executor(Arc::new(SteelExecutor::Standard(Box::new(executor))))
    }

    /// Attaches an ordinary executor whose command result is produced across ticks.
    #[must_use]
    pub(crate) fn executes_suspended<T>(
        self,
        executor: impl Fn(&SteelCommandContext<S>) -> Result<T, CommandSyntaxError>
        + Send
        + Sync
        + 'static,
    ) -> Self
    where
        T: CommandResultSuspension,
    {
        let executor = move |context: &SteelCommandContext<S>| {
            executor(context)
                .map(|suspension| Box::new(suspension) as Box<dyn CommandResultSuspension>)
        };
        self.executes_with_executor(Arc::new(SteelExecutor::Suspended(Box::new(executor))))
    }

    /// Attaches an internal executor with frame and queue control.
    #[must_use]
    pub(crate) fn executes_custom(self, executor: impl CustomCommandExecutor<S> + 'static) -> Self {
        self.executes_with_executor(Arc::new(SteelExecutor::Custom(Arc::new(executor))))
    }

    /// Redirects parsing and transforms the source once before continuing.
    #[must_use]
    pub(crate) fn redirects_with(
        self,
        target: impl Into<CommandRedirectTarget>,
        modifier: impl Fn(&SteelCommandContext<S>) -> Result<S, CommandSyntaxError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let modifier = SteelModifier::Standard(Box::new(move |context| {
            modifier(context).map(|source| vec![source])
        }));
        self.redirects_with_modifier(target, Arc::new(modifier), false)
    }

    /// Redirects parsing and expands one source into zero or more sources.
    #[must_use]
    pub(crate) fn forks(
        self,
        target: impl Into<CommandRedirectTarget>,
        modifier: impl Fn(&SteelCommandContext<S>) -> Result<Vec<S>, CommandSyntaxError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.redirects_with_modifier(
            target,
            Arc::new(SteelModifier::Standard(Box::new(modifier))),
            true,
        )
    }

    /// Redirects with an internal modifier that controls frames or queued work.
    #[must_use]
    pub(crate) fn redirects_custom(
        self,
        target: impl Into<CommandRedirectTarget>,
        modifier: impl CustomModifierExecutor<S> + 'static,
        forks: bool,
    ) -> Self {
        self.redirects_with_modifier(
            target,
            Arc::new(SteelModifier::Custom(Arc::new(modifier))),
            forks,
        )
    }
}

impl<S> SteelCommandContext<S>
where
    S: ExecutionCommandSource,
{
    fn typed_argument<T: DowncastType>(&self, name: &str) -> Result<&T, CommandSyntaxError> {
        self.argument(name)?
            .downcast_ref::<T>()
            .ok_or_else(|| incorrectly_typed_argument(name))
    }

    /// Returns a parsed Minecraft time argument in ticks.
    pub(crate) fn time(&self, name: &str) -> Result<i32, CommandSyntaxError> {
        self.typed_argument::<TimeValue>(name).map(|value| value.0)
    }

    /// Returns a parsed coordinate expression without resolving it early.
    pub(crate) fn coordinates(&self, name: &str) -> Result<Coordinates, CommandSyntaxError> {
        self.typed_argument::<Coordinates>(name).copied()
    }

    /// Returns a parsed entity position anchor.
    pub(crate) fn entity_anchor(&self, name: &str) -> Result<EntityAnchor, CommandSyntaxError> {
        self.typed_argument::<EntityAnchor>(name).copied()
    }

    pub(crate) fn swizzle(&self, name: &str) -> Result<CoordinateAxes, CommandSyntaxError> {
        self.typed_argument::<CoordinateAxes>(name).copied()
    }

    pub(crate) fn heightmap(&self, name: &str) -> Result<HeightmapType, CommandSyntaxError> {
        self.typed_argument::<HeightmapType>(name).copied()
    }

    pub(crate) fn score_holder_argument(
        &self,
        name: &str,
    ) -> Result<&ScoreHolderArgument, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn objective_name(&self, name: &str) -> Result<&str, CommandSyntaxError> {
        self.typed_argument::<ObjectiveValue>(name)
            .map(|value| value.0.as_ref())
    }

    pub(crate) fn int_range(&self, name: &str) -> Result<IntRange, CommandSyntaxError> {
        self.typed_argument::<IntRange>(name).copied()
    }

    pub(crate) fn biome_or_tag(&self, name: &str) -> Result<&BiomeOrTag, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn structure_or_tag_key(
        &self,
        name: &str,
    ) -> Result<&StructureOrTagKey, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn block_predicate(
        &self,
        name: &str,
    ) -> Result<&BlockPredicate, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn block_input(&self, name: &str) -> Result<&BlockInput, CommandSyntaxError> {
        self.typed_argument(name)
    }

    /// Returns a configured Steel domain name.
    pub(crate) fn domain(&self, name: &str) -> Result<&str, CommandSyntaxError> {
        self.typed_argument::<DomainValue>(name)
            .map(|value| value.0.as_ref())
    }

    pub(crate) fn world_argument(&self, name: &str) -> Result<&WorldArgument, CommandSyntaxError> {
        self.typed_argument(name)
    }

    /// Returns a parsed vanilla game mode.
    pub(crate) fn game_mode(&self, name: &str) -> Result<GameType, CommandSyntaxError> {
        self.typed_argument::<GameModeValue>(name)
            .map(|value| value.0)
    }

    pub(crate) fn entity_type(&self, name: &str) -> Result<EntityTypeRef, CommandSyntaxError> {
        self.typed_argument::<EntityTypeValue>(name)
            .map(|value| value.0)
    }

    pub(crate) fn enchantment(&self, name: &str) -> Result<EnchantmentRef, CommandSyntaxError> {
        self.typed_argument::<EnchantmentValue>(name)
            .map(|value| value.0)
    }

    pub(crate) fn damage_type(&self, name: &str) -> Result<DamageTypeRef, CommandSyntaxError> {
        self.typed_argument::<DamageTypeValue>(name)
            .map(|value| value.0)
    }

    pub(crate) fn item_stack(&self, name: &str) -> Result<&ItemStack, CommandSyntaxError> {
        self.typed_argument::<ItemStackValue>(name)
            .map(|value| &value.0)
    }

    pub(crate) fn item_predicate(&self, name: &str) -> Result<&ItemPredicate, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn text_component(&self, name: &str) -> Result<&TextComponent, CommandSyntaxError> {
        self.typed_argument::<ComponentValue>(name)
            .map(|value| &value.0)
    }

    pub(crate) fn nbt_path(&self, name: &str) -> Result<&NbtPath, CommandSyntaxError> {
        self.typed_argument::<NbtPathValue>(name)
            .map(|value| &value.0)
    }

    pub(crate) fn identifier(&self, name: &str) -> Result<&Identifier, CommandSyntaxError> {
        self.typed_argument::<IdentifierValue>(name)
            .map(|value| &value.0)
    }

    pub(crate) fn world_clock(&self, name: &str) -> Result<WorldClockRef, CommandSyntaxError> {
        self.typed_argument::<WorldClockValue>(name)
            .map(|value| value.0)
    }

    pub(crate) fn timeline(&self, name: &str) -> Result<TimelineRef, CommandSyntaxError> {
        self.typed_argument::<TimelineValue>(name)
            .map(|value| value.0)
    }

    pub(crate) fn advancement(&self, name: &str) -> Result<AdvancementRef, CommandSyntaxError> {
        self.typed_argument::<AdvancementValue>(name)
            .map(|value| value.0)
    }

    pub(crate) fn entity_selector(
        &self,
        name: &str,
    ) -> Result<&EntitySelector, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn game_profile_argument(
        &self,
        name: &str,
    ) -> Result<&GameProfileArgument, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn permission_rule_expression(
        &self,
        name: &str,
    ) -> Result<&PermissionRuleExpression, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn permission_metadata_expression(
        &self,
        name: &str,
    ) -> Result<&PermissionMetadataExpression, CommandSyntaxError> {
        self.typed_argument(name)
    }

    pub(crate) fn permission_group(
        &self,
        name: &str,
    ) -> Result<&PermissionGroupName, CommandSyntaxError> {
        self.typed_argument(name)
    }

    /// Returns a parsed packed opaque ARGB color.
    pub(crate) fn hex_color(&self, name: &str) -> Result<i32, CommandSyntaxError> {
        self.typed_argument::<HexColorValue>(name)
            .map(|value| value.0)
    }

    /// Returns a parsed mob effect.
    pub(crate) fn mob_effect(&self, name: &str) -> Result<MobEffectRef, CommandSyntaxError> {
        self.typed_argument::<MobEffectValue>(name)
            .map(|value| value.0)
    }

    /// Returns the registered particle type selected by a particle argument.
    pub(crate) fn particle(&self, name: &str) -> Result<ParticleTypeRef, CommandSyntaxError> {
        self.typed_argument::<ParticleValue>(name)
            .map(|value| value.0)
    }

    /// Returns a parsed attribute.
    pub(crate) fn attribute(&self, name: &str) -> Result<AttributeRef, CommandSyntaxError> {
        self.typed_argument::<AttributeValue>(name)
            .map(|value| value.0)
    }

    /// Returns a parsed scoreboard operation.
    pub(crate) fn operation(&self, name: &str) -> Result<Operation, CommandSyntaxError> {
        self.typed_argument::<OperationValue>(name)
            .map(|value| value.0)
    }

    /// Returns a validated vanilla objective criteria name.
    pub(crate) fn objective_criteria(&self, name: &str) -> Result<&str, CommandSyntaxError> {
        self.typed_argument::<ObjectiveCriteriaValue>(name)
            .map(|value| value.0.as_ref())
    }

    /// Returns a parsed scoreboard display slot.
    pub(crate) fn scoreboard_slot(&self, name: &str) -> Result<DisplaySlot, CommandSyntaxError> {
        self.typed_argument::<ScoreboardSlotValue>(name)
            .map(|value| value.0)
    }

    /// Returns the scoreboard team name, mirroring vanilla `getTeam`'s existence check.
    pub(crate) fn team_argument(&self, name: &str) -> Result<&str, CommandSyntaxError> {
        let team = self.typed_argument::<TeamValue>(name)?.0.as_ref();
        if self
            .source()
            .selector_team_names()
            .iter()
            .any(|existing| existing == team)
        {
            return Ok(team);
        }
        Err(CommandSyntaxError::dynamic(
            translations::TEAM_NOT_FOUND
                .message([team.to_owned()])
                .component(),
        ))
    }

    /// Returns a parsed team color.
    pub(crate) fn team_color(&self, name: &str) -> Result<TeamColor, CommandSyntaxError> {
        self.typed_argument::<TeamColorValue>(name)
            .map(|value| value.0)
    }

    /// Returns a parsed UUID.
    pub(crate) fn uuid(&self, name: &str) -> Result<Uuid, CommandSyntaxError> {
        self.typed_argument::<UuidValue>(name).map(|value| value.0)
    }

    /// Returns a parsed inclusive double range.
    pub(crate) fn double_range(&self, name: &str) -> Result<FloatRange, CommandSyntaxError> {
        self.typed_argument::<FloatRange>(name).copied()
    }

    /// Returns a parsed SNBT value.
    pub(crate) fn nbt_tag(&self, name: &str) -> Result<&NbtTag, CommandSyntaxError> {
        self.typed_argument::<NbtTagValue>(name)
            .map(|value| &value.0)
    }

    /// Returns a parsed swing animation.
    pub(crate) fn swing_animation(
        &self,
        name: &str,
    ) -> Result<SwingAnimationType, CommandSyntaxError> {
        self.typed_argument::<SwingAnimationValue>(name)
            .map(|value| value.0)
    }

    /// Returns a parsed generic resource location.
    pub(crate) fn identifier_argument(
        &self,
        name: &str,
    ) -> Result<&Identifier, CommandSyntaxError> {
        self.typed_argument::<IdentifierValue>(name)
            .map(|value| &value.0)
    }

    /// Returns a parsed vanilla slot range.
    pub(crate) fn slots(&self, name: &str) -> Result<&SlotRange, CommandSyntaxError> {
        self.typed_argument::<SlotsValue>(name)
            .map(|value| &value.0)
    }

    /// Returns a parsed structure mirror.
    pub(crate) fn template_mirror(
        &self,
        name: &str,
    ) -> Result<StructureMirror, CommandSyntaxError> {
        self.typed_argument::<TemplateMirrorValue>(name)
            .map(|value| value.0)
    }

    /// Returns a parsed structure rotation.
    pub(crate) fn template_rotation(&self, name: &str) -> Result<Rotation, CommandSyntaxError> {
        self.typed_argument::<TemplateRotationValue>(name)
            .map(|value| value.0)
    }

    /// Returns a parsed chat style.
    pub(crate) fn style(&self, name: &str) -> Result<&Format, CommandSyntaxError> {
        self.typed_argument::<StyleValue>(name)
            .map(|value| &value.0)
    }
}

impl<S> SteelSuggestionContext<'_, S> {
    fn typed_argument<T: DowncastType>(&self, name: &str) -> Result<&T, CommandSyntaxError> {
        self.argument(name)?
            .downcast_ref::<T>()
            .ok_or_else(|| incorrectly_typed_argument(name))
    }

    /// Returns a parsed advancement argument.
    pub(crate) fn advancement(&self, name: &str) -> Result<AdvancementRef, CommandSyntaxError> {
        self.typed_argument::<AdvancementValue>(name)
            .map(|value| value.0)
    }
}

impl SteelCommandContext<CommandSource> {
    /// Returns the yaw selected by a vanilla angle argument, wrapped like vanilla.
    pub(crate) fn angle(&self, name: &str) -> Result<f32, CommandSyntaxError> {
        let angle = self.typed_argument::<AngleValue>(name)?.0;
        let (yaw, _pitch) = self.source().rotation();
        Ok(angle.resolve(yaw))
    }

    /// Returns the greedy message with its entity selectors resolved to display names.
    pub(crate) fn message(&self, name: &str) -> Result<TextComponent, CommandSyntaxError> {
        let message = &self.typed_argument::<MessageValue>(name)?.0;
        resolve_message(message, self.source())
    }

    /// Returns the validated column position of a two-dimensional coordinate argument.
    pub(crate) fn column_pos(&self, name: &str) -> Result<ColumnPos, CommandSyntaxError> {
        let coordinates = self.typed_argument::<ColumnPosValue>(name)?.0;
        let block = coordinates.block_pos(self.source());
        Ok(ColumnPos {
            x: block.x(),
            z: block.z(),
        })
    }

    pub(crate) fn score_holders(
        &self,
        name: &str,
        wildcard: ScoreHolderWildcard,
    ) -> Result<Vec<ScoreHolder>, CommandSyntaxError> {
        let holders = self
            .score_holder_argument(name)?
            .resolve(self.source(), wildcard)?;
        if holders.is_empty() {
            Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::ARGUMENT_SCORE_HOLDER_EMPTY,
            )))
        } else {
            Ok(holders)
        }
    }

    pub(crate) fn score_holder(&self, name: &str) -> Result<ScoreHolder, CommandSyntaxError> {
        let mut holders = self.score_holders(name, ScoreHolderWildcard::Empty)?;
        Ok(holders.remove(0))
    }

    pub(crate) fn optional_entities(
        &self,
        name: &str,
    ) -> Result<Vec<SharedEntity>, CommandSyntaxError> {
        self.entity_selector(name)?.find_entities(self.source())
    }

    pub(crate) fn entities(&self, name: &str) -> Result<Vec<SharedEntity>, CommandSyntaxError> {
        let entities = self.optional_entities(name)?;
        if entities.is_empty() {
            Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::ARGUMENT_ENTITY_NOTFOUND_ENTITY,
            )))
        } else {
            Ok(entities)
        }
    }

    pub(crate) fn entity(&self, name: &str) -> Result<SharedEntity, CommandSyntaxError> {
        let mut entities = self.entities(name)?;
        if entities.len() != 1 {
            return Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::ARGUMENT_ENTITY_TOOMANY,
            )));
        }
        Ok(entities.remove(0))
    }

    pub(crate) fn optional_players(
        &self,
        name: &str,
    ) -> Result<Vec<Arc<Player>>, CommandSyntaxError> {
        self.entity_selector(name)?.find_players(self.source())
    }

    pub(crate) fn players(&self, name: &str) -> Result<Vec<Arc<Player>>, CommandSyntaxError> {
        let players = self.optional_players(name)?;
        if players.is_empty() {
            Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::ARGUMENT_ENTITY_NOTFOUND_PLAYER,
            )))
        } else {
            Ok(players)
        }
    }

    pub(crate) fn player(&self, name: &str) -> Result<Arc<Player>, CommandSyntaxError> {
        let mut players = self.players(name)?;
        if players.len() != 1 {
            return Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::ARGUMENT_PLAYER_TOOMANY,
            )));
        }
        Ok(players.remove(0))
    }
}

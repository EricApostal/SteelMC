use std::{fmt, sync::Arc};

use super::{
    BiomeOrTag, BlockInput, BlockPredicate, CommandArgumentSource, CommandSource, Coordinates,
    IntRange, ItemPredicate, ScoreHolderArgument, StructureOrTagKey, WorldArgument,
    biome::{parse_biome_or_tag, suggest_biomes},
    block::{parse_block_input, parse_block_predicate, suggest_block_inputs, suggest_blocks},
    coordinates::{
        WorldCoordinate, WorldCoordinates, parse_block_pos, parse_rotation, parse_vec2, parse_vec3,
        suggest_coordinates, suggest_vec2,
    },
    item::{parse_item_stack, suggest_item_stack},
    item_predicate::{parse_item_predicate, suggest_item_predicate},
    nbt::parse_nbt_path,
    permission::{PermissionGroupParser, PermissionMetadataParser, PermissionRuleParser},
    profile::{GameProfileParser, GameProfileSuggestionMode},
    score::{parse_int_range, parse_score_holder, suggest_score_holders},
    selector::{EntitySelector, parse_entity_selector, suggest_entity_selector},
    structure::{parse_structure_or_tag_key, suggest_structures},
    suggestions::{matches_substring_ignore_case, suggest_resources},
    text::validate_component_syntax,
    world::{parse_world_argument, suggest_worlds},
};
use crate::chunk::heightmap::HeightmapType;
use crate::command::brigadier::{
    ArgumentSuggestionContext, ArgumentType, CommandArgumentParser, CommandSyntaxError,
    CommandSyntaxErrorKind, ContainsPrimitiveArgumentValue, PrimitiveArgumentValue, ReaderCursor,
    StringReader, SuggestionsBuilder,
};
use crate::command::incorrectly_typed_argument;
use crate::command::protocol::protocol_argument_type;
use crate::entity::{ENTITIES, EntityAnchor};
use crate::scoreboard::{DisplaySlot, ObjectiveCriteria, TeamColor};
use glam::DVec3;
use simdnbt::owned::NbtTag;
use steel_math::wrap_degrees;
use steel_protocol::packets::game::{
    ArgumentStringTypeBehavior, ArgumentType as ProtocolArgumentType,
    SuggestionType as ProtocolSuggestionType,
};
use steel_registry::advancement::registry::AdvancementRef;
use steel_registry::attribute::AttributeRef;
use steel_registry::damage_type::DamageTypeRef;
use steel_registry::data_components::vanilla_components::SwingAnimationType;
use steel_registry::mob_effect::MobEffectRef;
use steel_registry::particle_type::ParticleTypeRef;
use steel_registry::{
    DAMAGE_TYPE_REGISTRY, ENCHANTMENT_REGISTRY, ENTITY_TYPE_REGISTRY, MOB_EFFECT_REGISTRY,
    REGISTRY, RegistryExt as _, TIMELINE_REGISTRY, WORLD_CLOCK_REGISTRY,
    enchantment::EnchantmentRef, entity_type::EntityTypeRef, item_stack::ItemStack,
    timeline::TimelineRef, world_clock::WorldClockRef,
};
use steel_utils::translations::ADVANCEMENT_ADVANCEMENT_NOT_FOUND;
use steel_utils::{
    Downcast as _, DowncastType, DowncastTypeKey, ErasedType, Identifier, Rotation,
    nbt::{NbtPath, parse_snbt_argument},
    translations,
    types::GameType,
};
use steel_worldgen::structure::StructureMirror;
use text_components::TextComponent;
use text_components::format::Format;
use uuid::Uuid;

/// Axes selected by vanilla's coordinate swizzle argument.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CoordinateAxes(u8);

impl CoordinateAxes {
    const X: u8 = 1;
    const Y: u8 = 2;
    const Z: u8 = 4;

    pub(crate) const fn x(self) -> bool {
        self.0 & Self::X != 0
    }

    pub(crate) const fn y(self) -> bool {
        self.0 & Self::Y != 0
    }

    pub(crate) const fn z(self) -> bool {
        self.0 & Self::Z != 0
    }

    pub(crate) const fn align(self, mut position: DVec3) -> DVec3 {
        if self.x() {
            position.x = position.x.floor();
        }
        if self.y() {
            position.y = position.y.floor();
        }
        if self.z() {
            position.z = position.z.floor();
        }
        position
    }
}

/// Typed parser contract erased by [`SteelArgumentType`].
pub(crate) trait SteelArgumentParser:
    DowncastType + fmt::Debug + PartialEq + Send + Sync + 'static
{
    /// Concrete value produced by this parser.
    type Value: DowncastType + fmt::Debug + Send + Sync + 'static;

    /// Parses one value from the command reader.
    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError>;

    /// Adds context-aware completion suggestions.
    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        _builder: &mut SuggestionsBuilder<'_>,
    ) {
    }

    /// Returns the vanilla command-tree parser representation.
    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>);
}

trait ErasedSteelArgumentParser: ErasedType + fmt::Debug + Send + Sync {
    fn parse_erased(
        &self,
        reader: &mut StringReader<'_>,
        source: &dyn CommandArgumentSource,
    ) -> Result<SteelArgumentValue, CommandSyntaxError>;

    fn list_suggestions_erased(
        &self,
        context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    );

    fn protocol_argument_erased(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>);

    fn equals_erased(&self, other: &dyn ErasedSteelArgumentParser) -> bool;
}

impl<P> ErasedSteelArgumentParser for P
where
    P: SteelArgumentParser,
{
    fn parse_erased(
        &self,
        reader: &mut StringReader<'_>,
        source: &dyn CommandArgumentSource,
    ) -> Result<SteelArgumentValue, CommandSyntaxError> {
        self.parse(reader, source).map(SteelArgumentValue::new)
    }

    fn list_suggestions_erased(
        &self,
        context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        self.list_suggestions(context, builder);
    }

    fn protocol_argument_erased(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        self.protocol_argument()
    }

    fn equals_erased(&self, other: &dyn ErasedSteelArgumentParser) -> bool {
        other.downcast_ref::<P>() == Some(self)
    }
}

/// An extensible, keyed parser stored by Steel's command runtime.
#[derive(Clone)]
pub(crate) struct SteelArgumentType(Arc<dyn ErasedSteelArgumentParser>);

impl SteelArgumentType {
    /// Erases a concrete parser while retaining its deterministic type key.
    pub(crate) fn new(parser: impl SteelArgumentParser) -> Self {
        Self(Arc::new(parser))
    }

    pub(crate) fn time(minimum: i32) -> Self {
        Self::new(TimeParser { minimum })
    }

    pub(crate) fn block_pos() -> Self {
        Self::new(BlockPosParser)
    }

    pub(crate) fn vec3(center_integers: bool) -> Self {
        Self::new(Vec3Parser { center_integers })
    }

    pub(crate) fn vec2(center_integers: bool) -> Self {
        Self::new(Vec2Parser { center_integers })
    }

    pub(crate) fn rotation() -> Self {
        Self::new(RotationParser)
    }

    pub(crate) fn swizzle() -> Self {
        Self::new(SwizzleParser)
    }

    pub(crate) fn heightmap() -> Self {
        Self::new(HeightmapParser)
    }

    pub(crate) fn entity_anchor() -> Self {
        Self::new(EntityAnchorParser)
    }

    pub(crate) fn entity_tag() -> Self {
        Self::new(EntityTagParser)
    }

    pub(crate) fn entity() -> Self {
        Self::new(EntityParser {
            single: true,
            players_only: false,
        })
    }

    pub(crate) fn entities() -> Self {
        Self::new(EntityParser {
            single: false,
            players_only: false,
        })
    }

    pub(crate) fn player() -> Self {
        Self::new(EntityParser {
            single: true,
            players_only: true,
        })
    }

    pub(crate) fn players() -> Self {
        Self::new(EntityParser {
            single: false,
            players_only: true,
        })
    }

    pub(crate) fn score_holder() -> Self {
        Self::new(ScoreHolderParser { multiple: false })
    }

    pub(crate) fn non_operator_profile() -> Self {
        Self::new(GameProfileParser::new(
            GameProfileSuggestionMode::NonOperators,
        ))
    }

    pub(crate) fn game_profile() -> Self {
        Self::new(GameProfileParser::new(GameProfileSuggestionMode::All))
    }

    pub(crate) fn operator_profile() -> Self {
        Self::new(GameProfileParser::new(GameProfileSuggestionMode::Operators))
    }

    pub(crate) fn permission_rule() -> Self {
        Self::new(PermissionRuleParser::all())
    }

    pub(crate) fn user_permission_rule() -> Self {
        Self::new(PermissionRuleParser::user_owned())
    }

    pub(crate) fn group_permission_rule() -> Self {
        Self::new(PermissionRuleParser::group_owned())
    }

    pub(crate) fn permission_metadata() -> Self {
        Self::new(PermissionMetadataParser::all())
    }

    pub(crate) fn user_permission_metadata() -> Self {
        Self::new(PermissionMetadataParser::user_owned())
    }

    pub(crate) fn group_permission_metadata() -> Self {
        Self::new(PermissionMetadataParser::group_owned())
    }

    pub(crate) fn permission_group(require_existing: bool) -> Self {
        Self::new(PermissionGroupParser { require_existing })
    }

    pub(crate) fn score_holders() -> Self {
        Self::new(ScoreHolderParser { multiple: true })
    }

    pub(crate) fn objective() -> Self {
        Self::new(ObjectiveParser)
    }

    pub(crate) fn int_range() -> Self {
        Self::new(IntRangeParser)
    }

    pub(crate) fn biome_or_tag() -> Self {
        Self::new(BiomeOrTagParser)
    }

    pub(crate) fn structure_or_tag_key() -> Self {
        Self::new(StructureOrTagKeyParser)
    }

    pub(crate) fn block_predicate() -> Self {
        Self::new(BlockPredicateParser)
    }

    pub(crate) fn block_state() -> Self {
        Self::new(BlockStateParser)
    }

    pub(crate) fn game_mode() -> Self {
        Self::new(GameModeParser)
    }

    pub(crate) fn domain() -> Self {
        Self::new(DomainParser)
    }

    pub(crate) fn world() -> Self {
        Self::new(WorldParser)
    }

    pub(crate) fn summonable_entity() -> Self {
        Self::new(SummonableEntityParser)
    }

    pub(crate) fn enchantment() -> Self {
        Self::new(EnchantmentParser)
    }

    pub(crate) fn damage_type() -> Self {
        Self::new(DamageTypeParser)
    }

    pub(crate) fn item_stack() -> Self {
        Self::new(ItemStackParser)
    }

    pub(crate) fn item_predicate() -> Self {
        Self::new(ItemPredicateParser)
    }

    pub(crate) fn component() -> Self {
        Self::new(ComponentParser)
    }

    pub(crate) fn nbt_path() -> Self {
        Self::new(NbtPathParser)
    }

    pub(crate) fn storage_key() -> Self {
        Self::new(StorageKeyParser)
    }

    pub(crate) fn sound() -> Self {
        Self::new(SoundParser)
    }

    pub(crate) fn advancement() -> Self {
        Self::new(AdvancementParser)
    }

    pub(crate) fn world_clock() -> Self {
        Self::new(WorldClockParser)
    }

    pub(crate) fn timeline(clock_argument: Option<&'static str>) -> Self {
        Self::new(TimelineParser { clock_argument })
    }

    pub(crate) fn time_marker(clock_argument: Option<&'static str>) -> Self {
        Self::new(TimeMarkerParser { clock_argument })
    }

    /// Vanilla `AngleArgument`: one source-relative or absolute yaw.
    pub(crate) fn angle() -> Self {
        Self::new(AngleParser)
    }

    /// Vanilla `MessageArgument`: greedy text with resolvable entity selectors.
    pub(crate) fn message() -> Self {
        Self::new(MessageParser)
    }

    /// Vanilla `HexColorArgument`: a packed opaque ARGB color.
    pub(crate) fn hex_color() -> Self {
        Self::new(HexColorParser)
    }

    /// Vanilla `ResourceArgument<MobEffect>`.
    pub(crate) fn mob_effect() -> Self {
        Self::new(MobEffectParser)
    }

    /// Vanilla `ResourceArgument<Attribute>`.
    pub(crate) fn attribute() -> Self {
        Self::new(AttributeParser)
    }

    /// Vanilla `ParticleArgument` for the bare-key form.
    pub(crate) fn particle() -> Self {
        Self::new(ParticleParser)
    }

    /// Vanilla `OperationArgument`: a scoreboard operation symbol.
    pub(crate) fn operation() -> Self {
        Self::new(OperationParser)
    }

    /// Vanilla `ObjectiveCriteriaArgument`.
    pub(crate) fn objective_criteria() -> Self {
        Self::new(ObjectiveCriteriaParser)
    }

    /// Vanilla `ScoreboardSlotArgument`.
    pub(crate) fn scoreboard_slot() -> Self {
        Self::new(ScoreboardSlotParser)
    }

    /// Vanilla `TeamArgument`: a scoreboard team name retained for execution.
    pub(crate) fn team() -> Self {
        Self::new(TeamParser)
    }

    /// Vanilla `TeamColorArgument`.
    pub(crate) fn team_color() -> Self {
        Self::new(TeamColorParser)
    }

    /// Vanilla `UuidArgument`.
    pub(crate) fn uuid() -> Self {
        Self::new(UuidParser)
    }

    /// Vanilla `ColumnPosArgument`: a two-dimensional block column.
    pub(crate) fn column_pos() -> Self {
        Self::new(ColumnPosParser)
    }

    /// Vanilla `RangeArgument.Floats`: a double range.
    pub(crate) fn double_range() -> Self {
        Self::new(FloatRangeParser)
    }

    /// Vanilla `NbtTagArgument`: one SNBT value of any type.
    pub(crate) fn nbt_tag() -> Self {
        Self::new(NbtTagParser)
    }

    /// Vanilla `SwingAnimationArgument`.
    pub(crate) fn swing_animation() -> Self {
        Self::new(SwingAnimationParser)
    }

    /// Vanilla `IdentifierArgument`: a generic, unvalidated resource location.
    pub(crate) fn identifier_argument() -> Self {
        Self::new(IdentifierParser)
    }

    /// Vanilla `SlotsArgument`: a named range of vanilla container slots.
    pub(crate) fn slots() -> Self {
        Self::new(SlotsParser)
    }

    /// Vanilla `TemplateMirrorArgument`.
    pub(crate) fn template_mirror() -> Self {
        Self::new(TemplateMirrorParser)
    }

    /// Vanilla `TemplateRotationArgument`.
    pub(crate) fn template_rotation() -> Self {
        Self::new(TemplateRotationParser)
    }

    /// Vanilla `StyleArgument`: a chat style parsed from SNBT.
    pub(crate) fn style() -> Self {
        Self::new(StyleParser)
    }

    pub(crate) fn protocol_argument(
        &self,
    ) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        self.0.protocol_argument_erased()
    }

    #[cfg(test)]
    pub(crate) fn parser_type_key(&self) -> DowncastTypeKey {
        self.0.downcast_type_key()
    }
}

impl fmt::Debug for SteelArgumentType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SteelArgumentType")
            .field("type_key", &self.0.downcast_type_key())
            .field("parser", &self.0)
            .finish()
    }
}

impl PartialEq for SteelArgumentType {
    fn eq(&self, other: &Self) -> bool {
        self.0.equals_erased(other.0.as_ref())
    }
}

impl From<ArgumentType> for SteelArgumentType {
    fn from(argument: ArgumentType) -> Self {
        Self::new(PrimitiveParser(argument))
    }
}

trait ErasedSteelArgumentValue: ErasedType + fmt::Debug + Send + Sync {}

impl<T> ErasedSteelArgumentValue for T where T: DowncastType + fmt::Debug + Send + Sync {}

/// A keyed parsed value retained by Steel's command runtime.
#[derive(Clone)]
pub(crate) struct SteelArgumentValue(Arc<dyn ErasedSteelArgumentValue>);

impl SteelArgumentValue {
    pub(crate) fn new(value: impl DowncastType + fmt::Debug + Send + Sync) -> Self {
        Self(Arc::new(value))
    }

    pub(crate) fn downcast_ref<T: DowncastType>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }

    #[cfg(test)]
    pub(crate) fn type_key(&self) -> DowncastTypeKey {
        self.0.downcast_type_key()
    }
}

impl fmt::Debug for SteelArgumentValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SteelArgumentValue")
            .field("type_key", &self.0.downcast_type_key())
            .field("value", &self.0)
            .finish()
    }
}

impl ContainsPrimitiveArgumentValue for SteelArgumentValue {
    fn primitive_value(&self, name: &str) -> Result<&PrimitiveArgumentValue, CommandSyntaxError> {
        self.downcast_ref::<PrimitiveArgumentValue>()
            .ok_or_else(|| incorrectly_typed_argument(name))
    }
}

/// Suggestion context exposed to erased Steel argument parsers.
pub(crate) trait SteelArgumentSuggestionContext {
    fn source(&self) -> &dyn CommandArgumentSource;

    fn argument(&self, name: &str) -> Result<&SteelArgumentValue, CommandSyntaxError>;
}

impl<S> SteelArgumentSuggestionContext for ArgumentSuggestionContext<'_, S, SteelArgumentValue>
where
    S: CommandArgumentSource,
{
    fn source(&self) -> &dyn CommandArgumentSource {
        ArgumentSuggestionContext::source(self)
    }

    fn argument(&self, name: &str) -> Result<&SteelArgumentValue, CommandSyntaxError> {
        ArgumentSuggestionContext::argument(self, name)
    }
}

impl<S> CommandArgumentParser<S> for SteelArgumentType
where
    S: CommandArgumentSource,
{
    type Value = SteelArgumentValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        source: &S,
    ) -> Result<Self::Value, CommandSyntaxError> {
        self.0.parse_erased(reader, source)
    }

    fn list_suggestions(
        &self,
        context: &ArgumentSuggestionContext<'_, S, Self::Value>,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        self.0.list_suggestions_erased(context, builder);
    }
}

macro_rules! impl_downcast_type {
    ($type:ty, $key:literal) => {
        // SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
        unsafe impl DowncastType for $type {
            const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new($key);
        }
    };
}

impl_downcast_type!(PrimitiveArgumentValue, "steel:command/value/primitive");
impl_downcast_type!(Coordinates, "steel:command/value/coordinates");
impl_downcast_type!(EntityAnchor, "steel:command/value/entity_anchor");
impl_downcast_type!(CoordinateAxes, "steel:command/value/swizzle");
impl_downcast_type!(HeightmapType, "steel:command/value/heightmap");
impl_downcast_type!(EntitySelector, "steel:command/value/entity_selector");
impl_downcast_type!(ScoreHolderArgument, "steel:command/value/score_holder");
impl_downcast_type!(IntRange, "steel:command/value/int_range");
impl_downcast_type!(FloatRange, "steel:command/value/float_range");
impl_downcast_type!(BiomeOrTag, "steel:command/value/biome_or_tag");
impl_downcast_type!(
    StructureOrTagKey,
    "steel:command/value/structure_or_tag_key"
);
impl_downcast_type!(BlockPredicate, "steel:command/value/block_predicate");
impl_downcast_type!(BlockInput, "steel:command/value/block_input");
impl_downcast_type!(WorldArgument, "steel:command/value/world");
impl_downcast_type!(ItemPredicate, "steel:command/value/item_predicate");

macro_rules! argument_value_wrapper {
    ($name:ident($value:ty), $key:literal) => {
        #[derive(Debug)]
        pub(super) struct $name(pub(super) $value);

        impl_downcast_type!($name, $key);
    };
}

argument_value_wrapper!(TimeValue(i32), "steel:command/value/time");
argument_value_wrapper!(ObjectiveValue(Box<str>), "steel:command/value/objective");
argument_value_wrapper!(GameModeValue(GameType), "steel:command/value/game_mode");
argument_value_wrapper!(DomainValue(Box<str>), "steel:command/value/domain");
argument_value_wrapper!(
    EntityTypeValue(EntityTypeRef),
    "steel:command/value/entity_type"
);
argument_value_wrapper!(
    EnchantmentValue(EnchantmentRef),
    "steel:command/value/enchantment"
);
argument_value_wrapper!(
    AdvancementValue(AdvancementRef),
    "steel:command/value/advancement"
);
argument_value_wrapper!(
    DamageTypeValue(DamageTypeRef),
    "steel:command/value/damage_type"
);
argument_value_wrapper!(ItemStackValue(ItemStack), "steel:command/value/item_stack");
argument_value_wrapper!(
    ComponentValue(TextComponent),
    "steel:command/value/component"
);
argument_value_wrapper!(NbtPathValue(NbtPath), "steel:command/value/nbt_path");
argument_value_wrapper!(AngleValue(Angle), "steel:command/value/angle");
argument_value_wrapper!(MessageValue(Message), "steel:command/value/message");
argument_value_wrapper!(HexColorValue(i32), "steel:command/value/hex_color");
argument_value_wrapper!(
    MobEffectValue(MobEffectRef),
    "steel:command/value/mob_effect"
);
argument_value_wrapper!(
    AttributeValue(AttributeRef),
    "steel:command/value/attribute"
);
argument_value_wrapper!(OperationValue(Operation), "steel:command/value/operation");
argument_value_wrapper!(
    ParticleValue(ParticleTypeRef),
    "steel:command/value/particle"
);
argument_value_wrapper!(
    ObjectiveCriteriaValue(Box<str>),
    "steel:command/value/objective_criteria"
);
argument_value_wrapper!(
    ScoreboardSlotValue(DisplaySlot),
    "steel:command/value/scoreboard_slot"
);
argument_value_wrapper!(TeamValue(Box<str>), "steel:command/value/team");
argument_value_wrapper!(TeamColorValue(TeamColor), "steel:command/value/team_color");
argument_value_wrapper!(UuidValue(Uuid), "steel:command/value/uuid");
argument_value_wrapper!(
    ColumnPosValue(Coordinates),
    "steel:command/value/column_pos"
);
argument_value_wrapper!(NbtTagValue(NbtTag), "steel:command/value/nbt_tag");
argument_value_wrapper!(
    SwingAnimationValue(SwingAnimationType),
    "steel:command/value/swing_animation"
);

/// A parsed vanilla `Style`; `text_components::format::Format` is Steel's style type.
pub(super) struct StyleValue(pub(super) Format);

impl fmt::Debug for StyleValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `text_components::format::Format` has no `Debug`, so report emptiness only.
        formatter
            .debug_struct("StyleValue")
            .field("is_empty", &self.0.is_none())
            .finish()
    }
}

impl_downcast_type!(StyleValue, "steel:command/value/style");
argument_value_wrapper!(SlotsValue(SlotRange), "steel:command/value/slots");
argument_value_wrapper!(
    TemplateMirrorValue(StructureMirror),
    "steel:command/value/template_mirror"
);
argument_value_wrapper!(
    TemplateRotationValue(Rotation),
    "steel:command/value/template_rotation"
);
argument_value_wrapper!(
    IdentifierValue(Identifier),
    "steel:command/value/identifier"
);
argument_value_wrapper!(
    WorldClockValue(WorldClockRef),
    "steel:command/value/world_clock"
);
argument_value_wrapper!(TimelineValue(TimelineRef), "steel:command/value/timeline");

/// Vanilla `AngleArgument.SingleAngle`: a yaw that may be relative to the source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Angle {
    value: f32,
    relative: bool,
}

impl Angle {
    pub(crate) const fn new(value: f32, relative: bool) -> Self {
        Self { value, relative }
    }

    /// Resolves this angle against `source_yaw`, wrapping like vanilla `Mth.wrapDegrees`.
    #[must_use]
    pub(crate) fn resolve(self, source_yaw: f32) -> f32 {
        wrap_degrees(if self.relative {
            self.value + source_yaw
        } else {
            self.value
        })
    }
}

/// Vanilla `OperationArgument.Operation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    /// `=`
    Assign,
    /// `+=`
    Add,
    /// `-=`
    Subtract,
    /// `*=`
    Multiply,
    /// `/=`
    Divide,
    /// `%=`
    Modulo,
    /// `><`
    Swap,
    /// `<`
    Min,
    /// `>`
    Max,
}

impl Operation {
    /// Every vanilla operation symbol, in `ArgumentTypeInfos`-visible order.
    pub(crate) const ALL: [Self; 9] = [
        Self::Assign,
        Self::Add,
        Self::Subtract,
        Self::Multiply,
        Self::Divide,
        Self::Modulo,
        Self::Swap,
        Self::Min,
        Self::Max,
    ];

    /// The vanilla symbol accepted by `OperationArgument`.
    #[must_use]
    pub(crate) const fn symbol(self) -> &'static str {
        match self {
            Self::Assign => "=",
            Self::Add => "+=",
            Self::Subtract => "-=",
            Self::Multiply => "*=",
            Self::Divide => "/=",
            Self::Modulo => "%=",
            Self::Swap => "><",
            Self::Min => "<",
            Self::Max => ">",
        }
    }

    fn from_symbol(symbol: &str) -> Option<Self> {
        match symbol {
            "=" => Some(Self::Assign),
            "+=" => Some(Self::Add),
            "-=" => Some(Self::Subtract),
            "*=" => Some(Self::Multiply),
            "/=" => Some(Self::Divide),
            "%=" => Some(Self::Modulo),
            "><" => Some(Self::Swap),
            "<" => Some(Self::Min),
            ">" => Some(Self::Max),
            _ => None,
        }
    }

    /// Returns whether this operation exchanges both score values.
    #[must_use]
    pub(crate) const fn is_swap(self) -> bool {
        matches!(self, Self::Swap)
    }

    /// Applies this operation to a left and right value, mirroring vanilla
    /// `OperationArgument.SimpleOperation`.
    pub(crate) fn apply_values(self, left: i32, right: i32) -> Result<i32, CommandSyntaxError> {
        let zero_division = || {
            CommandSyntaxError::dynamic(TextComponent::from(
                &translations::ARGUMENTS_OPERATION_DIV0,
            ))
        };
        match self {
            Self::Assign => Ok(right),
            Self::Add => Ok(left.wrapping_add(right)),
            Self::Subtract => Ok(left.wrapping_sub(right)),
            Self::Multiply => Ok(left.wrapping_mul(right)),
            Self::Divide => {
                if right == 0 {
                    Err(zero_division())
                } else {
                    // Vanilla `Mth.floorDiv`: floor division, not truncation.
                    Ok(left.div_euclid(right))
                }
            }
            Self::Modulo => {
                if right == 0 {
                    Err(zero_division())
                } else {
                    // Vanilla `Mth.positiveModulo`: always non-negative.
                    Ok(left.rem_euclid(right))
                }
            }
            Self::Min => Ok(left.min(right)),
            Self::Max => Ok(left.max(right)),
            Self::Swap => Ok(right),
        }
    }
}

/// Vanilla `server.level.ColumnPos`: the x/z pair of a block column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ColumnPos {
    /// Column x coordinate.
    pub(crate) x: i32,
    /// Column z coordinate.
    pub(crate) z: i32,
}

/// Vanilla `RangeArgument.Floats` (`MinMaxBounds.Doubles`) inclusive double bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FloatRange {
    minimum: Option<f64>,
    maximum: Option<f64>,
}

impl FloatRange {
    const fn new(minimum: Option<f64>, maximum: Option<f64>) -> Self {
        Self { minimum, maximum }
    }

    /// Returns the inclusive minimum bound.
    #[must_use]
    pub(crate) const fn minimum(self) -> Option<f64> {
        self.minimum
    }

    /// Returns the inclusive maximum bound.
    #[must_use]
    pub(crate) const fn maximum(self) -> Option<f64> {
        self.maximum
    }

    /// Returns whether `value` lies inside the inclusive bounds.
    #[must_use]
    pub(crate) fn matches(self, value: f64) -> bool {
        if let Some(minimum) = self.minimum
            && value < minimum
        {
            return false;
        }
        if let Some(maximum) = self.maximum
            && value > maximum
        {
            return false;
        }
        true
    }
}

/// Vanilla `world.inventory.SlotRange`: a named range of vanilla slot ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlotRange {
    name: Box<str>,
    slots: Box<[i32]>,
}

impl SlotRange {
    /// The vanilla serialized name accepted by `SlotsArgument`.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The vanilla slot ids covered by this range, in vanilla order.
    #[must_use]
    pub(crate) fn slots(&self) -> &[i32] {
        &self.slots
    }

    /// Returns whether this range covers exactly one slot.
    #[must_use]
    pub(crate) fn is_single(&self) -> bool {
        self.slots.len() == 1
    }
}

/// Vanilla `MessageArgument.Message`: greedy text plus its selector parts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Message {
    text: Box<str>,
    parts: Box<[MessagePart]>,
}

impl Message {
    /// The raw message text as typed by the sender.
    #[must_use]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The entity-selector parts resolved when the message is read.
    #[must_use]
    pub(crate) fn parts(&self) -> &[MessagePart] {
        &self.parts
    }
}

/// One `@selector` occurrence inside a [`Message`], offset from the text start.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MessagePart {
    start: usize,
    end: usize,
    selector: Box<EntitySelector>,
}

impl MessagePart {
    /// Offset of the first character of the selector.
    #[must_use]
    pub(crate) const fn start(&self) -> usize {
        self.start
    }

    /// Offset one past the last character of the selector.
    #[must_use]
    pub(crate) const fn end(&self) -> usize {
        self.end
    }

    /// The parsed selector.
    #[must_use]
    pub(crate) fn selector(&self) -> &EntitySelector {
        &self.selector
    }
}

macro_rules! unit_argument_parser {
    (
        $parser:ident,
        $key:literal,
        $value:ty,
        parse |$reader:ident, $source:ident| $parse:block,
        suggest |$context:ident, $builder:ident| $suggest:block,
        protocol $protocol:expr
    ) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct $parser;

        impl_downcast_type!($parser, $key);

        impl SteelArgumentParser for $parser {
            type Value = $value;

            fn parse(
                &self,
                $reader: &mut StringReader<'_>,
                $source: &dyn CommandArgumentSource,
            ) -> Result<Self::Value, CommandSyntaxError> $parse

            fn list_suggestions(
                &self,
                $context: &dyn SteelArgumentSuggestionContext,
                $builder: &mut SuggestionsBuilder<'_>,
            ) $suggest

            fn protocol_argument(
                &self,
            ) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
                $protocol
            }
        }
    };
}

#[derive(Clone, Debug, PartialEq)]
struct PrimitiveParser(ArgumentType);

impl_downcast_type!(PrimitiveParser, "steel:command/parser/primitive");

impl SteelArgumentParser for PrimitiveParser {
    type Value = PrimitiveArgumentValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        self.0.parse_value(reader)
    }

    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        self.0.suggest(builder);
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (protocol_argument_type(&self.0), None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimeParser {
    minimum: i32,
}

impl_downcast_type!(TimeParser, "steel:command/parser/time");

impl SteelArgumentParser for TimeParser {
    type Value = TimeValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        parse_time(reader, self.minimum).map(TimeValue)
    }

    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        suggest_time_units(builder);
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (ProtocolArgumentType::Time { min: self.minimum }, None)
    }
}

unit_argument_parser!(
    BlockPosParser,
    "steel:command/parser/block_pos",
    Coordinates,
    parse | reader,
    _source | { parse_block_pos(reader) },
    suggest | _context,
    builder | {
        suggest_coordinates(builder, parse_block_pos);
    },
    protocol(ProtocolArgumentType::BlockPos, None)
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Vec3Parser {
    center_integers: bool,
}

impl_downcast_type!(Vec3Parser, "steel:command/parser/vec3");

impl SteelArgumentParser for Vec3Parser {
    type Value = Coordinates;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        parse_vec3(reader, self.center_integers)
    }

    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        suggest_coordinates(builder, |reader| parse_vec3(reader, self.center_integers));
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (ProtocolArgumentType::Vec3, None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Vec2Parser {
    center_integers: bool,
}

impl_downcast_type!(Vec2Parser, "steel:command/parser/vec2");

impl SteelArgumentParser for Vec2Parser {
    type Value = Coordinates;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        parse_vec2(reader, self.center_integers)
    }

    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        suggest_vec2(builder, |reader| parse_vec2(reader, self.center_integers));
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (ProtocolArgumentType::Vec2, None)
    }
}

unit_argument_parser!(
    RotationParser,
    "steel:command/parser/rotation",
    Coordinates,
    parse | reader,
    _source | { parse_rotation(reader) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::Rotation, None)
);
unit_argument_parser!(
    SwizzleParser,
    "steel:command/parser/swizzle",
    CoordinateAxes,
    parse | reader,
    _source | { parse_swizzle(reader) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::Swizzle, None)
);
unit_argument_parser!(
    HeightmapParser,
    "steel:command/parser/heightmap",
    HeightmapType,
    parse | reader,
    _source | { parse_heightmap(reader) },
    suggest | _context,
    builder | {
        suggest_heightmaps(builder);
    },
    protocol(ProtocolArgumentType::Heightmap, None)
);
unit_argument_parser!(
    EntityAnchorParser,
    "steel:command/parser/entity_anchor",
    EntityAnchor,
    parse | reader,
    _source | { parse_entity_anchor(reader) },
    suggest | _context,
    builder | {
        suggest_entity_anchors(builder);
    },
    protocol(ProtocolArgumentType::EntityAnchor, None)
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EntityTagParser;

impl_downcast_type!(EntityTagParser, "steel:command/parser/entity_tag");

impl SteelArgumentParser for EntityTagParser {
    type Value = PrimitiveArgumentValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        Ok(PrimitiveArgumentValue::String(
            reader.read_unquoted_string().into(),
        ))
    }

    fn list_suggestions(
        &self,
        context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        let Ok(targets) = context.argument("targets") else {
            return;
        };
        let Some(targets) = targets.downcast_ref::<EntitySelector>() else {
            return;
        };

        let prefix = builder.remaining();
        for tag in context
            .source()
            .entity_tag_suggestions(targets)
            .into_iter()
            .filter(|tag| matches_substring_ignore_case(prefix, tag))
        {
            builder.suggest(tag);
        }
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (
            ProtocolArgumentType::String {
                behavior: ArgumentStringTypeBehavior::SingleWord,
            },
            Some(ProtocolSuggestionType::AskServer),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EntityParser {
    single: bool,
    players_only: bool,
}

impl_downcast_type!(EntityParser, "steel:command/parser/entity");

impl SteelArgumentParser for EntityParser {
    type Value = EntitySelector;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        parse_entity_selector(reader, source, self.single, self.players_only)
    }

    fn list_suggestions(
        &self,
        context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        suggest_entity_selector(builder, context.source(), self.single, self.players_only);
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (
            ProtocolArgumentType::Entity {
                flags: u8::from(self.single) | (u8::from(self.players_only) << 1),
            },
            None,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScoreHolderParser {
    multiple: bool,
}

impl_downcast_type!(ScoreHolderParser, "steel:command/parser/score_holder");

impl SteelArgumentParser for ScoreHolderParser {
    type Value = ScoreHolderArgument;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        parse_score_holder(reader, source, self.multiple)
    }

    fn list_suggestions(
        &self,
        context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        suggest_score_holders(builder, context.source());
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (
            ProtocolArgumentType::ScoreHolder {
                flags: u8::from(self.multiple),
            },
            Some(ProtocolSuggestionType::AskServer),
        )
    }
}

unit_argument_parser!(
    ObjectiveParser,
    "steel:command/parser/objective",
    ObjectiveValue,
    parse | reader,
    _source | { Ok(ObjectiveValue(reader.read_unquoted_string().into())) },
    suggest | context,
    builder | {
        let prefix = builder.remaining();
        for objective in context
            .source()
            .scoreboard_objective_names()
            .into_iter()
            .filter(|objective| objective.starts_with(prefix))
        {
            builder.suggest(objective);
        }
    },
    protocol(
        ProtocolArgumentType::Objective,
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    IntRangeParser,
    "steel:command/parser/int_range",
    IntRange,
    parse | reader,
    _source | { parse_int_range(reader) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::IntRange, None)
);
unit_argument_parser!(
    BiomeOrTagParser,
    "steel:command/parser/biome_or_tag",
    BiomeOrTag,
    parse | reader,
    _source | { parse_biome_or_tag(reader) },
    suggest | _context,
    builder | {
        suggest_biomes(builder);
    },
    protocol(
        ProtocolArgumentType::ResourceOrTag {
            identifier: "minecraft:worldgen/biome",
        },
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    StructureOrTagKeyParser,
    "steel:command/parser/structure_or_tag_key",
    StructureOrTagKey,
    parse | reader,
    _source | { parse_structure_or_tag_key(reader) },
    suggest | _context,
    builder | {
        suggest_structures(builder);
    },
    protocol(
        ProtocolArgumentType::ResourceOrTagKey {
            identifier: "minecraft:worldgen/structure",
        },
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    BlockStateParser,
    "steel:command/parser/block_state",
    BlockInput,
    parse | reader,
    _source | { parse_block_input(reader) },
    suggest | _context,
    builder | {
        suggest_block_inputs(builder);
    },
    protocol(ProtocolArgumentType::BlockState, None)
);
unit_argument_parser!(
    BlockPredicateParser,
    "steel:command/parser/block_predicate",
    BlockPredicate,
    parse | reader,
    _source | { parse_block_predicate(reader) },
    suggest | _context,
    builder | {
        suggest_blocks(builder);
    },
    protocol(ProtocolArgumentType::BlockPredicate, None)
);
unit_argument_parser!(
    GameModeParser,
    "steel:command/parser/game_mode",
    GameModeValue,
    parse | reader,
    _source | { parse_game_mode(reader).map(GameModeValue) },
    suggest | _context,
    builder | {
        suggest_game_modes(builder);
    },
    protocol(ProtocolArgumentType::Gamemode, None)
);
unit_argument_parser!(
    DomainParser,
    "steel:command/parser/domain",
    DomainValue,
    parse | reader,
    source | { parse_domain(reader, source).map(DomainValue) },
    suggest | context,
    builder | {
        let prefix = builder.remaining();
        for domain in context
            .source()
            .domain_names()
            .into_iter()
            .filter(|domain| domain.starts_with(prefix))
        {
            builder.suggest(domain);
        }
    },
    protocol(
        ProtocolArgumentType::ResourceLocation,
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    WorldParser,
    "steel:command/parser/world",
    WorldArgument,
    parse | reader,
    _source | { parse_world_argument(reader) },
    suggest | context,
    builder | {
        suggest_worlds(builder, context.source());
    },
    protocol(
        ProtocolArgumentType::Dimension,
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    SummonableEntityParser,
    "steel:command/parser/summonable_entity",
    EntityTypeValue,
    parse | reader,
    _source | { parse_summonable_entity(reader).map(EntityTypeValue) },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY
                .entity_types
                .iter()
                .filter(|(_, entity_type)| can_summon(entity_type))
                .map(|(_, entity_type)| &entity_type.key),
            builder,
        );
    },
    protocol(
        ProtocolArgumentType::Resource {
            identifier: "minecraft:entity_type",
        },
        Some(ProtocolSuggestionType::SummonableEntities),
    )
);
unit_argument_parser!(
    EnchantmentParser,
    "steel:command/parser/enchantment",
    EnchantmentValue,
    parse | reader,
    _source | {
        let key = parse_identifier(reader)?;
        REGISTRY.enchantments.by_key(&key).map_or_else(
            || Err(unknown_resource(reader, &key, &ENCHANTMENT_REGISTRY)),
            |enchantment| Ok(EnchantmentValue(enchantment)),
        )
    },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY
                .enchantments
                .iter()
                .map(|(_, enchantment)| &enchantment.key),
            builder,
        );
    },
    protocol(
        ProtocolArgumentType::Resource {
            identifier: "minecraft:enchantment",
        },
        None,
    )
);
unit_argument_parser!(
    DamageTypeParser,
    "steel:command/parser/damage_type",
    DamageTypeValue,
    parse | reader,
    _source | {
        let key = parse_identifier(reader)?;
        REGISTRY.damage_types.by_key(&key).map_or_else(
            || Err(unknown_resource(reader, &key, &DAMAGE_TYPE_REGISTRY)),
            |damage_type| Ok(DamageTypeValue(damage_type)),
        )
    },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY
                .damage_types
                .iter()
                .map(|(_, damage_type)| &damage_type.key),
            builder,
        );
    },
    protocol(
        ProtocolArgumentType::Resource {
            identifier: "minecraft:damage_type",
        },
        None,
    )
);
unit_argument_parser!(
    AdvancementParser,
    "steel:command/parser/advancement",
    AdvancementValue,
    parse | reader,
    _source | { Ok(AdvancementValue(parse_advancement(reader)?)) },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY
                .advancements
                .advancements()
                .iter()
                .map(|adv| adv.key()),
            builder,
        );
    },
    protocol(
        ProtocolArgumentType::ResourceLocation,
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    ItemStackParser,
    "steel:command/parser/item_stack",
    ItemStackValue,
    parse | reader,
    _source | { parse_item_stack(reader).map(ItemStackValue) },
    suggest | _context,
    builder | {
        suggest_item_stack(builder);
    },
    protocol(
        ProtocolArgumentType::ItemStack,
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    ItemPredicateParser,
    "steel:command/parser/item_predicate",
    ItemPredicate,
    parse | reader,
    _source | { parse_item_predicate(reader) },
    suggest | _context,
    builder | {
        suggest_item_predicate(builder);
    },
    protocol(
        ProtocolArgumentType::ItemPredicate,
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    ComponentParser,
    "steel:command/parser/component",
    ComponentValue,
    parse | reader,
    _source | { parse_component(reader).map(ComponentValue) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::Component, None)
);
unit_argument_parser!(
    NbtTagParser,
    "steel:command/parser/nbt_tag",
    NbtTagValue,
    parse | reader,
    _source | { parse_nbt_tag(reader).map(NbtTagValue) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::NbtTag, None)
);
unit_argument_parser!(
    NbtPathParser,
    "steel:command/parser/nbt_path",
    NbtPathValue,
    parse | reader,
    _source | { parse_nbt_path(reader).map(NbtPathValue) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::NbtPath, None)
);
unit_argument_parser!(
    IdentifierParser,
    "steel:command/parser/identifier",
    IdentifierValue,
    parse | reader,
    _source | { parse_identifier(reader).map(IdentifierValue) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::ResourceLocation, None)
);
unit_argument_parser!(
    FloatRangeParser,
    "steel:command/parser/float_range",
    FloatRange,
    parse | reader,
    _source | { parse_float_range(reader) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::FloatRange, None)
);
unit_argument_parser!(
    AngleParser,
    "steel:command/parser/angle",
    AngleValue,
    parse | reader,
    _source | { parse_angle(reader).map(AngleValue) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::Angle, None)
);
unit_argument_parser!(
    UuidParser,
    "steel:command/parser/uuid",
    UuidValue,
    parse | reader,
    _source | { parse_uuid(reader).map(UuidValue) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::Uuid, None)
);
unit_argument_parser!(
    ParticleParser,
    "steel:command/parser/particle",
    ParticleValue,
    parse | reader,
    _source | {
        let key = parse_identifier(reader)?;
        REGISTRY.particle_types.by_key(&key).map_or_else(
            || {
                let message = translations::PARTICLE_NOT_FOUND
                    .message([key.to_string()])
                    .component();
                Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message))))
            },
            |particle| Ok(ParticleValue(particle)),
        )
    },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY
                .particle_types
                .iter()
                .map(|(_, particle)| &particle.key),
            builder,
        );
    },
    protocol(ProtocolArgumentType::Particle, None)
);
unit_argument_parser!(
    OperationParser,
    "steel:command/parser/operation",
    OperationValue,
    parse | reader,
    _source | { parse_operation(reader).map(OperationValue) },
    suggest | _context,
    builder | {
        suggest_operations(builder);
    },
    protocol(ProtocolArgumentType::Operation, None)
);
unit_argument_parser!(
    ScoreboardSlotParser,
    "steel:command/parser/scoreboard_slot",
    ScoreboardSlotValue,
    parse | reader,
    _source | { parse_display_slot(reader).map(ScoreboardSlotValue) },
    suggest | _context,
    builder | {
        suggest_display_slots(builder);
    },
    protocol(ProtocolArgumentType::ScoreboardSlot, None)
);
unit_argument_parser!(
    TeamParser,
    "steel:command/parser/team",
    TeamValue,
    parse | reader,
    _source | { parse_team_name(reader).map(TeamValue) },
    suggest | context,
    builder | {
        suggest_team_names(builder, context.source());
    },
    protocol(
        ProtocolArgumentType::Team,
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    TeamColorParser,
    "steel:command/parser/team_color",
    TeamColorValue,
    parse | reader,
    _source | { parse_team_color(reader).map(TeamColorValue) },
    suggest | _context,
    builder | {
        suggest_team_colors(builder);
    },
    protocol(ProtocolArgumentType::Color, None)
);
unit_argument_parser!(
    ObjectiveCriteriaParser,
    "steel:command/parser/objective_criteria",
    ObjectiveCriteriaValue,
    parse | reader,
    _source | { parse_objective_criteria(reader).map(ObjectiveCriteriaValue) },
    suggest | _context,
    builder | {
        suggest_objective_criteria(builder);
    },
    protocol(ProtocolArgumentType::ObjectiveCriteria, None)
);
unit_argument_parser!(
    SwingAnimationParser,
    "steel:command/parser/swing_animation",
    SwingAnimationValue,
    parse | reader,
    _source | { parse_swing_animation(reader).map(SwingAnimationValue) },
    suggest | _context,
    builder | {
        suggest_swing_animations(builder);
    },
    protocol(ProtocolArgumentType::SwingAnimation, None)
);
unit_argument_parser!(
    SlotsParser,
    "steel:command/parser/slots",
    SlotsValue,
    parse | reader,
    _source | { parse_slot_range(reader).map(SlotsValue) },
    suggest | _context,
    builder | {
        suggest_slot_ranges(builder, false);
    },
    protocol(ProtocolArgumentType::ItemSlots, None)
);
unit_argument_parser!(
    TemplateMirrorParser,
    "steel:command/parser/template_mirror",
    TemplateMirrorValue,
    parse | reader,
    _source | { parse_template_mirror(reader).map(TemplateMirrorValue) },
    suggest | _context,
    builder | {
        suggest_template_mirrors(builder);
    },
    protocol(ProtocolArgumentType::TemplateMirror, None)
);
unit_argument_parser!(
    TemplateRotationParser,
    "steel:command/parser/template_rotation",
    TemplateRotationValue,
    parse | reader,
    _source | { parse_template_rotation(reader).map(TemplateRotationValue) },
    suggest | _context,
    builder | {
        suggest_template_rotations(builder);
    },
    protocol(ProtocolArgumentType::TemplateRotation, None)
);
unit_argument_parser!(
    ColumnPosParser,
    "steel:command/parser/column_pos",
    ColumnPosValue,
    parse | reader,
    _source | { parse_column_pos(reader).map(ColumnPosValue) },
    suggest | _context,
    builder | {
        suggest_column_pos(builder);
    },
    protocol(ProtocolArgumentType::ColumnPos, None)
);
unit_argument_parser!(
    StyleParser,
    "steel:command/parser/style",
    StyleValue,
    parse | reader,
    _source | { parse_style(reader).map(StyleValue) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::Style, None)
);
unit_argument_parser!(
    HexColorParser,
    "steel:command/parser/hex_color",
    HexColorValue,
    parse | reader,
    _source | { parse_hex_color(reader).map(HexColorValue) },
    suggest | _context,
    builder | {
        builder.suggest("F00");
        builder.suggest("FF0000");
    },
    protocol(ProtocolArgumentType::HexColor, None)
);
unit_argument_parser!(
    MessageParser,
    "steel:command/parser/message",
    MessageValue,
    parse | reader,
    source | { parse_message(reader, source, source.allows_entity_selectors()).map(MessageValue) },
    suggest | _context,
    _builder | {},
    protocol(ProtocolArgumentType::Message, None)
);
unit_argument_parser!(
    MobEffectParser,
    "steel:command/parser/mob_effect",
    MobEffectValue,
    parse | reader,
    _source | {
        let key = parse_identifier(reader)?;
        REGISTRY.mob_effects.by_key(&key).map_or_else(
            || Err(unknown_resource(reader, &key, &MOB_EFFECT_REGISTRY)),
            |effect| Ok(MobEffectValue(effect)),
        )
    },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY.mob_effects.iter().map(|(_, effect)| &effect.key),
            builder,
        );
    },
    protocol(
        ProtocolArgumentType::Resource {
            identifier: "minecraft:mob_effect",
        },
        None,
    )
);
unit_argument_parser!(
    AttributeParser,
    "steel:command/parser/attribute",
    AttributeValue,
    parse | reader,
    _source | {
        let key = parse_identifier(reader)?;
        REGISTRY.attributes.by_key(&key).map_or_else(
            || {
                let message = translations::ARGUMENT_RESOURCE_NOT_FOUND
                    .message([
                        key.to_string(),
                        Identifier::vanilla_static("attribute").to_string(),
                    ])
                    .component();
                Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message))))
            },
            |attribute| Ok(AttributeValue(attribute)),
        )
    },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY
                .attributes
                .iter()
                .map(|(_, attribute)| &attribute.key),
            builder,
        );
    },
    protocol(
        ProtocolArgumentType::Resource {
            identifier: "minecraft:attribute",
        },
        None,
    )
);

unit_argument_parser!(
    StorageKeyParser,
    "steel:command/parser/storage_key",
    IdentifierValue,
    parse | reader,
    _source | { parse_identifier(reader).map(IdentifierValue) },
    suggest | context,
    builder | {
        suggest_storage_keys(context.source(), builder);
    },
    protocol(
        ProtocolArgumentType::ResourceLocation,
        Some(ProtocolSuggestionType::AskServer),
    )
);
unit_argument_parser!(
    SoundParser,
    "steel:command/parser/sound",
    IdentifierValue,
    parse | reader,
    _source | { parse_identifier(reader).map(IdentifierValue) },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY.sound_events.iter().map(|(_, sound)| &sound.key),
            builder,
        );
    },
    protocol(
        ProtocolArgumentType::ResourceLocation,
        Some(ProtocolSuggestionType::AvailableSounds),
    )
);
unit_argument_parser!(
    WorldClockParser,
    "steel:command/parser/world_clock",
    WorldClockValue,
    parse | reader,
    _source | {
        let key = parse_identifier(reader)?;
        REGISTRY.world_clocks.by_key(&key).map_or_else(
            || Err(unknown_resource(reader, &key, &WORLD_CLOCK_REGISTRY)),
            |clock| Ok(WorldClockValue(clock)),
        )
    },
    suggest | _context,
    builder | {
        suggest_resources(
            REGISTRY.world_clocks.iter().map(|(_, clock)| &clock.key),
            builder,
        );
    },
    protocol(
        ProtocolArgumentType::Resource {
            identifier: "minecraft:world_clock",
        },
        None,
    )
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimelineParser {
    clock_argument: Option<&'static str>,
}

impl_downcast_type!(TimelineParser, "steel:command/parser/timeline");

impl SteelArgumentParser for TimelineParser {
    type Value = TimelineValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        let key = parse_identifier(reader)?;
        REGISTRY.timelines.by_key(&key).map_or_else(
            || Err(unknown_resource(reader, &key, &TIMELINE_REGISTRY)),
            |timeline| Ok(TimelineValue(timeline)),
        )
    }

    fn list_suggestions(
        &self,
        context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        let Some(clock) = selected_clock(context, self.clock_argument) else {
            return;
        };
        suggest_resources(
            REGISTRY
                .timelines
                .iter()
                .filter(|(_, timeline)| timeline.clock == clock)
                .map(|(_, timeline)| &timeline.key),
            builder,
        );
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (
            ProtocolArgumentType::Resource {
                identifier: "minecraft:timeline",
            },
            Some(ProtocolSuggestionType::AskServer),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimeMarkerParser {
    clock_argument: Option<&'static str>,
}

impl_downcast_type!(TimeMarkerParser, "steel:command/parser/time_marker");

impl SteelArgumentParser for TimeMarkerParser {
    type Value = IdentifierValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        parse_identifier(reader).map(IdentifierValue)
    }

    fn list_suggestions(
        &self,
        context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        let Some(clock) = selected_clock(context, self.clock_argument) else {
            return;
        };
        suggest_resources(
            REGISTRY
                .timelines
                .iter()
                .filter(|(_, timeline)| timeline.clock == clock)
                .flat_map(|(_, timeline)| timeline.time_markers)
                .filter(|marker| marker.show_in_commands == Some(true))
                .map(|marker| &marker.key),
            builder,
        );
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (
            ProtocolArgumentType::ResourceLocation,
            Some(ProtocolSuggestionType::AskServer),
        )
    }
}

fn selected_clock(
    context: &dyn SteelArgumentSuggestionContext,
    clock_argument: Option<&str>,
) -> Option<WorldClockRef> {
    let Some(clock_argument) = clock_argument else {
        return context.source().default_world_clock();
    };
    context
        .argument(clock_argument)
        .ok()?
        .downcast_ref::<WorldClockValue>()
        .map(|clock| clock.0)
}

fn parse_component(reader: &mut StringReader<'_>) -> Result<TextComponent, CommandSyntaxError> {
    let start = reader.checkpoint();
    let (tag, consumed) = parse_snbt_argument(reader.remaining()).map_err(|error| {
        reader.advance_bytes(error.cursor());
        component_snbt_error(reader, error.component())
    })?;
    if !reader.advance_bytes(consumed) {
        return Err(component_snbt_error(
            reader,
            "Invalid text component cursor",
        ));
    }

    let component = TextComponent::try_from_nbt(&tag).map_err(|error| {
        reader.restore(start);
        invalid_component(reader, error.to_string())
    })?;
    validate_component_syntax(&component).map_err(|error| {
        reader.restore(start);
        invalid_component(reader, error)
    })?;
    Ok(component)
}

fn component_snbt_error(
    reader: &StringReader<'_>,
    message: impl Into<TextComponent>,
) -> CommandSyntaxError {
    reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message.into())))
}

fn invalid_component(reader: &StringReader<'_>, message: String) -> CommandSyntaxError {
    reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
        translations::ARGUMENT_COMPONENT_INVALID
            .message([message])
            .component(),
    )))
}

fn parse_swizzle(reader: &mut StringReader<'_>) -> Result<CoordinateAxes, CommandSyntaxError> {
    let mut axes = CoordinateAxes::default();
    while reader.can_read() && reader.peek() != Some(' ') {
        let bit = match reader.read() {
            Some('x') => CoordinateAxes::X,
            Some('y') => CoordinateAxes::Y,
            Some('z') => CoordinateAxes::Z,
            Some(_) | None => return Err(invalid_swizzle(reader)),
        };
        if axes.0 & bit != 0 {
            return Err(invalid_swizzle(reader));
        }
        axes.0 |= bit;
    }
    Ok(axes)
}

fn invalid_swizzle(reader: &StringReader<'_>) -> CommandSyntaxError {
    reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
        TextComponent::from(&translations::ARGUMENTS_SWIZZLE_INVALID),
    )))
}

fn parse_heightmap(reader: &mut StringReader<'_>) -> Result<HeightmapType, CommandSyntaxError> {
    let raw = reader.read_unquoted_string();
    match raw.to_ascii_lowercase().as_str() {
        "world_surface" => Ok(HeightmapType::WorldSurface),
        "motion_blocking" => Ok(HeightmapType::MotionBlocking),
        "motion_blocking_no_leaves" => Ok(HeightmapType::MotionBlockingNoLeaves),
        "ocean_floor" => Ok(HeightmapType::OceanFloor),
        _ => {
            let message = translations::ARGUMENT_ENUM_INVALID
                .message([raw.to_owned()])
                .component();
            Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message))))
        }
    }
}

fn suggest_heightmaps(builder: &mut SuggestionsBuilder<'_>) {
    const HEIGHTMAPS: &[&str] = &[
        "world_surface",
        "motion_blocking",
        "motion_blocking_no_leaves",
        "ocean_floor",
    ];
    for heightmap in HEIGHTMAPS {
        if heightmap.starts_with(builder.remaining_lowercase()) {
            builder.suggest(*heightmap);
        }
    }
}

fn parse_entity_anchor(reader: &mut StringReader<'_>) -> Result<EntityAnchor, CommandSyntaxError> {
    let start = reader.checkpoint();
    let name = reader.read_unquoted_string();
    match name {
        "feet" => Ok(EntityAnchor::Feet),
        "eyes" => Ok(EntityAnchor::Eyes),
        _ => {
            reader.restore(start);
            let message = translations::ARGUMENT_ANCHOR_INVALID
                .message([name.to_owned()])
                .component();
            Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message))))
        }
    }
}

fn suggest_entity_anchors(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining_lowercase().to_owned();
    for anchor in ["feet", "eyes"] {
        if anchor.starts_with(&prefix) {
            builder.suggest(anchor);
        }
    }
}

fn parse_game_mode(reader: &mut StringReader<'_>) -> Result<GameType, CommandSyntaxError> {
    let name = reader.read_unquoted_string();
    let game_mode = match name {
        "survival" => GameType::Survival,
        "creative" => GameType::Creative,
        "adventure" => GameType::Adventure,
        "spectator" => GameType::Spectator,
        _ => {
            let message = translations::ARGUMENT_GAMEMODE_INVALID
                .message([name.to_owned()])
                .component();
            return Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message))));
        }
    };
    Ok(game_mode)
}

fn suggest_game_modes(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining_lowercase().to_owned();
    for game_mode in [
        GameType::Survival,
        GameType::Creative,
        GameType::Adventure,
        GameType::Spectator,
    ] {
        let name = game_mode.name();
        if name.starts_with(&prefix) {
            builder.suggest(name);
        }
    }
}

fn parse_summonable_entity(
    reader: &mut StringReader<'_>,
) -> Result<EntityTypeRef, CommandSyntaxError> {
    let key = parse_identifier(reader)?;
    let Some(entity_type) = REGISTRY.entity_types.by_key(&key) else {
        return Err(unknown_resource(reader, &key, &ENTITY_TYPE_REGISTRY));
    };
    if can_summon(entity_type) {
        return Ok(entity_type);
    }
    let message = translations::ENTITY_NOT_SUMMONABLE
        .message([key.to_string()])
        .component();
    Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message))))
}

fn parse_advancement(reader: &mut StringReader<'_>) -> Result<AdvancementRef, CommandSyntaxError> {
    parse_identifier(reader).and_then(|key| {
        REGISTRY.advancements.by_key(&key).map_or(
            Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
                TextComponent::translated(
                    ADVANCEMENT_ADVANCEMENT_NOT_FOUND.message([key.to_string()]),
                ),
            )))),
            |val| Ok(val.value()),
        )
    })
}

fn can_summon(entity_type: EntityTypeRef) -> bool {
    entity_type.summonable
        && ENTITIES
            .get()
            .is_some_and(|registry| registry.has_factory(entity_type))
}

fn parse_domain<S>(
    reader: &mut StringReader<'_>,
    source: &S,
) -> Result<Box<str>, CommandSyntaxError>
where
    S: CommandArgumentSource + ?Sized,
{
    let domain = reader.read_unquoted_string();
    if source.domain_exists(domain) {
        return Ok(domain.into());
    }
    Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
        TextComponent::from(format!("Unknown domain {domain}")),
    ))))
}

pub(super) fn parse_identifier(
    reader: &mut StringReader<'_>,
) -> Result<Identifier, CommandSyntaxError> {
    let start = reader.checkpoint();
    let start_byte = reader.read_so_far().len();
    while reader.peek().is_some_and(is_allowed_in_identifier) {
        reader.skip();
    }
    let raw = &reader.read_so_far()[start_byte..];
    let (namespace, path) =
        raw.split_once(':')
            .map_or((Identifier::VANILLA_NAMESPACE, raw), |(namespace, path)| {
                if namespace.is_empty() {
                    (Identifier::VANILLA_NAMESPACE, path)
                } else {
                    (namespace, path)
                }
            });
    if namespace != ".."
        && Identifier::validate_namespace(namespace)
        && Identifier::validate_path(path)
    {
        return Ok(Identifier::new(namespace.to_owned(), path.to_owned()));
    }

    reader.restore(start);
    Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
        TextComponent::from(&translations::ARGUMENT_ID_INVALID),
    ))))
}

const fn is_allowed_in_identifier(character: char) -> bool {
    character.is_ascii_digit()
        || character.is_ascii_lowercase()
        || matches!(character, '_' | ':' | '/' | '.' | '-')
}

pub(super) fn unknown_resource(
    reader: &StringReader<'_>,
    key: &Identifier,
    registry: &Identifier,
) -> CommandSyntaxError {
    let message = translations::ARGUMENT_RESOURCE_NOT_FOUND
        .message([key.to_string(), registry.to_string()])
        .component();
    reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message)))
}

fn suggest_storage_keys<S>(source: &S, builder: &mut SuggestionsBuilder<'_>)
where
    S: CommandArgumentSource + ?Sized,
{
    let keys = source
        .command_storage_keys()
        .into_iter()
        .filter_map(|key| key.parse::<Identifier>().ok())
        .collect::<Vec<_>>();
    suggest_resources(keys.iter(), builder);
}

fn parse_time(reader: &mut StringReader<'_>, minimum: i32) -> Result<i32, CommandSyntaxError> {
    let value = reader.read_float()?;
    let unit = reader.read_unquoted_string();
    let factor = match unit {
        "d" => 24_000.0,
        "s" => 20.0,
        "t" | "" => 1.0,
        _ => {
            return Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
                TextComponent::from(&translations::ARGUMENT_TIME_INVALID_UNIT),
            ))));
        }
    };
    let ticks = java_round(value * factor);
    if ticks < minimum {
        let message = translations::ARGUMENT_TIME_TICK_COUNT_TOO_LOW
            .message([minimum.to_string(), ticks.to_string()])
            .component();
        return Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message))));
    }
    Ok(ticks)
}

fn suggest_time_units(builder: &mut SuggestionsBuilder<'_>) {
    let mut reader = StringReader::new(builder.remaining());
    if reader.read_float().is_err() {
        return;
    }
    let number = reader.read_so_far();
    let unit = reader.read_unquoted_string();
    for candidate in ["d", "s", "t"] {
        if candidate.starts_with(unit) {
            builder.suggest(format!("{number}{candidate}"));
        }
    }
}

fn java_round(value: f32) -> i32 {
    (value + 0.5).floor() as i32
}

fn dynamic_error(reader: &StringReader<'_>, component: TextComponent) -> CommandSyntaxError {
    reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(component)))
}

fn translated_error(
    reader: &StringReader<'_>,
    translation: &'static text_components::translation::Translation<0>,
) -> CommandSyntaxError {
    dynamic_error(reader, TextComponent::from(translation))
}

fn translated_argument_error(
    reader: &StringReader<'_>,
    translation: &'static text_components::translation::Translation<1>,
    value: impl Into<String>,
) -> CommandSyntaxError {
    dynamic_error(reader, translation.message([value.into()]).component())
}

fn parse_angle(reader: &mut StringReader<'_>) -> Result<Angle, CommandSyntaxError> {
    if !reader.can_read() {
        return Err(translated_error(
            reader,
            &translations::ARGUMENT_ANGLE_INCOMPLETE,
        ));
    }
    let relative = reader.peek() == Some('~');
    if relative {
        reader.skip();
    }
    let value = if reader.can_read() && reader.peek() != Some(' ') {
        reader.read_float()?
    } else {
        0.0
    };
    if value.is_nan() || value.is_infinite() {
        return Err(translated_error(
            reader,
            &translations::ARGUMENT_ANGLE_INVALID,
        ));
    }
    Ok(Angle::new(value, relative))
}

/// Vanilla `MessageArgument.Message.parseText`.
fn parse_message(
    reader: &mut StringReader<'_>,
    source: &dyn CommandArgumentSource,
    allow_selectors: bool,
) -> Result<Message, CommandSyntaxError> {
    const MAX_MESSAGE_LENGTH: usize = 256;

    let remaining = reader.remaining();
    // Vanilla bounds `StringReader.getRemainingLength()`, which is UTF-16 code units.
    let remaining_length = remaining.encode_utf16().count();
    if remaining_length > MAX_MESSAGE_LENGTH {
        return Err(dynamic_error(
            reader,
            translations::ARGUMENT_MESSAGE_TOO_LONG
                .message([remaining_length.to_string(), MAX_MESSAGE_LENGTH.to_string()])
                .component(),
        ));
    }

    let text: Box<str> = remaining.into();
    if !allow_selectors {
        reader.advance_bytes(remaining.len());
        return Ok(Message {
            text,
            parts: Box::new([]),
        });
    }

    let mut parts = Vec::new();
    let mut index = 0;
    let mut consumed = 0;
    while index < remaining.len() {
        if remaining.as_bytes()[index] != b'@' {
            index += 1;
            continue;
        }
        let start = index;
        if !remaining[start + 1..]
            .chars()
            .next()
            .is_some_and(is_selector_type)
        {
            // Vanilla treats a missing or unknown selector type as literal text.
            index += 1;
            continue;
        }

        if !reader.advance_bytes(start - consumed) {
            index += 1;
            continue;
        }
        let checkpoint = reader.checkpoint();
        match parse_entity_selector(reader, source, false, false) {
            Ok(selector) => {
                let end = remaining.len() - reader.remaining().len();
                consumed = end;
                parts.push(MessagePart {
                    start,
                    end,
                    selector: Box::new(selector),
                });
                index = end;
            }
            Err(error) => {
                reader.restore(checkpoint);
                return Err(error);
            }
        }
    }

    reader.advance_bytes(remaining.len() - consumed);
    Ok(Message {
        text,
        parts: parts.into_boxed_slice(),
    })
}

const fn is_selector_type(character: char) -> bool {
    matches!(character, 'a' | 'e' | 'n' | 'p' | 'r' | 's')
}

/// Vanilla `MessageArgument.Part.toComponent` and `EntitySelector.joinNames`.
pub(crate) fn resolve_message(
    message: &Message,
    source: &CommandSource,
) -> Result<TextComponent, CommandSyntaxError> {
    if message.parts.is_empty() {
        return Ok(TextComponent::plain(message.text.to_string()));
    }

    let mut result = TextComponent::new();
    result.children.push(TextComponent::plain(
        message.text[..message.parts[0].start].to_string(),
    ));
    let mut read_to = message.parts[0].start;
    for part in message.parts.iter() {
        let entities = part.selector.find_entities(source)?;
        if read_to < part.start {
            result.children.push(TextComponent::plain(
                message.text[read_to..part.start].to_string(),
            ));
        }
        result.children.push(join_names(&entities));
        read_to = part.end;
    }
    if read_to < message.text.len() {
        result
            .children
            .push(TextComponent::plain(message.text[read_to..].to_string()));
    }
    Ok(result)
}

fn join_names(entities: &[crate::entity::SharedEntity]) -> TextComponent {
    match entities {
        [] => TextComponent::new(),
        [only] => only.display_name(),
        many => {
            let mut component = TextComponent::new();
            for (index, entity) in many.iter().enumerate() {
                if index > 0 {
                    component.children.push(TextComponent::plain(", "));
                }
                component.children.push(entity.display_name());
            }
            component
        }
    }
}

fn parse_hex_color(reader: &mut StringReader<'_>) -> Result<i32, CommandSyntaxError> {
    let raw = reader.read_unquoted_string();
    let channel = |start: usize, length: usize| -> Option<i32> {
        let slice = raw.get(start..start + length)?;
        if length == 1 {
            let digit = slice.chars().next()?.to_digit(16)?;
            Some(digit as i32 * 17)
        } else {
            i32::from_str_radix(slice, 16).ok()
        }
    };
    let color = match raw.len() {
        3 => channel(0, 1).zip(channel(1, 1)).zip(channel(2, 1)),
        6 => channel(0, 2).zip(channel(2, 2)).zip(channel(4, 2)),
        _ => None,
    };
    let Some(((red, green), blue)) = color else {
        return Err(translated_argument_error(
            reader,
            &translations::ARGUMENT_HEXCOLOR_INVALID,
            raw,
        ));
    };
    Ok(0xFF00_0000u32 as i32 | (red << 16) | (green << 8) | blue)
}

fn parse_uuid(reader: &mut StringReader<'_>) -> Result<Uuid, CommandSyntaxError> {
    let remaining = reader.remaining();
    let length = remaining
        .find(|character: char| !matches!(character, '-' | '0'..='9' | 'a'..='f' | 'A'..='F'))
        .unwrap_or(remaining.len());
    let candidate = &remaining[..length];
    if candidate.len() == 36
        && let Ok(uuid) = Uuid::parse_str(candidate)
    {
        reader.advance_bytes(length);
        return Ok(uuid);
    }
    Err(translated_error(
        reader,
        &translations::ARGUMENT_UUID_INVALID,
    ))
}

fn parse_operation(reader: &mut StringReader<'_>) -> Result<Operation, CommandSyntaxError> {
    if !reader.can_read() {
        return Err(translated_error(
            reader,
            &translations::ARGUMENTS_OPERATION_INVALID,
        ));
    }
    let raw = reader.read_unquoted_token();
    Operation::from_symbol(raw)
        .ok_or_else(|| translated_error(reader, &translations::ARGUMENTS_OPERATION_INVALID))
}

fn suggest_operations(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining().to_owned();
    for operation in Operation::ALL {
        let symbol = operation.symbol();
        if symbol.starts_with(&prefix) {
            builder.suggest(symbol);
        }
    }
}

fn parse_display_slot(reader: &mut StringReader<'_>) -> Result<DisplaySlot, CommandSyntaxError> {
    let raw = reader.read_unquoted_string();
    let parsed = DisplaySlot::from_str(raw);
    if let Some(slot) = parsed {
        return Ok(slot);
    }
    Err(translated_argument_error(
        reader,
        &translations::ARGUMENT_SCOREBOARD_DISPLAY_SLOT_INVALID,
        raw,
    ))
}

fn suggest_display_slots(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining().to_owned();
    for slot in DisplaySlot::ALL {
        let name = slot.as_str();
        if name.starts_with(&prefix) {
            builder.suggest(name);
        }
    }
}

fn parse_team_name(reader: &mut StringReader<'_>) -> Result<Box<str>, CommandSyntaxError> {
    Ok(reader.read_unquoted_string().into())
}

fn suggest_team_names(builder: &mut SuggestionsBuilder<'_>, source: &dyn CommandArgumentSource) {
    let prefix = builder.remaining();
    for team in source
        .selector_team_names()
        .into_iter()
        .filter(|team| team.starts_with(prefix))
    {
        builder.suggest(team);
    }
}

fn parse_team_color(reader: &mut StringReader<'_>) -> Result<TeamColor, CommandSyntaxError> {
    let raw = reader.read_unquoted_string();
    let parsed = TeamColor::from_str(raw);
    if let Some(color) = parsed {
        return Ok(color);
    }
    Err(translated_argument_error(
        reader,
        &translations::ARGUMENT_COLOR_INVALID,
        raw,
    ))
}

fn suggest_team_colors(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining().to_owned();
    for color in TeamColor::ALL {
        let name = color.as_str();
        if name.starts_with(&prefix) {
            builder.suggest(name);
        }
    }
}

fn objective_criteria_name(stat_type: &Identifier, value: &Identifier) -> String {
    format!(
        "{}.{}:{}.{}",
        stat_type.namespace, stat_type.path, value.namespace, value.path
    )
}

fn parse_objective_criteria(reader: &mut StringReader<'_>) -> Result<Box<str>, CommandSyntaxError> {
    let raw = reader.read_unquoted_token();
    if ObjectiveCriteria::parse(raw).is_some() {
        Ok(raw.into())
    } else {
        Err(translated_argument_error(
            reader,
            &translations::ARGUMENT_CRITERIA_INVALID,
            raw,
        ))
    }
}

fn suggest_objective_criteria(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining().to_owned();
    for criteria in ObjectiveCriteria::custom_criteria_names() {
        if criteria.starts_with(&prefix) {
            builder.suggest(criteria);
        }
    }
    for (_, entry) in REGISTRY.stat_types.iter() {
        for id in 0..entry.registry_len() {
            let Some(value) = entry.key_from_id(id) else {
                continue;
            };
            let name = objective_criteria_name(&entry.key, value);
            if name.starts_with(&prefix) {
                builder.suggest(name);
            }
        }
    }
}

fn parse_swing_animation(
    reader: &mut StringReader<'_>,
) -> Result<SwingAnimationType, CommandSyntaxError> {
    let raw = reader.read_unquoted_string();
    let parsed = [
        SwingAnimationType::None,
        SwingAnimationType::Whack,
        SwingAnimationType::Stab,
    ]
    .into_iter()
    .find(|animation| animation.serialized_name() == raw);
    if let Some(animation) = parsed {
        return Ok(animation);
    }
    Err(translated_argument_error(
        reader,
        &translations::ARGUMENT_SWING_ANIMATION_INVALID,
        raw,
    ))
}

fn suggest_swing_animations(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining().to_owned();
    for animation in [
        SwingAnimationType::None,
        SwingAnimationType::Whack,
        SwingAnimationType::Stab,
    ] {
        let name = animation.serialized_name();
        if name.starts_with(&prefix) {
            builder.suggest(name);
        }
    }
}

fn parse_template_mirror(
    reader: &mut StringReader<'_>,
) -> Result<StructureMirror, CommandSyntaxError> {
    let raw = reader.read_unquoted_string();
    match raw {
        "none" => Ok(StructureMirror::None),
        "left_right" => Ok(StructureMirror::LeftRight),
        "front_back" => Ok(StructureMirror::FrontBack),
        _ => Err(translated_argument_error(
            reader,
            &translations::ARGUMENT_ENUM_INVALID,
            raw,
        )),
    }
}

fn suggest_template_mirrors(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining().to_owned();
    for name in ["none", "left_right", "front_back"] {
        if name.starts_with(&prefix) {
            builder.suggest(name);
        }
    }
}

fn parse_template_rotation(reader: &mut StringReader<'_>) -> Result<Rotation, CommandSyntaxError> {
    let raw = reader.read_unquoted_string();
    match raw {
        "none" => Ok(Rotation::None),
        "clockwise_90" => Ok(Rotation::Clockwise90),
        "180" => Ok(Rotation::Clockwise180),
        "counterclockwise_90" => Ok(Rotation::CounterClockwise90),
        _ => Err(translated_argument_error(
            reader,
            &translations::ARGUMENT_ENUM_INVALID,
            raw,
        )),
    }
}

fn suggest_template_rotations(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining().to_owned();
    for name in ["none", "clockwise_90", "180", "counterclockwise_90"] {
        if name.starts_with(&prefix) {
            builder.suggest(name);
        }
    }
}

fn parse_column_pos(reader: &mut StringReader<'_>) -> Result<Coordinates, CommandSyntaxError> {
    let start = reader.checkpoint();
    if !reader.can_read() {
        return Err(translated_error(
            reader,
            &translations::ARGUMENT_POS2D_INCOMPLETE,
        ));
    }
    let x = parse_column_coordinate(reader)?;
    if reader.peek() != Some(' ') {
        reader.restore(start);
        return Err(translated_error(
            reader,
            &translations::ARGUMENT_POS2D_INCOMPLETE,
        ));
    }
    reader.skip();
    let z = parse_column_coordinate(reader)?;
    Ok(Coordinates::World(WorldCoordinates::new(
        x,
        WorldCoordinate::new(true, 0.0),
        z,
    )))
}

fn parse_column_coordinate(
    reader: &mut StringReader<'_>,
) -> Result<WorldCoordinate, CommandSyntaxError> {
    if reader.peek() == Some('^') {
        return Err(translated_error(reader, &translations::ARGUMENT_POS_MIXED));
    }
    if !reader.can_read() {
        return Err(translated_error(
            reader,
            &translations::ARGUMENT_POS_MISSING_INT,
        ));
    }
    let relative = reader.peek() == Some('~');
    if relative {
        reader.skip();
    }
    let value = if reader.can_read() && reader.peek() != Some(' ') {
        if relative {
            reader.read_double()?
        } else {
            f64::from(reader.read_int()?)
        }
    } else {
        0.0
    };
    Ok(WorldCoordinate::new(relative, value))
}

fn suggest_column_pos(builder: &mut SuggestionsBuilder<'_>) {
    let input = builder.remaining();
    let coordinate = if input.starts_with('^') { "^" } else { "~" };
    if input.is_empty() {
        let full = format!("{coordinate} {coordinate}");
        builder.suggest(coordinate);
        builder.suggest(full);
        return;
    }
    let fields = input.trim_end().split(' ').collect::<Vec<_>>();
    if let [x] = fields.as_slice() {
        builder.suggest(format!("{x} {coordinate}"));
    }
}

fn parse_float_range(reader: &mut StringReader<'_>) -> Result<FloatRange, CommandSyntaxError> {
    let start = reader.checkpoint();
    if !reader.can_read() {
        return Err(range_error(
            reader,
            start,
            &translations::ARGUMENT_RANGE_EMPTY,
        ));
    }

    let minimum = read_optional_double(reader, start)?;
    let maximum = if reader.can_read_length(2)
        && reader.peek() == Some('.')
        && peek_next(reader) == Some('.')
    {
        reader.skip();
        reader.skip();
        read_optional_double(reader, start)?
    } else {
        minimum
    };

    if minimum.is_none() && maximum.is_none() {
        return Err(range_error(
            reader,
            start,
            &translations::ARGUMENT_RANGE_EMPTY,
        ));
    }
    if minimum
        .zip(maximum)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(range_error(
            reader,
            start,
            &translations::ARGUMENT_RANGE_SWAPPED,
        ));
    }
    Ok(FloatRange::new(minimum, maximum))
}

fn read_optional_double(
    reader: &mut StringReader<'_>,
    range_start: ReaderCursor,
) -> Result<Option<f64>, CommandSyntaxError> {
    let start = reader.read_so_far().len();
    while reader
        .peek()
        .is_some_and(|character| is_range_number_character(character, peek_next(reader)))
    {
        reader.skip();
    }
    let raw = &reader.input()[start..reader.read_so_far().len()];
    if raw.is_empty() {
        return Ok(None);
    }
    let Ok(value) = raw.parse() else {
        reader.restore(range_start);
        return Err(reader.error(CommandSyntaxErrorKind::InvalidDouble(raw.into())));
    };
    Ok(Some(value))
}

fn peek_next(reader: &StringReader<'_>) -> Option<char> {
    reader.remaining().chars().nth(1)
}

fn is_range_number_character(character: char, next: Option<char>) -> bool {
    character.is_ascii_digit()
        || character == '-'
        || character == '+'
        || (character == '.' && next != Some('.'))
}

fn range_error(
    reader: &mut StringReader<'_>,
    start: ReaderCursor,
    translation: &'static text_components::translation::Translation<0>,
) -> CommandSyntaxError {
    reader.restore(start);
    translated_error(reader, translation)
}

fn parse_nbt_tag(reader: &mut StringReader<'_>) -> Result<NbtTag, CommandSyntaxError> {
    let (tag, consumed) = parse_snbt_argument(reader.remaining()).map_err(|error| {
        reader.advance_bytes(error.cursor());
        dynamic_error(reader, error.component())
    })?;
    if !reader.advance_bytes(consumed) {
        return Err(dynamic_error(
            reader,
            TextComponent::plain("Invalid NBT tag cursor"),
        ));
    }
    Ok(tag)
}

fn parse_style(reader: &mut StringReader<'_>) -> Result<Format, CommandSyntaxError> {
    let tag = parse_nbt_tag(reader).map_err(|error| {
        reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
            translations::ARGUMENT_STYLE_INVALID
                .message([error.message_component()])
                .component(),
        )))
    })?;
    let NbtTag::Compound(compound) = tag else {
        return Err(translated_argument_error(
            reader,
            &translations::ARGUMENT_STYLE_INVALID,
            "Style must be a compound tag",
        ));
    };

    let mut style = Format::new();
    for (key, value) in compound.iter() {
        match key.to_str().as_ref() {
            "color" => style.color = Some(parse_style_color(reader, value)?),
            "font" => {
                let NbtTag::String(font) = value else {
                    return Err(translated_argument_error(
                        reader,
                        &translations::ARGUMENT_STYLE_INVALID,
                        "font must be a string",
                    ));
                };
                style.font = Some(std::borrow::Cow::Owned(font.to_str().into_owned()));
            }
            "bold" => style.bold = Some(parse_style_bool(reader, value)?),
            "italic" => style.italic = Some(parse_style_bool(reader, value)?),
            "underlined" => style.underlined = Some(parse_style_bool(reader, value)?),
            "strikethrough" => style.strikethrough = Some(parse_style_bool(reader, value)?),
            "obfuscated" => style.obfuscated = Some(parse_style_bool(reader, value)?),
            "shadow_color" => style.shadow_color = Some(parse_style_int(reader, value)?),
            // Steel's `Format` has no field for these vanilla style members.
            unsupported => {
                return Err(translated_argument_error(
                    reader,
                    &translations::ARGUMENT_STYLE_INVALID,
                    format!("Unsupported style field {unsupported}"),
                ));
            }
        }
    }
    Ok(style)
}

fn parse_style_color(
    reader: &StringReader<'_>,
    value: &NbtTag,
) -> Result<text_components::format::Color, CommandSyntaxError> {
    let NbtTag::String(color) = value else {
        return Err(translated_argument_error(
            reader,
            &translations::ARGUMENT_STYLE_INVALID,
            "color must be a string",
        ));
    };
    let color = color.to_str();
    if let Some(color) = text_components::format::Color::from_hex(&color) {
        return Ok(color);
    }
    named_style_color(&color).ok_or_else(|| {
        translated_argument_error(
            reader,
            &translations::ARGUMENT_STYLE_INVALID,
            format!("Unknown color {color}"),
        )
    })
}

fn named_style_color(name: &str) -> Option<text_components::format::Color> {
    use text_components::format::Color;
    Some(match name {
        "black" => Color::Black,
        "dark_blue" => Color::DarkBlue,
        "dark_green" => Color::DarkGreen,
        "dark_aqua" => Color::DarkAqua,
        "dark_red" => Color::DarkRed,
        "dark_purple" => Color::DarkPurple,
        "gold" => Color::Gold,
        "gray" => Color::Gray,
        "dark_gray" => Color::DarkGray,
        "blue" => Color::Blue,
        "green" => Color::Green,
        "aqua" => Color::Aqua,
        "red" => Color::Red,
        "light_purple" => Color::LightPurple,
        "yellow" => Color::Yellow,
        "white" => Color::White,
        _ => return None,
    })
}

fn parse_style_bool(reader: &StringReader<'_>, value: &NbtTag) -> Result<bool, CommandSyntaxError> {
    match value {
        NbtTag::Byte(byte) => Ok(*byte != 0),
        NbtTag::Int(int) => Ok(*int != 0),
        _ => Err(translated_argument_error(
            reader,
            &translations::ARGUMENT_STYLE_INVALID,
            "Style flags must be bytes",
        )),
    }
}

fn parse_style_int(reader: &StringReader<'_>, value: &NbtTag) -> Result<i32, CommandSyntaxError> {
    match value {
        NbtTag::Int(int) => Ok(*int),
        NbtTag::Byte(byte) => Ok(i32::from(*byte)),
        NbtTag::Short(short) => Ok(i32::from(*short)),
        NbtTag::Long(long) => i32::try_from(*long).map_err(|_| {
            translated_argument_error(
                reader,
                &translations::ARGUMENT_STYLE_INVALID,
                "shadow_color is out of range",
            )
        }),
        _ => Err(translated_argument_error(
            reader,
            &translations::ARGUMENT_STYLE_INVALID,
            "shadow_color must be an int",
        )),
    }
}

fn parse_slot_range(reader: &mut StringReader<'_>) -> Result<SlotRange, CommandSyntaxError> {
    let raw = reader.read_unquoted_string();
    let parsed = slot_ranges()
        .iter()
        .find(|range| range.name() == raw)
        .cloned();
    if let Some(range) = parsed {
        return Ok(range);
    }
    Err(translated_argument_error(
        reader,
        &translations::SLOT_UNKNOWN,
        raw,
    ))
}

fn suggest_slot_ranges(builder: &mut SuggestionsBuilder<'_>, single_only: bool) {
    let prefix = builder.remaining();
    for range in slot_ranges() {
        if single_only && !range.is_single() {
            continue;
        }
        if range.name().starts_with(prefix) {
            builder.suggest(range.name());
        }
    }
}

/// Vanilla `SlotRanges`: every named slot range, in vanilla declaration order.
fn slot_ranges() -> &'static [SlotRange] {
    static SLOT_RANGES: std::sync::LazyLock<Vec<SlotRange>> =
        std::sync::LazyLock::new(build_slot_ranges);
    &SLOT_RANGES
}

fn build_slot_ranges() -> Vec<SlotRange> {
    fn single(ranges: &mut Vec<SlotRange>, name: impl Into<String>, id: i32) {
        push(ranges, name, vec![id]);
    }

    fn push(ranges: &mut Vec<SlotRange>, name: impl Into<String>, slots: Vec<i32>) {
        ranges.push(SlotRange {
            name: name.into().into_boxed_str(),
            slots: slots.into_boxed_slice(),
        });
    }

    fn range(ranges: &mut Vec<SlotRange>, prefix: &str, offset: i32, size: i32) {
        let mut all = Vec::with_capacity(size as usize);
        for index in 0..size {
            let slot = offset + index;
            single(ranges, format!("{prefix}{index}"), slot);
            all.push(slot);
        }
        push(ranges, format!("{prefix}*"), all);
    }

    let mut ranges = Vec::new();
    single(&mut ranges, "contents", 0);
    range(&mut ranges, "container.", 0, 54);
    range(&mut ranges, "hotbar.", 0, 9);
    range(&mut ranges, "inventory.", 9, 27);
    range(&mut ranges, "enderchest.", 200, 27);
    range(&mut ranges, "mob.inventory.", 300, 8);
    range(&mut ranges, "horse.", 500, 15);
    single(&mut ranges, "weapon", 98);
    single(&mut ranges, "weapon.mainhand", 98);
    single(&mut ranges, "weapon.offhand", 99);
    push(&mut ranges, "weapon.*", vec![98, 99]);
    single(&mut ranges, "armor.head", 103);
    single(&mut ranges, "armor.chest", 102);
    single(&mut ranges, "armor.legs", 101);
    single(&mut ranges, "armor.feet", 100);
    single(&mut ranges, "armor.body", 105);
    push(&mut ranges, "armor.*", vec![103, 102, 101, 100, 105]);
    single(&mut ranges, "saddle", 106);
    single(&mut ranges, "horse.chest", 499);
    single(&mut ranges, "player.cursor", 499);
    range(&mut ranges, "player.crafting.", 500, 4);
    ranges
}

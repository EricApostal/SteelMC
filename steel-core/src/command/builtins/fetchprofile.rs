//! Vanilla `/fetchprofile` command.
//!
//! Mirrors `FetchProfileCommand`: `name <name>` and `id <uuid>` resolve a game
//! profile asynchronously through the server's profile resolver, and
//! `entity <target>` reads the profile straight off an avatar, which in Steel is
//! a player or a mannequin.
//!
//! Each success renders the profile twice: as the `PlayerSprite` chat-object
//! component the client draws as the player's head, and as that component's SNBT
//! so the click actions can hand the profile to `/give` and `/summon mannequin`.
//!
//! Deviations:
//! - Vanilla's `ProfileResolver` returns profiles that carry the `textures`
//!   property, so its sprite ships skin data. Steel's identity cache stores only
//!   UUID and last-known name, so a looked-up sprite carries exactly those two
//!   fields and the client resolves the skin itself. `/fetchprofile entity` does
//!   pass through whatever the entity's own profile holds, including properties.
//! - Vanilla's `ProfileResolver.fetchByName` only knows cached or session-server
//!   names. Steel routes both forms through the shared identity resolver, which in
//!   offline mode mints the deterministic offline profile for any valid name, so
//!   `name <unknown>` can resolve where vanilla would fail.
//! - Vanilla returns 1 immediately and reports the lookup on the IO pool later.
//!   Steel runs the lookup as a suspension of the source's execution: the result
//!   and feedback are still vanilla's, but later commands from the same source
//!   wait for the lookup, as with `/ban`.

use simdnbt::ToNbtTag;
use steel_registry::game_profile::ProfileProperty;
use steel_registry::resolvable_profile::{
    PartialProfile, PlayerModelType, PlayerSkinPatch, ResolvableProfile, ResolvableProfileContents,
};
use steel_utils::{
    Downcast as _, Identifier, UuidExt as _,
    nbt::to_canonical_snbt,
    translations::{
        COMMANDS_FETCHPROFILE_COPY_COMPONENT, COMMANDS_FETCHPROFILE_COPY_TEXT,
        COMMANDS_FETCHPROFILE_ENTITY_SUCCESS, COMMANDS_FETCHPROFILE_GIVE_ITEM,
        COMMANDS_FETCHPROFILE_ID_FAILURE, COMMANDS_FETCHPROFILE_ID_SUCCESS,
        COMMANDS_FETCHPROFILE_NAME_FAILURE, COMMANDS_FETCHPROFILE_NAME_SUCCESS,
        COMMANDS_FETCHPROFILE_NO_PROFILE, COMMANDS_FETCHPROFILE_SUMMON_MANNEQUIN,
    },
};
use text_components::{
    Modifier as _, TextComponent,
    content::{Object, ObjectPlayer, PlayerModel, PlayerProperties},
    format::Color,
    interactivity::ClickEvent,
    translation::Translation,
};
use tokio::{sync::oneshot, task::JoinHandle};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandResultSuspension, CommandResultSuspensionPoll, CommandSource,
        CommandSuspensionOrder, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        argument, literal,
    },
    registration::CommandRegistration,
};
use crate::{
    entity::{SharedEntity, entities::MannequinEntity},
    permission::PermissionLevel,
};

/// The feedback a completed `/fetchprofile name|id` lookup produces: the success
/// message, or the vanilla failure message for its source.
type FetchProfileOutcome = Result<TextComponent, TextComponent>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("fetchprofile"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("fetchprofile")
        .then(
            literal("name").then(
                argument("name", ArgumentType::greedy_string()).executes_suspended(resolve_name),
            ),
        )
        .then(
            literal("id")
                .then(argument("id", SteelArgumentType::uuid()).executes_suspended(resolve_id)),
        )
        .then(
            literal("entity")
                .then(argument("entity", SteelArgumentType::entity()).executes(print_for_entity)),
        )
}

/// Vanilla `FetchProfileCommand.resolveName`.
fn resolve_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<FetchProfileSuspension, CommandSyntaxError> {
    let name = context.string("name")?.to_owned();
    let source = context.source().clone();
    Ok(FetchProfileSuspension::spawn(source.clone(), async move {
        lookup_profile(
            &source,
            &name,
            TextComponent::plain(name.clone()),
            &COMMANDS_FETCHPROFILE_NAME_SUCCESS,
            &COMMANDS_FETCHPROFILE_NAME_FAILURE,
        )
        .await
    }))
}

/// Vanilla `FetchProfileCommand.resolveId`.
fn resolve_id(
    context: &SteelCommandContext<CommandSource>,
) -> Result<FetchProfileSuspension, CommandSyntaxError> {
    let uuid = context.uuid("id")?;
    let source = context.source().clone();
    Ok(FetchProfileSuspension::spawn(source.clone(), async move {
        // Vanilla's `Component.translationArg(uuid)` is a plain UUID literal.
        lookup_profile(
            &source,
            &uuid.to_string(),
            TextComponent::plain(uuid.to_string()),
            &COMMANDS_FETCHPROFILE_ID_SUCCESS,
            &COMMANDS_FETCHPROFILE_ID_FAILURE,
        )
        .await
    }))
}

/// Vanilla `ProfileResolver.fetchByName`/`fetchById` followed by
/// `reportResolvedProfile`.
#[expect(
    clippy::result_large_err,
    reason = "vanilla mirrors TextComponent conventions for suspension payloads"
)]
async fn lookup_profile(
    source: &CommandSource,
    requested: &str,
    argument: TextComponent,
    success: &'static Translation<2>,
    failure: &'static Translation<1>,
) -> FetchProfileOutcome {
    let profile = match source.server().resolve_player_profile(requested).await {
        Ok(profile) => profile,
        Err(error) => {
            tracing::debug!(%error, %requested, "failed to resolve a profile for /fetchprofile");
            return Err(failure.message([argument]).component());
        }
    };

    let Ok(partial) = PartialProfile::new(
        Some(profile.last_known_name().to_owned()),
        Some(profile.uuid()),
        Vec::new(),
    ) else {
        return Err(failure.message([argument]).component());
    };
    let resolved = ResolvableProfile::static_partial(partial, PlayerSkinPatch::default());
    Ok(report_resolved_profile(success, argument, &resolved))
}

/// Vanilla `FetchProfileCommand.printForEntity`.
fn print_for_entity(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("entity")?;
    let Some(profile) = avatar_profile(&target) else {
        let message = COMMANDS_FETCHPROFILE_NO_PROFILE
            .message([target.display_name()])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    };

    let message = report_resolved_profile(
        &COMMANDS_FETCHPROFILE_ENTITY_SUCCESS,
        target.display_name(),
        &profile,
    );
    context.source().send_success(&message, false);
    Ok(1)
}

/// Vanilla `Avatar.getProfile` for the entity types that carry a profile.
fn avatar_profile(entity: &SharedEntity) -> Option<ResolvableProfile> {
    if let Some(player) = entity.as_player() {
        return Some(ResolvableProfile::from(player.gameprofile.clone()));
    }
    entity
        .downcast_ref::<MannequinEntity>()
        .map(MannequinEntity::profile)
}

/// Vanilla `FetchProfileCommand.reportResolvedProfile`: the resolved profile's
/// own SNBT, the `PlayerSprite` component built from it, and the click actions
/// that reuse both.
fn report_resolved_profile(
    success: &Translation<2>,
    argument: TextComponent,
    profile: &ResolvableProfile,
) -> TextComponent {
    // `ToNbtTag` consumes its receiver, so the borrowed profile is cloned once
    // for the SNBT form the click actions embed.
    let encoded_profile = to_canonical_snbt(&profile.clone().to_nbt_tag()).unwrap_or_default();
    let sprite = player_sprite(profile);
    let encoded_component = to_canonical_snbt(&sprite.to_codec_nbt()).unwrap_or_default();

    let copy_component = COMMANDS_FETCHPROFILE_COPY_COMPONENT
        .msg()
        .component()
        .click_event(ClickEvent::copy_to_clipboard(encoded_component.clone()));
    let give_item = COMMANDS_FETCHPROFILE_GIVE_ITEM
        .msg()
        .component()
        .click_event(ClickEvent::run_command(format!(
            "give @s minecraft:player_head[profile={encoded_profile}]"
        )));
    let summon_mannequin = COMMANDS_FETCHPROFILE_SUMMON_MANNEQUIN
        .msg()
        .component()
        .click_event(ClickEvent::run_command(format!(
            "summon minecraft:mannequin ~ ~ ~ {{profile:{encoded_profile}}}"
        )));
    let copy_text = COMMANDS_FETCHPROFILE_COPY_TEXT
        .message([sprite.color(Color::White)])
        .component()
        .click_event(ClickEvent::copy_to_clipboard(encoded_component));

    let clickable = format_list(
        vec![copy_component, give_item, summon_mannequin, copy_text],
        TextComponent::plain(" "),
        |component| {
            TextComponent::plain("[")
                .add_child(component.color(Color::Green))
                .add_child(TextComponent::plain("]"))
        },
    );
    success.message([argument, clickable]).component()
}

/// Vanilla `PlayerSprite`: the profile wrapped in the `player` chat object.
fn player_sprite(profile: &ResolvableProfile) -> TextComponent {
    TextComponent::from(Object::Player {
        player: object_player(profile),
        hat: true,
        fallback: None,
    })
}

/// Vanilla `PlayerSprite`'s `player` field: the profile's serialized fields.
fn object_player(profile: &ResolvableProfile) -> ObjectPlayer {
    let mut player = ObjectPlayer {
        name: None,
        id: None,
        texture: None,
        cape: None,
        elytra: None,
        model: None,
        properties: Vec::new(),
    };
    match profile.contents() {
        ResolvableProfileContents::StaticFull(full) => {
            player.id = Some(full.id().to_int_array());
            player.name = Some(full.name().to_owned().into());
            player.properties = full.properties().iter().map(player_property).collect();
        }
        ResolvableProfileContents::StaticPartial(partial) => {
            player.name = partial.name().map(ToOwned::to_owned).map(Into::into);
            player.id = partial.id().map(|id| id.to_int_array());
            player.properties = partial.properties().iter().map(player_property).collect();
        }
        ResolvableProfileContents::DynamicName(name) => {
            player.name = Some(name.clone().into());
        }
        ResolvableProfileContents::DynamicId(id) => {
            player.id = Some(id.to_int_array());
        }
    }

    let patch = profile.skin_patch();
    player.texture = patch.texture().map(|id| id.to_string().into());
    player.cape = patch.cape().map(|id| id.to_string().into());
    player.elytra = patch.elytra().map(|id| id.to_string().into());
    player.model = patch.model().map(|model| match model {
        PlayerModelType::Slim => PlayerModel::Slim,
        PlayerModelType::Wide => PlayerModel::Wide,
    });
    player
}

fn player_property(property: &ProfileProperty) -> PlayerProperties {
    PlayerProperties {
        name: property.name().to_owned().into(),
        value: property.value().to_owned().into(),
        signature: property.signature().map(ToOwned::to_owned).map(Into::into),
    }
}

/// Vanilla `ComponentUtils.formatList`: `separator` between formatted entries.
fn format_list(
    entries: Vec<TextComponent>,
    separator: TextComponent,
    format: impl Fn(TextComponent) -> TextComponent,
) -> TextComponent {
    let mut list = TextComponent::plain("");
    for (index, entry) in entries.into_iter().enumerate() {
        if index > 0 {
            list = list.add_child(separator.clone());
        }
        list = list.add_child(format(entry));
    }
    list
}

/// A `/fetchprofile name|id` result produced by the asynchronous profile lookup.
struct FetchProfileSuspension {
    source: CommandSource,
    receiver: oneshot::Receiver<FetchProfileOutcome>,
    task: Option<JoinHandle<()>>,
}

impl FetchProfileSuspension {
    /// Runs `work` on the async runtime and reports its result on a later tick.
    fn spawn(
        source: CommandSource,
        work: impl Future<Output = FetchProfileOutcome> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = sender.send(work.await);
        });
        Self {
            source,
            receiver,
            task: Some(task),
        }
    }
}

impl CommandResultSuspension for FetchProfileSuspension {
    fn order(&self) -> CommandSuspensionOrder {
        CommandSuspensionOrder::Source
    }

    fn poll(&mut self) -> CommandResultSuspensionPoll {
        match self.receiver.try_recv() {
            Ok(outcome) => {
                self.task = None;
                match outcome {
                    Ok(message) => self.source.send_success(&message, false),
                    Err(message) => self.source.send_failure(message),
                }
                CommandResultSuspensionPoll::Ready(Ok(LOOKUP_COMMAND_RESULT))
            }
            Err(oneshot::error::TryRecvError::Empty) => CommandResultSuspensionPoll::Pending,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.task = None;
                CommandResultSuspensionPoll::Ready(Err(CommandSyntaxError::dynamic(
                    "fetchprofile task ended without a result",
                )))
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Vanilla's lookup returns 1 before the profile is known, and the failure branch
/// never changes that result.
const LOOKUP_COMMAND_RESULT: i32 = 1;

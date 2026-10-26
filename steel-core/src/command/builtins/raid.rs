//! Vanilla raid command.

use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::SoundSource;
use steel_registry::{sound_events, vanilla_entities};
use steel_utils::text::DisplayResolutor;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::entity::raid::Raid;
use crate::entity::{ENTITIES, Entity as _, EntitySpawnReason, next_entity_id};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::Player;

/// Vanilla `RaidCommand`'s "No raid found here" failure text.
const NO_RAID_FOUND: &str = "No raid found here";
/// Vanilla `RaidCommand`'s "No raid here" failure text.
const NO_RAID_HERE: &str = "No raid here";

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("raid"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("raid")
        .then(
            literal("start")
                .then(argument("omenlvl", ArgumentType::integer(0, i32::MAX)).executes(start_raid)),
        )
        .then(literal("stop").executes(stop_raid))
        .then(literal("check").executes(check_raid))
        .then(
            literal("sound")
                .then(argument("type", SteelArgumentType::component()).executes(play_raid_sound)),
        )
        .then(literal("spawnleader").executes(spawn_leader))
        .then(literal("setomen").then(
            argument("level", ArgumentType::integer(0, i32::MAX)).executes(set_raid_omen_level),
        ))
}

/// Vanilla `RaidCommand.start`.
fn start_raid(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let omen_level = context.integer("omenlvl")?;
    let player = source_player(context)?;
    let position = player.block_position();
    let world = context.source().world();

    if world.is_raided(position) {
        context
            .source()
            .send_failure(TextComponent::plain("Raid already started close by"));
        return Ok(-1);
    }

    let raid = world
        .raids()
        .lock()
        .create_or_extend_raid(world, player.as_ref(), position);
    let Some(raid) = raid else {
        context.source().send_failure(TextComponent::plain(
            "Failed to create a raid in your local village",
        ));
        return Ok(1);
    };

    if let Some(active) = world.raids().lock().get_mut(raid.id) {
        active.set_raid_omen_level(omen_level);
    }
    world.mark_raids_dirty();
    context.source().send_success(
        &TextComponent::plain("Created a raid in your local village"),
        false,
    );
    Ok(1)
}

/// Vanilla `RaidCommand.stop`.
fn stop_raid(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let player = source_player(context)?;
    let world = context.source().world();
    let Some(raid) = world.get_raid_at(player.block_position()) else {
        context
            .source()
            .send_failure(TextComponent::plain(NO_RAID_HERE));
        return Ok(-1);
    };

    if let Some(active) = world.raids().lock().get_mut(raid.id) {
        active.stop();
    }
    context
        .source()
        .send_success(&TextComponent::plain("Stopped raid"), false);
    Ok(1)
}

/// Vanilla `RaidCommand.check`.
fn check_raid(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let player = source_player(context)?;
    let world = context.source().world();
    let Some(raid) = world.get_raid_at(player.block_position()) else {
        context
            .source()
            .send_failure(TextComponent::plain("Found no started raids"));
        return Ok(0);
    };

    context
        .source()
        .send_success(&TextComponent::plain("Found a started raid!"), false);
    let (health, total_health) = {
        let raids = world.raids().lock();
        raids.get(raid.id).map_or((0.0, 0.0), |active| {
            (
                active.health_of_living_raiders(world),
                active.total_health(),
            )
        })
    };
    let status = format!(
        "Num groups spawned: {} Raid omen level: {} Num mobs: {} Raid health: {health} / {total_health}",
        raid.groups_spawned, raid.raid_omen_level, raid.total_raiders_alive
    );
    context
        .source()
        .send_success(&TextComponent::plain(status), false);
    Ok(1)
}

/// Vanilla `RaidCommand.sound`: only the literal `local` component plays the horn.
fn play_raid_sound(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let sound_type = context.text_component("type")?;
    if sound_type.to_plain(&DisplayResolutor) == "local" {
        let world = context.source().world();
        let pos = context.source().position() + DVec3::new(5.0, 0.0, 0.0);
        world.play_sound_at(
            &sound_events::EVENT_RAID_HORN,
            SoundSource::Neutral,
            pos,
            2.0,
            1.0,
            None,
        );
    }
    Ok(1)
}

/// Vanilla `RaidCommand.spawnleader`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn spawn_leader(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let world = source.world();
    let Some(entity) = ENTITIES.create(
        &vanilla_entities::PILLAGER,
        next_entity_id(),
        source.position(),
        Arc::downgrade(world),
    ) else {
        source.send_failure(TextComponent::plain("Pillager failed to spawn"));
        return Ok(0);
    };

    let Some(raider) = entity.as_raider() else {
        source.send_failure(TextComponent::plain("Pillager failed to spawn"));
        return Ok(0);
    };
    raider.set_patrol_leader(true);
    if let Some(living) = entity.as_living_entity() {
        living
            .living_base()
            .equipment()
            .lock()
            .set(EquipmentSlot::Head, Raid::ominous_banner_instance());
    }
    if let Some(mob) = entity.as_mob() {
        mob.finalize_spawn(world, EntitySpawnReason::Command, None);
    }

    if world.try_add_entity(Arc::clone(&entity)).is_err() {
        source.send_failure(TextComponent::plain("Pillager failed to spawn"));
        return Ok(0);
    }

    source.send_success(&TextComponent::plain("Spawned a raid captain"), false);
    Ok(1)
}

/// Vanilla `RaidCommand.setomen`.
fn set_raid_omen_level(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let level = context.integer("level")?;
    let player = source_player(context)?;
    let world = context.source().world();
    let Some(raid) = world.get_raid_at(player.block_position()) else {
        context
            .source()
            .send_failure(TextComponent::plain(NO_RAID_FOUND));
        return Ok(1);
    };

    let mut raids = world.raids().lock();
    let Some(active) = raids.get_mut(raid.id) else {
        drop(raids);
        context
            .source()
            .send_failure(TextComponent::plain(NO_RAID_FOUND));
        return Ok(1);
    };

    let max = active.max_raid_omen_level();
    if level > max {
        drop(raids);
        context.source().send_failure(TextComponent::plain(format!(
            "Sorry, the max raid omen level you can set is {max}"
        )));
        return Ok(1);
    }

    let before = active.raid_omen_level();
    active.set_raid_omen_level(level);
    drop(raids);
    world.mark_raids_dirty();
    context.source().send_success(
        &TextComponent::plain(format!(
            "Changed village's raid omen level from {before} to {level}"
        )),
        false,
    );
    Ok(1)
}

/// Vanilla `CommandSourceStack.getPlayerOrException`.
fn source_player(
    context: &SteelCommandContext<CommandSource>,
) -> Result<Arc<Player>, CommandSyntaxError> {
    context.source().player().cloned().ok_or_else(|| {
        CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_PLAYER,
        ))
    })
}

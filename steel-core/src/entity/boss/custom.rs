//! Vanilla `CustomBossEvents` + `CustomBossEvent`: persisted custom boss bars.
//!
//! Backs the vanilla `/bossbar` command. Vanilla keeps
//! `Map<Identifier, CustomBossEvent>` as `SavedData` (`"custom_boss_events"`)
//! and packs each bar into a `CustomBossEvent.Packed` record; Steel keeps the
//! same map and the same record fields, with the `SavedData.setDirty` hook
//! folded into the manager's dirty flag.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use steel_protocol::packets::game::{BossBarColor, BossBarOverlay, CBossEvent};
use steel_utils::Identifier;
use text_components::format::Color;
use text_components::interactivity::HoverEvent;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::event::ServerBossEvent;
use crate::entity::Entity as _;
use crate::player::Player;
use crate::server::Server;

/// Vanilla `SavedData` name for the custom boss-bar manager.
pub const CUSTOM_BOSS_EVENTS_NAME: &str = "custom_boss_events";

/// Per-tick flush of the server's custom boss bars.
///
/// Vanilla `ServerBossEvent` sends `ClientboundBossEventPacket`s while the `/bossbar` command
/// mutates a bar; Steel queues them and the game tick hands them over here, after commands ran, so
/// every `/bossbar add`/`set`/`remove` reaches the clients on the tick it was issued.
///
/// Vanilla equivalent: `MinecraftServer` driving `ServerBossEvent`'s own sends, plus
/// `CustomBossEvents.onPlayerConnect`/`onPlayerDisconnect`.
pub fn tick_custom_boss_events(server: &Server) {
    let online_players = server.get_players();
    server.custom_boss_events.lock().tick(&online_players);
}

/// Vanilla `CustomBossEvent.DEFAULT_MAX`.
pub const DEFAULT_MAX: i32 = 100;

/// Vanilla `CustomBossEvent`: a named boss bar with a `value`/`max` pair.
///
/// The bar renders `value / max`; `value` itself is stored unclamped, exactly
/// like vanilla (`setValue` only clamps the derived progress).
#[derive(Clone)]
pub struct CustomBossEvent {
    event: ServerBossEvent,
    custom_id: Identifier,
    value: i32,
    max: i32,
    players: BTreeSet<Uuid>,
}

impl CustomBossEvent {
    /// Vanilla `CustomBossEvent(UUID, Identifier, Component, Runnable)`.
    ///
    /// `value` starts at `0` and progress at `0.0`; `max` defaults to
    /// [`DEFAULT_MAX`].
    #[must_use]
    pub fn new(id: Uuid, custom_id: Identifier, name: TextComponent) -> Self {
        let mut event =
            ServerBossEvent::new(id, name, BossBarColor::White, BossBarOverlay::Progress);
        event.set_progress(0.0);
        Self {
            event,
            custom_id,
            value: 0,
            max: DEFAULT_MAX,
            players: BTreeSet::new(),
        }
    }

    /// Vanilla `CustomBossEvent.customId`.
    #[must_use]
    pub const fn custom_id(&self) -> &Identifier {
        &self.custom_id
    }

    /// Vanilla `CustomBossEvent.getName`.
    #[must_use]
    pub fn name(&self) -> &TextComponent {
        self.event.name()
    }

    /// Vanilla `CustomBossEvent.setName`.
    pub fn set_name(&mut self, name: TextComponent) {
        self.event.set_name(name);
    }

    /// Vanilla `CustomBossEvent.getDisplayName`: the bracketed, colored name
    /// with the custom id as hover text and insertion.
    #[must_use]
    pub fn display_name(&self) -> TextComponent {
        let custom_id = self.custom_id.to_string();
        let name = self
            .event
            .name()
            .clone()
            .color(boss_bar_text_color(self.event.color()))
            .hover_event(HoverEvent::show_text(custom_id.clone()))
            .insertion(custom_id);
        TextComponent::plain("[")
            .add_child(name)
            .add_child(TextComponent::plain("]"))
    }

    /// Vanilla `CustomBossEvent.value`.
    #[must_use]
    pub const fn value(&self) -> i32 {
        self.value
    }

    /// Vanilla `CustomBossEvent.setValue`: stores the value and derives the
    /// clamped progress.
    pub fn set_value(&mut self, value: i32) {
        self.value = value;
        self.sync_progress();
    }

    /// Vanilla `CustomBossEvent.max`.
    #[must_use]
    pub const fn max(&self) -> i32 {
        self.max
    }

    /// Vanilla `CustomBossEvent.setMax`: stores the max and derives the
    /// clamped progress.
    pub fn set_max(&mut self, max: i32) {
        self.max = max;
        self.sync_progress();
    }

    /// Vanilla `CustomBossEvent.getColor`.
    #[must_use]
    pub const fn color(&self) -> BossBarColor {
        self.event.color()
    }

    /// Vanilla `CustomBossEvent.setColor`.
    pub fn set_color(&mut self, color: BossBarColor) {
        self.event.set_color(color);
    }

    /// Vanilla `CustomBossEvent.getOverlay`.
    #[must_use]
    pub const fn overlay(&self) -> BossBarOverlay {
        self.event.overlay()
    }

    /// Vanilla `CustomBossEvent.setOverlay`.
    pub fn set_overlay(&mut self, overlay: BossBarOverlay) {
        self.event.set_overlay(overlay);
    }

    /// Vanilla `CustomBossEvent.isVisible`.
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        self.event.is_visible()
    }

    /// Vanilla `CustomBossEvent.setVisible`.
    pub fn set_visible(&mut self, visible: bool) {
        self.event.set_visible(visible);
    }

    /// Vanilla `CustomBossEvent.shouldDarkenScreen`.
    #[must_use]
    pub const fn should_darken_screen(&self) -> bool {
        self.event.should_darken_screen()
    }

    /// Vanilla `CustomBossEvent.setDarkenScreen`.
    pub fn set_darken_screen(&mut self, darken_screen: bool) {
        self.event.set_darken_screen(darken_screen);
    }

    /// Vanilla `CustomBossEvent.shouldPlayBossMusic`.
    #[must_use]
    pub const fn should_play_boss_music(&self) -> bool {
        self.event.should_play_boss_music()
    }

    /// Vanilla `CustomBossEvent.setPlayBossMusic`.
    pub fn set_play_boss_music(&mut self, play_boss_music: bool) {
        self.event.set_play_boss_music(play_boss_music);
    }

    /// Vanilla `CustomBossEvent.shouldCreateWorldFog`.
    #[must_use]
    pub const fn should_create_world_fog(&self) -> bool {
        self.event.should_create_world_fog()
    }

    /// Vanilla `CustomBossEvent.setCreateWorldFog`.
    pub fn set_create_world_fog(&mut self, create_world_fog: bool) {
        self.event.set_create_world_fog(create_world_fog);
    }

    /// Vanilla `CustomBossEvent.addPlayer`.
    pub fn add_player(&mut self, player: Arc<Player>) {
        let uuid = player.uuid();
        self.event.add_player(player);
        if self.players.insert(uuid) {
            self.event.set_dirty();
        }
    }

    /// Vanilla `CustomBossEvent.removePlayer`.
    pub fn remove_player(&mut self, uuid: &Uuid) -> bool {
        let tracked = self.players.remove(uuid);
        self.event.remove_player(uuid);
        if tracked {
            self.event.set_dirty();
        }
        tracked
    }

    /// Vanilla `CustomBossEvent.removeAllPlayers`.
    pub fn clear_players(&mut self) {
        self.event.clear_players();
        if !self.players.is_empty() {
            self.players.clear();
            self.event.set_dirty();
        }
    }

    /// The persisted player set (vanilla `CustomBossEvent.players`).
    #[must_use]
    pub const fn player_uuids(&self) -> &BTreeSet<Uuid> {
        &self.players
    }

    /// The live viewers currently tracking this bar.
    pub fn players(&self) -> impl Iterator<Item = &Arc<Player>> {
        self.event.players()
    }

    /// Vanilla `CustomBossEvent.setPlayers`: reconciles the tracked set,
    /// returning whether anything changed.
    pub fn set_players(&mut self, players: &[Arc<Player>]) -> bool {
        let wanted: BTreeSet<Uuid> = players.iter().map(|player| player.uuid()).collect();
        let to_remove: Vec<Uuid> = self
            .players
            .iter()
            .filter(|uuid| !wanted.contains(uuid))
            .copied()
            .collect();
        let to_add: Vec<Arc<Player>> = players
            .iter()
            .filter(|player| !self.players.contains(&player.uuid()))
            .map(Arc::clone)
            .collect();

        for uuid in &to_remove {
            self.remove_player(uuid);
        }
        let added = !to_add.is_empty();
        for player in to_add {
            self.add_player(player);
        }

        let changed = !to_remove.is_empty() || added;
        if changed {
            self.event.set_dirty();
        }
        changed
    }

    /// The shared presentation state backing this custom bar.
    #[must_use]
    pub const fn event(&self) -> &ServerBossEvent {
        &self.event
    }

    /// The mutable shared presentation state.
    pub fn event_mut(&mut self) -> &mut ServerBossEvent {
        &mut self.event
    }

    /// Vanilla `CustomBossEvent.onPlayerConnect` + `onPlayerDisconnect`, evaluated once per tick.
    ///
    /// Vanilla drives the pair from the connection lifecycle. Steel deliberately folds them into
    /// the per-tick flush so no join/disconnect hook has to reach into the saved-data manager; the
    /// persisted viewer set still only grows and shrinks through `/bossbar`.
    pub fn sync_live_players(&mut self, online_players: &[Arc<Player>]) {
        let offline: Vec<Uuid> = self
            .event
            .player_uuids()
            .copied()
            .filter(|uuid| !online_players.iter().any(|player| player.uuid() == *uuid))
            .collect();
        for uuid in offline {
            // Vanilla `onPlayerDisconnect` removes only the live handle, keeping the
            // persisted viewer entry that re-shows the bar on the next connect.
            self.event.remove_player(&uuid);
        }

        for player in online_players {
            if self.players.contains(&player.uuid()) {
                self.add_player(Arc::clone(player));
            }
        }
    }

    /// Drains queued wire packets for this bar.
    pub fn drain_packets(&mut self) -> Vec<CBossEvent> {
        self.event.drain_packets()
    }

    /// Delivers queued wire operations to this bar's viewers.
    pub fn broadcast_pending(&mut self) {
        self.event.broadcast_pending();
    }

    /// Vanilla `CustomBossEvent.setValue`/`setMax` progress derivation:
    /// `clamp(value / max, 0, 1)`.
    fn sync_progress(&mut self) {
        let progress = if self.max == 0 {
            // Vanilla divides by zero here and gets NaN; Steel clamps to the
            // empty bar, which is what the client renders for a non-finite bar.
            0.0
        } else {
            (self.value as f32 / self.max as f32).clamp(0.0, 1.0)
        };
        self.event.set_progress(progress);
        self.event.set_dirty();
    }

    /// Vanilla `CustomBossEvent.load`.
    #[must_use]
    pub fn from_record(id: Uuid, custom_id: Identifier, record: &CustomBossEventRecord) -> Self {
        let mut bar = Self::new(id, custom_id, record.name.clone());
        bar.event.set_visible(record.visible);
        bar.set_value(record.value);
        bar.set_max(record.max);
        bar.set_color(parse_color(&record.color));
        bar.set_overlay(parse_overlay(&record.overlay));
        bar.set_darken_screen(record.darken_screen);
        bar.set_play_boss_music(record.play_boss_music);
        bar.set_create_world_fog(record.create_world_fog);
        bar.players = record.players.clone();
        bar.event.mark_saved();
        bar
    }

    /// Vanilla `CustomBossEvent.pack`.
    #[must_use]
    pub fn to_record(&self) -> CustomBossEventRecord {
        CustomBossEventRecord {
            name: self.event.name().clone(),
            visible: self.event.is_visible(),
            value: self.value,
            max: self.max,
            color: boss_bar_color_name(self.event.color()).to_owned(),
            overlay: boss_bar_overlay_name(self.event.overlay()).to_owned(),
            darken_screen: self.event.should_darken_screen(),
            play_boss_music: self.event.should_play_boss_music(),
            create_world_fog: self.event.should_create_world_fog(),
            players: self.players.clone(),
        }
    }
}

/// Vanilla `CustomBossEvent.Packed`.
#[derive(Clone, Serialize, Deserialize)]
pub struct CustomBossEventRecord {
    /// `Name`.
    #[serde(rename = "Name")]
    pub name: TextComponent,
    /// `Visible` (default `false`).
    #[serde(rename = "Visible", default)]
    pub visible: bool,
    /// `Value` (default `0`).
    #[serde(rename = "Value", default)]
    pub value: i32,
    /// `Max` (default `100`).
    #[serde(rename = "Max", default = "default_max")]
    pub max: i32,
    /// `Color` (default `white`), stored by vanilla serialized name.
    #[serde(rename = "Color", default = "default_color")]
    pub color: String,
    /// `Overlay` (default `progress`), stored by vanilla serialized name.
    #[serde(rename = "Overlay", default = "default_overlay")]
    pub overlay: String,
    /// `DarkenScreen` (default `false`).
    #[serde(rename = "DarkenScreen", default)]
    pub darken_screen: bool,
    /// `PlayBossMusic` (default `false`).
    #[serde(rename = "PlayBossMusic", default)]
    pub play_boss_music: bool,
    /// `CreateWorldFog` (default `false`).
    #[serde(rename = "CreateWorldFog", default)]
    pub create_world_fog: bool,
    /// `Players` (default empty).
    #[serde(rename = "Players", default)]
    pub players: BTreeSet<Uuid>,
}

const fn default_max() -> i32 {
    DEFAULT_MAX
}

fn default_color() -> String {
    "white".to_owned()
}

fn default_overlay() -> String {
    "progress".to_owned()
}

/// Vanilla `BossBarColor` codec: parse the serialized name.
fn parse_color(name: &str) -> BossBarColor {
    match name {
        "pink" => BossBarColor::Pink,
        "blue" => BossBarColor::Blue,
        "red" => BossBarColor::Red,
        "green" => BossBarColor::Green,
        "yellow" => BossBarColor::Yellow,
        "purple" => BossBarColor::Purple,
        _ => BossBarColor::White,
    }
}

/// Vanilla `BossBarOverlay` codec: parse the serialized name.
fn parse_overlay(name: &str) -> BossBarOverlay {
    match name {
        "notched_6" => BossBarOverlay::Notched6,
        "notched_10" => BossBarOverlay::Notched10,
        "notched_12" => BossBarOverlay::Notched12,
        "notched_20" => BossBarOverlay::Notched20,
        _ => BossBarOverlay::Progress,
    }
}

/// Vanilla `BossBarColor.getSerializedName`.
fn boss_bar_color_name(color: BossBarColor) -> &'static str {
    match color {
        BossBarColor::Pink => "pink",
        BossBarColor::Blue => "blue",
        BossBarColor::Red => "red",
        BossBarColor::Green => "green",
        BossBarColor::Yellow => "yellow",
        BossBarColor::Purple => "purple",
        BossBarColor::White => "white",
    }
}

/// Vanilla `BossBarOverlay.getSerializedName`.
fn boss_bar_overlay_name(overlay: BossBarOverlay) -> &'static str {
    match overlay {
        BossBarOverlay::Progress => "progress",
        BossBarOverlay::Notched6 => "notched_6",
        BossBarOverlay::Notched10 => "notched_10",
        BossBarOverlay::Notched12 => "notched_12",
        BossBarOverlay::Notched20 => "notched_20",
    }
}

/// Vanilla `CustomBossEvents` (`SavedData` `"custom_boss_events"`).
#[derive(Clone)]
pub struct CustomBossEvents {
    events: BTreeMap<Identifier, CustomBossEvent>,
    ids: BTreeMap<Identifier, Uuid>,
    dirty: bool,
}

impl Default for CustomBossEvents {
    fn default() -> Self {
        Self {
            events: BTreeMap::new(),
            ids: BTreeMap::new(),
            dirty: false,
        }
    }
}

impl CustomBossEvents {
    /// Creates an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Vanilla `CustomBossEvents.create`: inserts a default bar, returning the
    /// existing bar when the id is already taken.
    pub fn create(&mut self, id: &Identifier) -> &mut CustomBossEvent {
        if !self.events.contains_key(id) {
            let uuid = Uuid::new_v4();
            let name = TextComponent::plain(id.to_string());
            self.ids.insert(id.clone(), uuid);
            self.events
                .insert(id.clone(), CustomBossEvent::new(uuid, id.clone(), name));
            self.dirty = true;
        }
        self.events.get_mut(id).expect("just inserted")
    }

    /// Vanilla `CustomBossEvents.remove`.
    pub fn remove(&mut self, id: &Identifier) -> Option<CustomBossEvent> {
        let removed = self.events.remove(id);
        if removed.is_some() {
            self.ids.remove(id);
            self.dirty = true;
        }
        removed
    }

    /// Immutable lookup.
    #[must_use]
    pub fn get(&self, id: &Identifier) -> Option<&CustomBossEvent> {
        self.events.get(id)
    }

    /// Mutable lookup; marks the manager dirty.
    pub fn get_mut(&mut self, id: &Identifier) -> Option<&mut CustomBossEvent> {
        if self.events.contains_key(id) {
            self.dirty = true;
        }
        self.events.get_mut(id)
    }

    /// Vanilla `CustomBossEvents.getIds`.
    #[must_use]
    pub fn ids(&self) -> Vec<Identifier> {
        self.events.keys().cloned().collect()
    }

    /// Vanilla `CustomBossEvents.getEvents`.
    pub fn events(&self) -> impl Iterator<Item = &CustomBossEvent> {
        self.events.values()
    }

    /// Whether the manager changed since the last [`Self::mark_saved`].
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Vanilla `SavedData.save` completion for the manager.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
        for event in self.events.values_mut() {
            event.event_mut().mark_saved();
        }
    }

    /// Vanilla per-tick custom-bar flush: reconciles every bar's live viewers with the online set
    /// and then delivers the queued `ClientboundBossEventPacket`s.
    pub fn tick(&mut self, online_players: &[Arc<Player>]) {
        for event in self.events.values_mut() {
            event.sync_live_players(online_players);
        }
        for event in self.events.values_mut() {
            event.broadcast_pending();
        }
    }

    /// Drains pending wire packets from every bar in id order.
    pub fn drain_packets(&mut self) -> Vec<CBossEvent> {
        let mut packets = Vec::new();
        for event in self.events.values_mut() {
            packets.extend(event.drain_packets());
        }
        packets
    }

    /// Vanilla per-tick custom-bar flush: `CustomBossEvent.setDirty` requests persistence, while
    /// the actual `ClientboundBossEventPacket` traffic is delivered here, once per tick, to the
    /// players each bar currently tracks.
    pub fn broadcast_pending(&mut self) {
        for event in self.events.values_mut() {
            event.broadcast_pending();
        }
    }

    /// Vanilla `CustomBossEvents` save form.
    #[must_use]
    pub fn to_saved(&self) -> CustomBossEventsData {
        CustomBossEventsData {
            bars: self
                .events
                .iter()
                .map(|(id, event)| CustomBossEventsEntry {
                    id: id.clone(),
                    uuid: self.ids.get(id).copied().unwrap_or_else(Uuid::nil),
                    record: event.to_record(),
                })
                .collect(),
        }
    }

    /// Vanilla `CustomBossEvents` load form.
    #[must_use]
    pub fn from_saved(data: CustomBossEventsData) -> Self {
        let mut manager = Self::new();
        for entry in data.bars {
            let event = CustomBossEvent::from_record(entry.uuid, entry.id.clone(), &entry.record);
            manager.ids.insert(entry.id.clone(), entry.uuid);
            manager.events.insert(entry.id, event);
        }
        manager
    }
}

/// Serializable form of [`CustomBossEvents`].
#[derive(Clone, Serialize, Deserialize)]
pub struct CustomBossEventsData {
    /// One entry per custom bar.
    #[serde(default)]
    pub bars: Vec<CustomBossEventsEntry>,
}

/// One persisted custom bar.
#[derive(Clone, Serialize, Deserialize)]
pub struct CustomBossEventsEntry {
    /// Command-facing bar id.
    pub id: Identifier,
    /// Stable wire id.
    pub uuid: Uuid,
    /// The persisted bar state.
    pub record: CustomBossEventRecord,
}

/// Vanilla `BossEvent.BossBarColor.getFormatting` for the display name.
fn boss_bar_text_color(color: BossBarColor) -> Color {
    match color {
        BossBarColor::Pink => Color::Red,
        BossBarColor::Blue => Color::Blue,
        BossBarColor::Red => Color::DarkRed,
        BossBarColor::Green => Color::Green,
        BossBarColor::Yellow => Color::Yellow,
        BossBarColor::Purple => Color::DarkBlue,
        BossBarColor::White => Color::White,
    }
}

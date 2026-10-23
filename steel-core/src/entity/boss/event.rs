//! Vanilla `BossEvent` / `ServerBossEvent`: shared boss-bar state.
//!
//! Steel keeps the vanilla split: [`BossEvent`] is the presentation state and
//! [`ServerBossEvent`] adds per-player visibility tracking. Vanilla broadcasts
//! `ClientboundBossEventPacket` from inside the setters; Steel records the same
//! packet in [`ServerBossEvent::broadcast_pending`], which the bar's owner drives
//! once per tick, so the methods need no world handle.

use std::collections::BTreeMap;
use std::sync::Arc;

use steel_protocol::packets::game::{BossBarColor, BossBarOverlay, BossEventOperation, CBossEvent};
use text_components::TextComponent;
use uuid::Uuid;

use crate::entity::Entity as _;
use crate::player::Player;

/// Vanilla `BossEvent`: the shared boss-bar presentation state.
#[derive(Debug, Clone, PartialEq)]
pub struct BossEvent {
    id: Uuid,
    name: TextComponent,
    progress: f32,
    color: BossBarColor,
    overlay: BossBarOverlay,
    darken_screen: bool,
    play_boss_music: bool,
    create_world_fog: bool,
}

impl BossEvent {
    /// Vanilla `BossEvent(UUID, Component, BossBarColor, BossBarOverlay)`.
    #[must_use]
    pub fn new(
        id: Uuid,
        name: TextComponent,
        color: BossBarColor,
        overlay: BossBarOverlay,
    ) -> Self {
        Self {
            id,
            name,
            progress: 1.0,
            color,
            overlay,
            darken_screen: false,
            play_boss_music: false,
            create_world_fog: false,
        }
    }

    /// Vanilla `BossEvent.getId`.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    /// Vanilla `BossEvent.getName`.
    #[must_use]
    pub fn name(&self) -> &TextComponent {
        &self.name
    }

    /// Vanilla `BossEvent.setName`.
    pub fn set_name(&mut self, name: TextComponent) {
        self.name = name;
    }

    /// Vanilla `BossEvent.getProgress`.
    #[must_use]
    pub const fn progress(&self) -> f32 {
        self.progress
    }

    /// Vanilla `BossEvent.setProgress`.
    pub fn set_progress(&mut self, progress: f32) {
        self.progress = progress;
    }

    /// Vanilla `BossEvent.getColor`.
    #[must_use]
    pub const fn color(&self) -> BossBarColor {
        self.color
    }

    /// Vanilla `BossEvent.setColor`.
    pub fn set_color(&mut self, color: BossBarColor) {
        self.color = color;
    }

    /// Vanilla `BossEvent.getOverlay`.
    #[must_use]
    pub const fn overlay(&self) -> BossBarOverlay {
        self.overlay
    }

    /// Vanilla `BossEvent.setOverlay`.
    pub fn set_overlay(&mut self, overlay: BossBarOverlay) {
        self.overlay = overlay;
    }

    /// Vanilla `BossEvent.shouldDarkenScreen`.
    #[must_use]
    pub const fn should_darken_screen(&self) -> bool {
        self.darken_screen
    }

    /// Vanilla `BossEvent.setDarkenScreen`.
    pub fn set_darken_screen(&mut self, darken_screen: bool) {
        self.darken_screen = darken_screen;
    }

    /// Vanilla `BossEvent.shouldPlayBossMusic`.
    #[must_use]
    pub const fn should_play_boss_music(&self) -> bool {
        self.play_boss_music
    }

    /// Vanilla `BossEvent.setPlayBossMusic`.
    pub fn set_play_boss_music(&mut self, play_boss_music: bool) {
        self.play_boss_music = play_boss_music;
    }

    /// Vanilla `BossEvent.shouldCreateWorldFog`.
    #[must_use]
    pub const fn should_create_world_fog(&self) -> bool {
        self.create_world_fog
    }

    /// Vanilla `BossEvent.setCreateWorldFog`.
    pub fn set_create_world_fog(&mut self, create_world_fog: bool) {
        self.create_world_fog = create_world_fog;
    }
}

/// One queued wire operation and the players vanilla sends it to.
///
/// Vanilla `ServerBossEvent` sends member transitions (`addPlayer`/`removePlayer`) straight to
/// that one player, while `setProgress` and friends go through `broadcast`, which targets every
/// currently tracked player. Recording the target here keeps both shapes until
/// [`ServerBossEvent::broadcast_pending`] runs.
struct PendingBossPacket {
    packet: CBossEvent,
    /// `None` is vanilla `broadcast`: every player tracked when the drain runs.
    receiver: Option<Arc<Player>>,
}

/// Vanilla `ServerBossEvent`: a [`BossEvent`] with visible-player tracking.
///
/// Vanilla sends to `ServerPlayer` handles directly; Steel tracks the same
/// `Arc<Player>` handles so broadcasting needs no world lookup (players hold a
/// `Weak<World>`, so this cannot form a reference cycle).
#[derive(Clone)]
pub struct ServerBossEvent {
    event: BossEvent,
    players: BTreeMap<Uuid, Arc<Player>>,
    visible: bool,
    /// Vanilla `SavedData.setDirty` hook used by persisted custom bars.
    dirty: bool,
    pending: Vec<PendingBossPacket>,
}

impl ServerBossEvent {
    /// Vanilla `ServerBossEvent(UUID, Component, BossBarColor, BossBarOverlay)`.
    #[must_use]
    pub fn new(
        id: Uuid,
        name: TextComponent,
        color: BossBarColor,
        overlay: BossBarOverlay,
    ) -> Self {
        Self {
            event: BossEvent::new(id, name, color, overlay),
            players: BTreeMap::new(),
            visible: true,
            dirty: false,
            pending: Vec::new(),
        }
    }

    /// Vanilla `BossEvent.getId`.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.event.id()
    }

    /// Vanilla `BossEvent.getName`.
    #[must_use]
    pub fn name(&self) -> &TextComponent {
        self.event.name()
    }

    /// Vanilla `BossEvent.getProgress`.
    #[must_use]
    pub const fn progress(&self) -> f32 {
        self.event.progress()
    }

    /// Vanilla `BossEvent.getColor`.
    #[must_use]
    pub const fn color(&self) -> BossBarColor {
        self.event.color()
    }

    /// Vanilla `BossEvent.getOverlay`.
    #[must_use]
    pub const fn overlay(&self) -> BossBarOverlay {
        self.event.overlay()
    }

    /// Vanilla `BossEvent.shouldDarkenScreen`.
    #[must_use]
    pub const fn should_darken_screen(&self) -> bool {
        self.event.should_darken_screen()
    }

    /// Vanilla `BossEvent.shouldPlayBossMusic`.
    #[must_use]
    pub const fn should_play_boss_music(&self) -> bool {
        self.event.should_play_boss_music()
    }

    /// Vanilla `BossEvent.shouldCreateWorldFog`.
    #[must_use]
    pub const fn should_create_world_fog(&self) -> bool {
        self.event.should_create_world_fog()
    }

    /// The underlying presentation state.
    #[must_use]
    pub const fn event(&self) -> &BossEvent {
        &self.event
    }

    /// Vanilla `ServerBossEvent.isVisible`.
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    /// Vanilla `ServerBossEvent.getPlayers`.
    pub fn players(&self) -> impl Iterator<Item = &Arc<Player>> {
        self.players.values()
    }

    /// The uuids currently tracked by this bar, in stable order.
    pub fn player_uuids(&self) -> impl Iterator<Item = &Uuid> {
        self.players.keys()
    }

    /// Number of tracked players.
    #[must_use]
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// Vanilla `SavedData.setDirty`.
    pub fn set_dirty(&mut self) {
        self.dirty = true;
    }

    /// Whether the bar changed since the last [`Self::mark_saved`].
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Vanilla `SavedData.save` completion.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Vanilla `ServerBossEvent.setProgress`.
    pub fn set_progress(&mut self, progress: f32) {
        if progress != self.event.progress() {
            self.event.set_progress(progress);
            self.set_dirty();
            self.queue(CBossEvent::update_progress(self.event.id(), progress));
        }
    }

    /// Vanilla `ServerBossEvent.setName`.
    pub fn set_name(&mut self, name: TextComponent) {
        if name != *self.event.name() {
            self.event.set_name(name.clone());
            self.set_dirty();
            self.queue(CBossEvent::update_name(self.event.id(), name));
        }
    }

    /// Vanilla `ServerBossEvent.setColor`.
    pub fn set_color(&mut self, color: BossBarColor) {
        if color != self.event.color() {
            self.event.set_color(color);
            self.set_dirty();
            self.queue_style();
        }
    }

    /// Vanilla `ServerBossEvent.setOverlay`.
    pub fn set_overlay(&mut self, overlay: BossBarOverlay) {
        if overlay != self.event.overlay() {
            self.event.set_overlay(overlay);
            self.set_dirty();
            self.queue_style();
        }
    }

    /// Vanilla `ServerBossEvent.setDarkenScreen`.
    pub fn set_darken_screen(&mut self, darken_screen: bool) {
        if darken_screen != self.event.should_darken_screen() {
            self.event.set_darken_screen(darken_screen);
            self.set_dirty();
            self.queue_properties();
        }
    }

    /// Vanilla `ServerBossEvent.setPlayBossMusic`.
    pub fn set_play_boss_music(&mut self, play_boss_music: bool) {
        if play_boss_music != self.event.should_play_boss_music() {
            self.event.set_play_boss_music(play_boss_music);
            self.set_dirty();
            self.queue_properties();
        }
    }

    /// Vanilla `ServerBossEvent.setCreateWorldFog`.
    pub fn set_create_world_fog(&mut self, create_world_fog: bool) {
        if create_world_fog != self.event.should_create_world_fog() {
            self.event.set_create_world_fog(create_world_fog);
            self.set_dirty();
            self.queue_properties();
        }
    }

    /// Vanilla `ServerBossEvent.addPlayer`.
    pub fn add_player(&mut self, player: Arc<Player>) {
        if self
            .players
            .insert(player.uuid(), Arc::clone(&player))
            .is_some()
        {
            return;
        }
        self.set_dirty();
        let packet = self.add_packet();
        self.queue_direct(player, packet);
    }

    /// Vanilla `ServerBossEvent.removePlayer`.
    pub fn remove_player(&mut self, uuid: &Uuid) -> bool {
        let Some(player) = self.players.remove(uuid) else {
            return false;
        };
        self.set_dirty();
        self.queue_direct(player, CBossEvent::remove(self.event.id()));
        true
    }

    /// Vanilla `ServerBossEvent.removeAllPlayers`.
    pub fn clear_players(&mut self) {
        let uuids: Vec<Uuid> = self.players.keys().copied().collect();
        for uuid in uuids {
            self.remove_player(&uuid);
        }
    }

    /// Vanilla `ServerBossEvent.setVisible`.
    pub fn set_visible(&mut self, visible: bool) {
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        self.set_dirty();
        // Vanilla sends one packet per tracked player and ignores the new
        // visibility flag for these, because it already flipped it.
        let packet = if visible {
            self.add_packet()
        } else {
            CBossEvent::remove(self.event.id())
        };
        let players: Vec<Arc<Player>> = self.players.values().cloned().collect();
        for player in players {
            self.pending.push(PendingBossPacket {
                packet: packet.clone(),
                receiver: Some(player),
            });
        }
    }

    /// Delivers every queued operation to the players vanilla would have sent it to.
    ///
    /// Vanilla `ServerBossEvent` sends from inside its setters; Steel records the same operations
    /// and hands them to the owner-provided per-tick drain so the bar needs no world handle.
    pub fn broadcast_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for PendingBossPacket { packet, receiver } in pending {
            match receiver {
                Some(player) => player.send_packet(packet),
                None => {
                    for player in self.players.values() {
                        player.send_packet(packet.clone());
                    }
                }
            }
        }
    }

    /// Drains the packets recorded since the last call without delivering them.
    ///
    /// Prefer [`Self::broadcast_pending`]: this form loses the per-player targeting of
    /// `addPlayer`/`removePlayer` transitions and is only for callers that send the operations
    /// themselves.
    pub fn drain_packets(&mut self) -> Vec<CBossEvent> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .map(|pending| pending.packet)
            .collect()
    }

    /// Whether any packet is waiting to be drained.
    #[must_use]
    pub fn has_pending_packets(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Vanilla `ClientboundBossEventPacket.createAddPacket`.
    fn add_packet(&self) -> CBossEvent {
        CBossEvent::new(
            self.event.id(),
            BossEventOperation::Add {
                name: self.event.name().clone(),
                progress: self.event.progress(),
                color: self.event.color(),
                overlay: self.event.overlay(),
                darken_screen: self.event.should_darken_screen(),
                play_music: self.event.should_play_boss_music(),
                create_world_fog: self.event.should_create_world_fog(),
            },
        )
    }

    fn queue_style(&mut self) {
        let packet =
            CBossEvent::update_style(self.event.id(), self.event.color(), self.event.overlay());
        self.queue_broadcast(packet);
    }

    fn queue_properties(&mut self) {
        let packet = CBossEvent::update_properties(
            self.event.id(),
            self.event.should_darken_screen(),
            self.event.should_play_boss_music(),
            self.event.should_create_world_fog(),
        );
        self.queue_broadcast(packet);
    }

    /// Vanilla `ServerBossEvent.broadcast`: operations are only recorded while the
    /// bar is visible, and target every player tracked when the drain runs.
    fn queue_broadcast(&mut self, packet: CBossEvent) {
        if self.visible {
            self.pending.push(PendingBossPacket {
                packet,
                receiver: None,
            });
        }
    }

    /// Vanilla `ServerBossEvent.addPlayer`/`removePlayer`: member transitions reach one player
    /// only, and are also suppressed while the bar is invisible.
    fn queue_direct(&mut self, player: Arc<Player>, packet: CBossEvent) {
        if self.visible {
            self.pending.push(PendingBossPacket {
                packet,
                receiver: Some(player),
            });
        }
    }
}

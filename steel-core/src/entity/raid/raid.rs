//! Vanilla `Raid`: wave scheduling, raid-omen absorption, spawn placement and
//! victory/defeat resolution.
//!
//! Vanilla pushes client-visible state into a `ServerBossEvent`. Steel has no boss bar
//! foundation yet (no `ClientboundBossEventPacket` and no boss bar manager), so the same
//! state is kept in [`RaidBar`] and exposed through [`Raid::bar`]; the transport is a
//! reported gap rather than a stub.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::DVec3;
use steel_registry::DyeColor;
use steel_registry::RegistryHolder;
use steel_registry::data_components::components::{Rarity, TooltipDisplay};
use steel_registry::data_components::vanilla_components::{
    BANNER_PATTERNS, ITEM_NAME, RARITY, TOOLTIP_DISPLAY,
};
use steel_registry::data_components::{BannerPatternLayer, BannerPatternLayers};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_banner_patterns;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_items;
use steel_registry::vanilla_mob_effects;
use steel_utils::random::Random;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, SectionPos};
use text_components::translation::TranslatedMessage;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::entity::spawn::create_entity_instance;
use crate::entity::{
    Entity, EntitySpawnReason, LivingEntity, MobEffectInstance, Player, SharedEntity,
};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;

use crate::world::World;
use crate::world::village::{find_section_closest_to_village, poi::is_village_poi};

use super::raids::RaidSnapshot;

/// Vanilla `Raid.ALLOW_SPAWNING_WITHIN_VILLAGE_SECONDS_THRESHOLD`.
const ALLOW_SPAWNING_WITHIN_VILLAGE_SECONDS_THRESHOLD: i32 = 7;
/// Vanilla `Raid.SECTION_RADIUS_FOR_FINDING_NEW_VILLAGE_CENTER`.
const SECTION_RADIUS_FOR_FINDING_NEW_VILLAGE_CENTER: i32 = 2;
/// Vanilla `Raid.VILLAGE_SEARCH_RADIUS`.
pub const VILLAGE_SEARCH_RADIUS: i32 = 32;
/// Vanilla `Raid.RAID_TIMEOUT_TICKS`.
const RAID_TIMEOUT_TICKS: i64 = 48_000;
/// Vanilla `Raid.NUM_SPAWN_ATTEMPTS`.
const NUM_SPAWN_ATTEMPTS: i32 = 5;
/// Vanilla `Raid.POST_RAID_TICK_LIMIT`.
const POST_RAID_TICK_LIMIT: i32 = 40;
/// Vanilla `Raid.MAX_CELEBRATION_TICKS`.
pub const MAX_CELEBRATION_TICKS: i32 = 600;
/// Vanilla `Raid.OUTSIDE_RAID_BOUNDS_TIMEOUT`.
const OUTSIDE_RAID_BOUNDS_TIMEOUT: i32 = 30;
/// Vanilla `Raid.DEFAULT_MAX_RAID_OMEN_LEVEL`.
pub const DEFAULT_MAX_RAID_OMEN_LEVEL: i32 = 5;
/// Vanilla `Raid.HERO_OF_THE_VILLAGE_DURATION`.
const HERO_OF_THE_VILLAGE_DURATION: i32 = 48_000;
/// Vanilla `Raid.VALID_RAID_RADIUS_SQR`: 96 blocks.
pub const VALID_RAID_RADIUS_SQR: i32 = 9216;
/// Vanilla `Raid.RAID_REMOVAL_THRESHOLD_SQR`: 112 blocks.
pub const RAID_REMOVAL_THRESHOLD_SQR: i32 = 12_544;
/// Vanilla `Raid.VILLAGE_RADIUS_BUFFER`.
pub const VILLAGE_RADIUS_BUFFER: i32 = 16;
/// Vanilla `Raid.MAX_NO_ACTION_TIME`.
pub const MAX_NO_ACTION_TIME: i32 = 2400;
/// Vanilla `Raid.DEFAULT_PRE_RAID_TICKS`.
pub const DEFAULT_PRE_RAID_TICKS: i32 = 300;
/// Vanilla `Raid.LOW_MOB_THRESHOLD`.
const LOW_MOB_THRESHOLD: i32 = 2;

/// Vanilla `Raid.RaidStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RaidStatus {
    #[default]
    /// The raid is still running.
    Ongoing,
    /// The raid ended with the village defended.
    Victory,
    /// The raid ended with the village overrun.
    Loss,
    /// The raid was stopped before either outcome.
    Stopped,
}

impl RaidStatus {
    /// Vanilla `RaidStatus` declaration order, which is also codec order.
    pub const VALUES: [Self; 4] = [Self::Ongoing, Self::Victory, Self::Loss, Self::Stopped];

    /// Vanilla `RaidStatus.getSerializedName`.
    #[must_use]
    pub const fn serialized_name(self) -> &'static str {
        match self {
            Self::Ongoing => "ongoing",
            Self::Victory => "victory",
            Self::Loss => "loss",
            Self::Stopped => "stopped",
        }
    }

    /// Parses a saved status name.
    #[must_use]
    pub fn from_serialized_name(name: &str) -> Option<Self> {
        Self::VALUES
            .into_iter()
            .find(|status| status.serialized_name() == name)
    }
}

/// Vanilla `Raid.RaiderType`: the per-wave raider spawn table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RaiderType {
    /// Vindicator raiders.
    Vindicator,
    /// Evoker raiders.
    Evoker,
    /// Pillager raiders.
    Pillager,
    /// Witch raiders.
    Witch,
    /// Ravager raiders.
    Ravager,
}

impl RaiderType {
    /// Vanilla `Raid.RaiderType.VALUES`.
    pub const VALUES: [Self; 5] = [
        Self::Vindicator,
        Self::Evoker,
        Self::Pillager,
        Self::Witch,
        Self::Ravager,
    ];

    /// The entity type this row spawns.
    #[must_use]
    pub fn entity_type(self) -> EntityTypeRef {
        match self {
            Self::Vindicator => &vanilla_entities::VINDICATOR,
            Self::Evoker => &vanilla_entities::EVOKER,
            Self::Pillager => &vanilla_entities::PILLAGER,
            Self::Witch => &vanilla_entities::WITCH,
            Self::Ravager => &vanilla_entities::RAVAGER,
        }
    }

    /// Vanilla `RaiderType.spawnsPerWaveBeforeBonus`, indexed by wave; the bonus
    /// wave reads the difficulty group count instead.
    #[must_use]
    pub const fn spawns_per_wave_before_bonus(self) -> [i32; 8] {
        match self {
            Self::Vindicator => [0, 0, 2, 0, 1, 4, 2, 5],
            Self::Evoker => [0, 0, 0, 0, 0, 1, 1, 2],
            Self::Pillager => [0, 4, 3, 3, 4, 4, 4, 2],
            Self::Witch => [0, 0, 0, 0, 3, 0, 0, 1],
            Self::Ravager => [0, 0, 0, 1, 0, 1, 0, 2],
        }
    }
}

/// Vanilla `Raid.raidEvent` (`ServerBossEvent`) state.
#[derive(Debug, Clone, PartialEq)]
pub struct RaidBar {
    /// Vanilla `BossEvent.getProgress`.
    pub progress: f32,
    /// Vanilla `BossEvent.getName`.
    pub name: TextComponent,
    /// Vanilla `BossEvent.isVisible`.
    pub visible: bool,
    /// Vanilla `BossEvent.getPlayers`.
    pub players: BTreeSet<Uuid>,
}

/// Vanilla `Raid`.
/// Vanilla `Raid`.
#[expect(
    missing_debug_implementations,
    reason = "Raid holds a LegacyRandom whose Debug impl is not needed by its users"
)]
pub struct Raid {
    group_to_leader_map: BTreeMap<i32, Uuid>,
    group_raider_map: BTreeMap<i32, BTreeSet<Uuid>>,
    heroes_of_the_village: BTreeSet<Uuid>,
    ticks_active: i64,
    center: BlockPos,
    started: bool,
    total_health: f32,
    raid_omen_level: i32,
    active: bool,
    groups_spawned: i32,
    post_raid_ticks: i32,
    raid_cooldown_ticks: i32,
    random: LegacyRandom,
    num_groups: i32,
    status: RaidStatus,
    celebration_ticks: i32,
    wave_spawn_pos: Option<BlockPos>,
    bar: RaidBar,
}

impl Raid {
    /// Vanilla `Raid(BlockPos, Difficulty)`.
    #[must_use]
    pub fn new(center: BlockPos, difficulty: Difficulty) -> Self {
        Self {
            group_to_leader_map: BTreeMap::new(),
            group_raider_map: BTreeMap::new(),
            heroes_of_the_village: BTreeSet::new(),
            ticks_active: 0,
            center,
            started: false,
            total_health: 0.0,
            raid_omen_level: 0,
            active: true,
            groups_spawned: 0,
            post_raid_ticks: 0,
            raid_cooldown_ticks: DEFAULT_PRE_RAID_TICKS,
            random: LegacyRandom::from_seed(rand::random()),
            num_groups: Self::num_groups(difficulty),
            status: RaidStatus::Ongoing,
            celebration_ticks: 0,
            wave_spawn_pos: None,
            bar: RaidBar {
                progress: 0.0,
                name: raid_name_component(),
                visible: true,
                players: BTreeSet::new(),
            },
        }
    }

    /// Rebuilds a raid from saved data, matching vanilla's record constructor.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the vanilla saved-data fields one to one"
    )]
    pub fn from_saved(
        started: bool,
        active: bool,
        ticks_active: i64,
        raid_omen_level: i32,
        groups_spawned: i32,
        raid_cooldown_ticks: i32,
        post_raid_ticks: i32,
        total_health: f32,
        num_groups: i32,
        status: RaidStatus,
        center: BlockPos,
        heroes_of_the_village: BTreeSet<Uuid>,
    ) -> Self {
        Self {
            group_to_leader_map: BTreeMap::new(),
            group_raider_map: BTreeMap::new(),
            heroes_of_the_village,
            ticks_active,
            center,
            started,
            total_health,
            raid_omen_level,
            active,
            groups_spawned,
            post_raid_ticks,
            raid_cooldown_ticks,
            random: LegacyRandom::from_seed(rand::random()),
            num_groups,
            status,
            celebration_ticks: 0,
            wave_spawn_pos: None,
            bar: RaidBar {
                progress: 0.0,
                name: raid_name_component(),
                visible: active,
                players: BTreeSet::new(),
            },
        }
    }

    /// Vanilla `Raid.getCenter`.
    #[must_use]
    pub const fn center(&self) -> BlockPos {
        self.center
    }

    /// Vanilla `Raid.setCenter`.
    pub fn set_center(&mut self, center: BlockPos) {
        self.center = center;
    }

    /// Vanilla `Raid.getNumGroups(Difficulty)`.
    #[must_use]
    pub const fn num_groups(difficulty: Difficulty) -> i32 {
        match difficulty {
            Difficulty::Peaceful => 0,
            Difficulty::Easy => 3,
            Difficulty::Normal => 5,
            Difficulty::Hard => 7,
        }
    }

    /// This raid's stored group count.
    #[must_use]
    pub const fn group_count(&self) -> i32 {
        self.num_groups
    }

    /// Vanilla `Raid.getGroupsSpawned`.
    #[must_use]
    pub const fn groups_spawned(&self) -> i32 {
        self.groups_spawned
    }

    /// Vanilla `Raid.getTicksActive`.
    #[must_use]
    pub const fn ticks_active(&self) -> i64 {
        self.ticks_active
    }

    /// Vanilla `Raid.getRaidOmenLevel`.
    #[must_use]
    pub const fn raid_omen_level(&self) -> i32 {
        self.raid_omen_level
    }

    /// Vanilla `Raid.setRaidOmenLevel`.
    pub fn set_raid_omen_level(&mut self, raid_omen_level: i32) {
        self.raid_omen_level = raid_omen_level;
    }

    /// Vanilla `Raid.getMaxRaidOmenLevel`.
    #[must_use]
    pub const fn max_raid_omen_level(&self) -> i32 {
        DEFAULT_MAX_RAID_OMEN_LEVEL
    }

    /// Vanilla `Raid.isActive`.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Vanilla `Raid.isStarted`.
    #[must_use]
    pub const fn is_started(&self) -> bool {
        self.started
    }

    /// Vanilla `Raid.isStopped`.
    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        matches!(self.status, RaidStatus::Stopped)
    }

    /// Vanilla `Raid.isVictory`.
    #[must_use]
    pub const fn is_victory(&self) -> bool {
        matches!(self.status, RaidStatus::Victory)
    }

    /// Vanilla `Raid.isLoss`.
    #[must_use]
    pub const fn is_loss(&self) -> bool {
        matches!(self.status, RaidStatus::Loss)
    }

    /// Vanilla `Raid.isOver`.
    #[must_use]
    pub const fn is_over(&self) -> bool {
        self.is_victory() || self.is_loss()
    }

    /// Vanilla `Raid.getStatus`.
    #[must_use]
    pub const fn status(&self) -> RaidStatus {
        self.status
    }

    /// Vanilla `Raid.hasFirstWaveSpawned`.
    #[must_use]
    pub const fn has_first_wave_spawned(&self) -> bool {
        self.groups_spawned > 0
    }

    /// Vanilla `Raid.isBetweenWaves`.
    #[must_use]
    pub fn is_between_waves(&self) -> bool {
        self.has_first_wave_spawned()
            && self.total_raiders_alive() == 0
            && self.raid_cooldown_ticks > 0
    }

    /// Vanilla `Raid.getTotalHealth`.
    #[must_use]
    pub const fn total_health(&self) -> f32 {
        self.total_health
    }

    /// Vanilla `Raid.getTotalRaidersAlive`.
    #[must_use]
    pub fn total_raiders_alive(&self) -> i32 {
        i32::try_from(
            self.group_raider_map
                .values()
                .map(BTreeSet::len)
                .sum::<usize>(),
        )
        .unwrap_or(i32::MAX)
    }

    /// Vanilla `Raid.getHealthOfLivingRaiders`.
    #[must_use]
    pub fn health_of_living_raiders(&self, world: &World) -> f32 {
        let mut health = 0.0;
        for raiders in self.group_raider_map.values() {
            for uuid in raiders {
                if let Some(raider) = world.get_entity_by_uuid(uuid)
                    && let Some(living) = raider.as_living_entity()
                {
                    health += living.get_health();
                }
            }
        }
        health
    }

    /// Vanilla `Raid.getEnchantOdds`.
    #[must_use]
    pub fn enchant_odds(&self) -> f32 {
        match self.raid_omen_level {
            2 => 0.1,
            3 => 0.25,
            4 => 0.5,
            5 => 0.75,
            _ => 0.0,
        }
    }

    /// Vanilla `Raid.getOminousBannerInstance`.
    #[must_use]
    pub fn ominous_banner_instance() -> ItemStack {
        let layer = |pattern: &'static steel_registry::banner_pattern::BannerPattern,
                     color: DyeColor| {
            BannerPatternLayer::new(RegistryHolder::reference(pattern), color)
        };
        let patterns = BannerPatternLayers::new(vec![
            layer(&vanilla_banner_patterns::RHOMBUS, DyeColor::Cyan),
            layer(&vanilla_banner_patterns::STRIPE_BOTTOM, DyeColor::LightGray),
            layer(&vanilla_banner_patterns::STRIPE_CENTER, DyeColor::Gray),
            layer(&vanilla_banner_patterns::BORDER, DyeColor::LightGray),
            layer(&vanilla_banner_patterns::STRIPE_MIDDLE, DyeColor::Black),
            layer(
                &vanilla_banner_patterns::HALF_HORIZONTAL,
                DyeColor::LightGray,
            ),
            layer(&vanilla_banner_patterns::CIRCLE, DyeColor::LightGray),
            layer(&vanilla_banner_patterns::BORDER, DyeColor::Black),
        ]);

        let mut stack = ItemStack::new(&vanilla_items::WHITE_BANNER);
        stack.set(BANNER_PATTERNS, patterns);
        stack.set(
            TOOLTIP_DISPLAY,
            TooltipDisplay::DEFAULT.with_hidden(BANNER_PATTERNS, true),
        );
        stack.set(
            ITEM_NAME,
            TextComponent::translated(TranslatedMessage::new(
                "block.minecraft.ominous_banner",
                None,
            )),
        );
        stack.set(RARITY, Rarity::Uncommon);
        stack
    }

    /// Vanilla `Raid.raidCooldownTicks`.
    #[must_use]
    pub const fn raid_cooldown_ticks(&self) -> i32 {
        self.raid_cooldown_ticks
    }

    /// Vanilla `Raid.postRaidTicks`.
    #[must_use]
    pub const fn post_raid_ticks(&self) -> i32 {
        self.post_raid_ticks
    }

    /// Vanilla `Raid.heroesOfTheVillage`.
    #[must_use]
    pub fn heroes_of_the_village(&self) -> Vec<Uuid> {
        self.heroes_of_the_village.iter().copied().collect()
    }

    /// Vanilla `Raid.getLeader`.
    #[must_use]
    pub fn get_leader(&self, wave: i32) -> Option<Uuid> {
        self.group_to_leader_map.get(&wave).copied()
    }

    /// Whether the wave already has a leader (`Raider` banner pickup check).
    #[must_use]
    pub fn has_leader(&self, wave: i32) -> bool {
        self.group_to_leader_map.contains_key(&wave)
    }

    /// Vanilla `Raid.getBossBar` state.
    #[must_use]
    pub const fn bar(&self) -> &RaidBar {
        &self.bar
    }

    /// Vanilla `Raid.hasBonusWave`.
    #[must_use]
    pub const fn has_bonus_wave(&self) -> bool {
        self.raid_omen_level > 1
    }

    /// Vanilla `Raid.hasSpawnedBonusWave`.
    #[must_use]
    pub const fn has_spawned_bonus_wave(&self) -> bool {
        self.groups_spawned > self.num_groups
    }

    /// Vanilla `Raid.shouldSpawnBonusGroup`.
    #[must_use]
    pub fn should_spawn_bonus_group(&self) -> bool {
        self.is_final_wave() && self.total_raiders_alive() == 0 && self.has_bonus_wave()
    }

    /// Vanilla `Raid.isFinalWave`.
    #[must_use]
    pub const fn is_final_wave(&self) -> bool {
        self.groups_spawned == self.num_groups
    }

    /// Vanilla `Raid.hasMoreWaves`.
    #[must_use]
    pub fn has_more_waves(&self) -> bool {
        if self.has_bonus_wave() {
            !self.has_spawned_bonus_wave()
        } else {
            !self.is_final_wave()
        }
    }

    /// Vanilla `Raid.shouldSpawnGroup`.
    #[must_use]
    pub fn should_spawn_group(&self) -> bool {
        self.raid_cooldown_ticks == 0
            && (self.groups_spawned < self.num_groups || self.should_spawn_bonus_group())
            && self.total_raiders_alive() == 0
    }

    /// Vanilla `Raid.getDefaultNumSpawns`.
    #[must_use]
    pub fn get_default_num_spawns(
        &self,
        raider_type: RaiderType,
        wave: i32,
        is_bonus_wave: bool,
    ) -> i32 {
        let table = raider_type.spawns_per_wave_before_bonus();
        let index = if is_bonus_wave { self.num_groups } else { wave };
        usize::try_from(index)
            .ok()
            .and_then(|index| table.get(index).copied())
            .unwrap_or(0)
    }

    /// Vanilla `Raid.getPotentialBonusSpawns`.
    pub fn get_potential_bonus_spawns(
        &mut self,
        raider_type: RaiderType,
        wave: i32,
        difficulty: Difficulty,
        is_bonus_wave: bool,
    ) -> i32 {
        let is_easy = difficulty == Difficulty::Easy;
        let is_normal = difficulty == Difficulty::Normal;

        let bonus_spawns = match raider_type {
            RaiderType::Vindicator | RaiderType::Pillager => {
                if is_easy {
                    self.random.next_i32_bounded(2)
                } else if is_normal {
                    1
                } else {
                    2
                }
            }
            RaiderType::Evoker => return 0,
            RaiderType::Witch => {
                if is_easy || wave <= 2 || wave == 4 {
                    return 0;
                }
                1
            }
            RaiderType::Ravager => {
                if !is_easy && is_bonus_wave {
                    1
                } else {
                    0
                }
            }
        };

        if bonus_spawns > 0 {
            self.random.next_i32_bounded(bonus_spawns + 1)
        } else {
            0
        }
    }

    /// Total raiders one raider type contributes to one wave, before entity creation.
    #[must_use]
    pub fn wave_raider_count(
        &mut self,
        raider_type: RaiderType,
        wave: i32,
        difficulty: Difficulty,
        is_bonus_wave: bool,
    ) -> i32 {
        let base = self.get_default_num_spawns(raider_type, wave, is_bonus_wave);
        base + self.get_potential_bonus_spawns(raider_type, wave, difficulty, is_bonus_wave)
    }

    /// Vanilla `Raid.absorbRaidOmen`.
    pub fn absorb_raid_omen(&mut self, player: &Player) -> bool {
        let Some(effect) = player.mob_effect(vanilla_mob_effects::RAID_OMEN) else {
            return false;
        };

        self.raid_omen_level =
            (self.raid_omen_level + effect.amplifier() + 1).clamp(0, self.max_raid_omen_level());

        if !self.has_first_wave_spawned() {
            player.award_custom_stat(&steel_registry::vanilla_custom_stats::RAID_TRIGGER);
        }
        true
    }

    /// Vanilla `Raid.addHeroOfTheVillage`.
    pub fn add_hero_of_the_village(&mut self, killer: Uuid) {
        self.heroes_of_the_village.insert(killer);
    }

    /// Vanilla `Raid.stop`.
    pub fn stop(&mut self) {
        self.active = false;
        self.bar.players.clear();
        self.bar.visible = false;
        self.status = RaidStatus::Stopped;
    }

    /// Vanilla `Raid.snapshot` for read-only consumers.
    #[must_use]
    pub fn snapshot(&self, id: i32) -> RaidSnapshot {
        RaidSnapshot {
            id,
            center: self.center,
            status: self.status,
            active: self.active,
            started: self.started,
            groups_spawned: self.groups_spawned,
            num_groups: self.num_groups,
            raid_omen_level: self.raid_omen_level,
            total_raiders_alive: self.total_raiders_alive(),
            raid_cooldown_ticks: self.raid_cooldown_ticks,
            enchant_odds: self.enchant_odds(),
        }
    }

    /// Vanilla `Raid.tick`.
    pub fn tick(&mut self, world: &Arc<World>, raid_id: i32) {
        if self.is_stopped() {
            return;
        }

        if matches!(self.status, RaidStatus::Ongoing) {
            self.tick_ongoing(world, raid_id);
        } else if self.is_over() {
            self.celebration_ticks += 1;
            if self.celebration_ticks >= MAX_CELEBRATION_TICKS {
                self.stop();
                return;
            }

            if self.celebration_ticks % 20 == 0 {
                self.update_players(world);
                self.bar.visible = true;
                if self.is_victory() {
                    self.bar.progress = 0.0;
                    self.bar.name = victory_bar_component();
                } else {
                    self.bar.name = defeat_bar_component();
                }
            }
        }
    }

    fn tick_ongoing(&mut self, world: &Arc<World>, raid_id: i32) {
        let old_active = self.active;
        self.active = world.is_full_chunk_loaded_at(self.center);

        if world.difficulty() == Difficulty::Peaceful {
            self.stop();
            return;
        }

        if old_active != self.active {
            self.bar.visible = self.active;
        }

        if !self.active {
            return;
        }

        if !world.is_village(self.center) {
            self.move_raid_center_to_nearby_village(world);
        }

        if !world.is_village(self.center) {
            if self.groups_spawned > 0 {
                self.status = RaidStatus::Loss;
            } else {
                self.stop();
            }
        }

        self.ticks_active += 1;
        if self.ticks_active >= RAID_TIMEOUT_TICKS {
            self.stop();
            return;
        }

        let raiders_alive = self.total_raiders_alive();
        if raiders_alive == 0 && self.has_more_waves() {
            if self.raid_cooldown_ticks <= 0 {
                if self.raid_cooldown_ticks == 0 && self.groups_spawned > 0 {
                    self.raid_cooldown_ticks = DEFAULT_PRE_RAID_TICKS;
                    self.bar.name = raid_name_component();
                    return;
                }
            } else {
                let has_cached_wave_spawn_pos = self.wave_spawn_pos.is_some();
                let mut should_try_to_find_spawn_pos =
                    !has_cached_wave_spawn_pos && self.raid_cooldown_ticks % 5 == 0;

                if let Some(spawn_pos) = self.wave_spawn_pos
                    && !world.is_position_entity_ticking(spawn_pos)
                {
                    should_try_to_find_spawn_pos = true;
                }

                if should_try_to_find_spawn_pos {
                    self.wave_spawn_pos = self.get_valid_spawn_pos(world);
                }

                if self.raid_cooldown_ticks == DEFAULT_PRE_RAID_TICKS
                    || self.raid_cooldown_ticks % 20 == 0
                {
                    self.update_players(world);
                }

                self.raid_cooldown_ticks -= 1;
                self.bar.progress = ((DEFAULT_PRE_RAID_TICKS - self.raid_cooldown_ticks) as f32
                    / DEFAULT_PRE_RAID_TICKS as f32)
                    .clamp(0.0, 1.0);
            }
        }

        if self.ticks_active % 20 == 0 {
            self.update_players(world);
            self.update_raiders(world);
            if raiders_alive > 0 && raiders_alive <= LOW_MOB_THRESHOLD {
                self.bar.name = low_raider_count_component(raiders_alive);
            } else {
                self.bar.name = raid_name_component();
            }
        }

        let mut sound_played = false;
        let mut attempt = 0;

        while self.should_spawn_group() {
            let spawn_pos = self
                .wave_spawn_pos
                .or_else(|| self.find_random_spawn_pos(world, 20));
            let Some(spawn_pos) = spawn_pos else {
                attempt += 1;
                if attempt > NUM_SPAWN_ATTEMPTS {
                    self.stop();
                    break;
                }
                continue;
            };

            self.started = true;
            self.spawn_group(world, raid_id, spawn_pos);
            if !sound_played {
                self.play_sound(world, spawn_pos);
                sound_played = true;
            }
        }

        if self.is_started() && !self.has_more_waves() && raiders_alive == 0 {
            if self.post_raid_ticks < POST_RAID_TICK_LIMIT {
                self.post_raid_ticks += 1;
            } else {
                self.status = RaidStatus::Victory;
                self.grant_hero_of_the_village(world);
            }
        }
    }

    /// Vanilla `Raid.updatePlayers`: the players the boss bar tracks.
    fn update_players(&mut self, world: &Arc<World>) {
        // Vanilla asks `level.getRaidAt(pos) == this`; the raid manager lock is already
        // held here, so the same membership test is expressed as the raid radius check.
        let center = self.center;
        let mut current = BTreeSet::new();
        world.players.iter_players(|uuid, player| {
            if LivingEntity::is_alive(&**player)
                && block_distance_sqr(center, player.block_position()) <= VALID_RAID_RADIUS_SQR
            {
                current.insert(*uuid);
            }
            true
        });
        self.bar.players = current;
    }

    /// Vanilla `Raid.moveRaidCenterToNearbyVillageSection`.
    fn move_raid_center_to_nearby_village(&mut self, world: &Arc<World>) {
        let section = find_section_closest_to_village(
            world,
            SectionPos::from_block_pos(self.center),
            SECTION_RADIUS_FOR_FINDING_NEW_VILLAGE_CENTER,
        );
        if world.is_village_section(section) {
            self.center = crate::world::village::section_center(section);
        }
    }

    /// Vanilla `Raid.updateRaiders`.
    fn update_raiders(&mut self, world: &Arc<World>) {
        let mut to_remove = Vec::new();

        for raiders in self.group_raider_map.values() {
            for uuid in raiders {
                let Some(raider) = world.get_entity_by_uuid(uuid) else {
                    to_remove.push(*uuid);
                    continue;
                };
                let raider_pos = raider.block_position();
                let outside_bounds =
                    block_distance_sqr(self.center, raider_pos) >= RAID_REMOVAL_THRESHOLD_SQR;

                if raider.is_removed() || outside_bounds {
                    to_remove.push(*uuid);
                    continue;
                }

                if raider.tick_count() <= 600 {
                    continue;
                }

                let Some(raider_mob) = raider.as_raider() else {
                    continue;
                };

                let no_action_time = raider
                    .as_living_entity()
                    .map_or(0, crate::entity::LivingEntity::no_action_time);
                if !world.is_village(raider_pos) && no_action_time > MAX_NO_ACTION_TIME {
                    raider_mob.set_ticks_outside_raid(raider_mob.ticks_outside_raid() + 1);
                }

                if raider_mob.ticks_outside_raid() >= OUTSIDE_RAID_BOUNDS_TIMEOUT {
                    to_remove.push(*uuid);
                }
            }
        }

        for uuid in to_remove {
            let wave = self.wave_of(uuid);
            self.remove_from_raid(world, uuid, true);
            if let Some(wave) = wave
                && self.group_to_leader_map.get(&wave) == Some(&uuid)
            {
                self.remove_leader(wave);
            }
        }
    }

    /// The wave a raider belongs to, if any.
    #[must_use]
    pub fn wave_of(&self, uuid: Uuid) -> Option<i32> {
        self.group_raider_map
            .iter()
            .find(|(_, raiders)| raiders.contains(&uuid))
            .map(|(wave, _)| *wave)
    }

    /// Vanilla `Raid.getValidSpawnPos`.
    fn get_valid_spawn_pos(&mut self, world: &Arc<World>) -> Option<BlockPos> {
        self.find_random_spawn_pos(world, 8)
    }

    /// Vanilla `Raid.spawnGroup`.
    fn spawn_group(&mut self, world: &Arc<World>, raid_id: i32, pos: BlockPos) {
        let mut leader_set = false;
        let group_number = self.groups_spawned + 1;
        self.total_health = 0.0;
        let difficulty = world.difficulty();
        let is_bonus_group = self.should_spawn_bonus_group();
        let spawn_position = block_spawn_position(pos);

        for raider_type in RaiderType::VALUES {
            let num_spawns =
                self.wave_raider_count(raider_type, group_number, difficulty, is_bonus_group);
            let mut ravagers_spawned = 0;

            for _ in 0..num_spawns {
                let Ok(raider) =
                    create_entity_instance(world, raider_type.entity_type(), spawn_position)
                else {
                    break;
                };

                if !leader_set && raider.as_raider().is_some_and(|mob| mob.can_be_leader()) {
                    if let Some(raider_mob) = raider.as_raider() {
                        raider_mob.set_patrol_leader(true);
                    }
                    self.set_leader(group_number, &raider);
                    leader_set = true;
                }

                self.join_raid(world, raid_id, group_number, &raider, Some(pos), false);

                if raider_type != RaiderType::Ravager {
                    continue;
                }

                let riding_raider = if group_number == Self::num_groups(Difficulty::Normal) {
                    create_entity_instance(world, &vanilla_entities::PILLAGER, spawn_position).ok()
                } else if group_number >= Self::num_groups(Difficulty::Hard) {
                    if ravagers_spawned == 0 {
                        create_entity_instance(world, &vanilla_entities::EVOKER, spawn_position)
                            .ok()
                    } else {
                        create_entity_instance(world, &vanilla_entities::VINDICATOR, spawn_position)
                            .ok()
                    }
                } else {
                    None
                };

                ravagers_spawned += 1;
                if let Some(riding_raider) = riding_raider {
                    self.join_raid(
                        world,
                        raid_id,
                        group_number,
                        &riding_raider,
                        Some(pos),
                        false,
                    );
                    riding_raider.snap_to(spawn_position, 0.0, 0.0);
                    riding_raider.start_riding(&raider);
                }
            }
        }

        self.wave_spawn_pos = None;
        self.groups_spawned += 1;
        self.update_bossbar(world);
    }

    /// Vanilla `Raid.joinRaid`.
    pub fn join_raid(
        &mut self,
        world: &Arc<World>,
        raid_id: i32,
        group_number: i32,
        raider: &SharedEntity,
        pos: Option<BlockPos>,
        exists: bool,
    ) {
        if !self.add_wave_mob(world, group_number, raider, true) {
            return;
        }

        let Some(raider_mob) = raider.as_raider() else {
            return;
        };
        raider_mob.set_current_raid(Some(raid_id));
        raider_mob.set_wave(group_number);
        raider_mob.set_can_join_raid(true);
        raider_mob.set_ticks_outside_raid(0);

        let Some(pos) = pos.filter(|_| !exists) else {
            return;
        };

        raider.snap_to(block_spawn_position(pos), 0.0, 0.0);
        if let Some(mob) = raider.as_mob() {
            mob.finalize_spawn(world, EntitySpawnReason::Event, None);
        }
        let snapshot = self.snapshot(raid_id);
        raider_mob.apply_raid_buffs(world, &snapshot, group_number, false);
        raider.set_on_ground(true);
        let _ = world.try_add_entity(SharedEntity::clone(raider));
    }

    /// Vanilla `Raid.addWaveMob`.
    pub fn add_wave_mob(
        &mut self,
        world: &Arc<World>,
        wave: i32,
        raider: &SharedEntity,
        update_health: bool,
    ) -> bool {
        let uuid = raider.uuid();
        let raiders = self.group_raider_map.entry(wave).or_default();
        raiders.remove(&uuid);
        raiders.insert(uuid);

        if update_health && let Some(living) = raider.as_living_entity() {
            self.total_health += living.get_health();
        }
        world.mark_raids_dirty();
        true
    }

    /// Vanilla `Raid.removeFromRaid`.
    pub fn remove_from_raid(
        &mut self,
        world: &Arc<World>,
        uuid: Uuid,
        remove_from_total_health: bool,
    ) {
        let Some(wave) = self.wave_of(uuid) else {
            return;
        };
        let Some(raiders) = self.group_raider_map.get_mut(&wave) else {
            return;
        };
        if !raiders.remove(&uuid) {
            return;
        }

        let raider = world.get_entity_by_uuid(&uuid);
        if remove_from_total_health
            && let Some(living) = raider.as_ref().and_then(|raider| raider.as_living_entity())
        {
            self.total_health -= living.get_health();
        }

        if let Some(raider) = raider
            && let Some(raider_mob) = raider.as_raider()
        {
            raider_mob.set_current_raid(None);
        }

        self.update_bossbar(world);
        world.mark_raids_dirty();
    }

    /// Vanilla `Raid.setLeader`.
    pub fn set_leader(&mut self, wave: i32, raider: &SharedEntity) {
        self.group_to_leader_map.insert(wave, raider.uuid());
        if let Some(living) = raider.as_living_entity() {
            living
                .living_base()
                .equipment()
                .lock()
                .set(EquipmentSlot::Head, Self::ominous_banner_instance());
        }
        if let Some(mob) = raider.as_mob() {
            mob.set_equipment_drop_chance(EquipmentSlot::Head, 2.0);
        }
    }

    /// Vanilla `Raid.removeLeader`.
    pub fn remove_leader(&mut self, wave: i32) {
        self.group_to_leader_map.remove(&wave);
    }

    /// Vanilla `Raid.updateBossbar`.
    pub fn update_bossbar(&mut self, world: &Arc<World>) {
        let health = self.health_of_living_raiders(world);
        self.bar.progress = if self.total_health <= 0.0 {
            0.0
        } else {
            (health / self.total_health).clamp(0.0, 1.0)
        };
    }

    /// Vanilla `Raid.playSound`: the raid horn is offset 13 blocks from each player
    /// towards the wave spawn position.
    fn play_sound(&self, world: &Arc<World>, sound_origin: BlockPos) {
        let raid_loc = DVec3::new(
            f64::from(sound_origin.x()) + 0.5,
            f64::from(sound_origin.y()) + 0.5,
            f64::from(sound_origin.z()) + 0.5,
        );
        let seed = rand::random::<i64>();
        let players_in_raid = self.bar.players.clone();

        world.players.iter_players(|uuid, player| {
            let player_loc = player.position();
            let dx = raid_loc.x - player_loc.x;
            let dz = raid_loc.z - player_loc.z;
            let distance = (dx * dx + dz * dz).sqrt();

            let (x, z) = if distance == 0.0 {
                (raid_loc.x, raid_loc.z)
            } else {
                (
                    player_loc.x + 13.0 / distance * dx,
                    player_loc.z + 13.0 / distance * dz,
                )
            };

            if distance <= 64.0 || players_in_raid.contains(uuid) {
                player.send_packet(steel_protocol::packets::game::CSound::new(
                    &steel_registry::sound_events::EVENT_RAID_HORN,
                    steel_protocol::packets::game::SoundSource::Neutral,
                    DVec3::new(x, player_loc.y, z),
                    64.0,
                    1.0,
                    seed,
                ));
            }
            true
        });
    }

    /// Vanilla `Raid.findRandomSpawnPos`.
    fn find_random_spawn_pos(&mut self, world: &Arc<World>, max_tries: i32) -> Option<BlockPos> {
        let seconds_remaining = self.raid_cooldown_ticks / 20;
        let how_far = 0.22 * seconds_remaining as f32 - 0.24;
        let start_angle = self.random.next_f32() * std::f32::consts::TAU;

        for i in 0..max_tries {
            let angle = start_angle + std::f32::consts::PI * i as f32 / 8.0;
            let spawn_x = self.center.x()
                + (angle.cos() * 32.0 * how_far).floor() as i32
                + self.random.next_i32_bounded(3) * how_far.floor() as i32;
            let spawn_z = self.center.z()
                + (angle.sin() * 32.0 * how_far).floor() as i32
                + self.random.next_i32_bounded(3) * how_far.floor() as i32;
            let Some(spawn_y) = world.height_at(
                crate::chunk::heightmap::HeightmapType::WorldSurface,
                spawn_x,
                spawn_z,
            ) else {
                continue;
            };

            if (spawn_y - self.center.y()).abs() > 96 {
                continue;
            }

            let spawn_pos = BlockPos::new(spawn_x, spawn_y, spawn_z);
            if world.is_village(spawn_pos)
                && seconds_remaining > ALLOW_SPAWNING_WITHIN_VILLAGE_SECONDS_THRESHOLD
            {
                continue;
            }

            if !world.has_chunks_at(
                spawn_pos.x() - 10,
                spawn_pos.z() - 10,
                spawn_pos.x() + 10,
                spawn_pos.z() + 10,
            ) || !world.is_position_entity_ticking(spawn_pos)
            {
                continue;
            }

            if ravager_spawn_position_ok(world, spawn_pos)
                || (world.get_block_state(spawn_pos.below()).get_block()
                    == &steel_registry::vanilla_blocks::SNOW
                    && world.get_block_state(spawn_pos).is_air())
            {
                return Some(spawn_pos);
            }
        }

        None
    }

    /// Vanilla `Raid` victory branch: every hero of the village gets the effect.
    fn grant_hero_of_the_village(&mut self, world: &Arc<World>) {
        let amplifier = (self.raid_omen_level - 1).max(0);
        for uuid in &self.heroes_of_the_village {
            let Some(hero) = world.get_entity_by_uuid(uuid) else {
                continue;
            };
            let Some(living) = hero.as_living_entity() else {
                continue;
            };
            if hero.is_spectator() || !LivingEntity::is_alive(living) {
                continue;
            }

            living.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::HERO_OF_THE_VILLAGE,
                HERO_OF_THE_VILLAGE_DURATION,
                amplifier,
            ));
            if let Some(player) = hero.as_player() {
                player.award_custom_stat(&steel_registry::vanilla_custom_stats::RAID_WIN);
            }
        }
    }

    /// The raid's random source, for deterministic raid tests.
    pub fn random_mut(&mut self) -> &mut LegacyRandom {
        &mut self.random
    }
}

fn block_spawn_position(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 1.0,
        f64::from(pos.z()) + 0.5,
    )
}

fn block_distance_sqr(a: BlockPos, b: BlockPos) -> i32 {
    let dx = a.x() - b.x();
    let dy = a.y() - b.y();
    let dz = a.z() - b.z();
    dx * dx + dy * dy + dz * dz
}

fn raid_name_component() -> TextComponent {
    TextComponent::translated(steel_utils::translations::EVENT_MINECRAFT_RAID.msg())
}

fn victory_bar_component() -> TextComponent {
    TextComponent::translated(steel_utils::translations::EVENT_MINECRAFT_RAID_VICTORY_FULL.msg())
}

fn defeat_bar_component() -> TextComponent {
    TextComponent::translated(steel_utils::translations::EVENT_MINECRAFT_RAID_DEFEAT_FULL.msg())
}

fn low_raider_count_component(raiders: i32) -> TextComponent {
    raid_name_component()
        .add_child(TextComponent::plain(" - "))
        .add_child(TextComponent::translated(
            steel_utils::translations::EVENT_MINECRAFT_RAID_RAIDERS_REMAINING
                .message([TextComponent::plain(raiders.to_string())]),
        ))
}

/// Vanilla `SpawnPlacementTypes.ON_GROUND` check used for the ravager spawn position.
#[must_use]
pub fn ravager_spawn_position_ok(world: &World, pos: BlockPos) -> bool {
    use steel_registry::blocks::block_state_ext::BlockStateExt;

    let below = pos.below();
    let below_state = world.get_block_state(below);
    let below_ok = crate::behavior::BLOCK_BEHAVIORS
        .get_behavior(below_state.get_block())
        .is_valid_spawn(below_state, world, below, &vanilla_entities::RAVAGER);

    below_ok
        && is_valid_empty_spawn_block(world, pos)
        && is_valid_empty_spawn_block(world, pos.above())
}

/// Vanilla `NaturalSpawner.isValidEmptySpawnBlock`.
fn is_valid_empty_spawn_block(world: &World, pos: BlockPos) -> bool {
    use steel_registry::TaggedRegistryExt;
    use steel_registry::blocks::block_state_ext::BlockStateExt;
    use steel_registry::blocks::shapes::is_shape_full_block;
    use steel_registry::vanilla_block_tags::BlockTag;

    let state = world.get_block_state(pos);
    if is_shape_full_block(state.get_static_collision_shape()) {
        return false;
    }
    if crate::behavior::BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .is_signal_source(state, crate::world::SignalQueryContext::DEFAULT)
    {
        return false;
    }
    if state.has_fluid() {
        return false;
    }
    !steel_registry::REGISTRY
        .blocks
        .is_in_tag(state.get_block(), &BlockTag::INVALID_SPAWN_INSIDE)
}

/// Mirrors vanilla `Raids.createOrExtendRaid`'s village POI centre search.
#[must_use]
pub fn village_raid_center(world: &World, raid_position: BlockPos) -> BlockPos {
    let pois = world.poi_storage.lock().get_in_range(
        &is_village_poi,
        raid_position,
        VILLAGE_SEARCH_RADIUS,
        crate::poi::OccupationStatus::Occupied,
    );

    if pois.is_empty() {
        return raid_position;
    }

    let mut total = DVec3::ZERO;
    for (_, pos) in &pois {
        total += DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
    }
    BlockPos::from(total / pois.len() as f64)
}

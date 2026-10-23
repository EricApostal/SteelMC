//! Vanilla `Panda` (`net.minecraft.world.entity.animal.panda.Panda`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PandaEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_item_tags};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    AvoidEntityGoal, BreedGoal, FloatGoal, FollowParentGoal, HurtByTargetGoal, LookAtPlayerGoal,
    MeleeAttackGoal, PanicGoal, RandomLookAroundGoal, TemptGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 1;
/// Vanilla `Panda` baby spawn chance in group data.
const PANDA_BABY_CHANCE: f32 = 0.2;
/// Vanilla `Panda.Gene` count.
const PANDA_GENE_COUNT: i32 = 7;

/// Vanilla `Panda.Gene`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PandaGene {
    /// The default panda.
    #[default]
    Normal,
    /// Lazy panda (sits and lies down).
    Lazy,
    /// Worried panda.
    Worried,
    /// Playful panda (rolls around).
    Playful,
    /// Brown panda.
    Brown,
    /// Weak panda (sneezes).
    Weak,
    /// Aggressive panda.
    Aggressive,
}

impl PandaGene {
    /// Vanilla `Gene.values()` order.
    pub const VALUES: [Self; 7] = [
        Self::Normal,
        Self::Lazy,
        Self::Worried,
        Self::Playful,
        Self::Brown,
        Self::Weak,
        Self::Aggressive,
    ];

    /// Vanilla `Gene.getId`.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Normal => 0,
            Self::Lazy => 1,
            Self::Worried => 2,
            Self::Playful => 3,
            Self::Brown => 4,
            Self::Weak => 5,
            Self::Aggressive => 6,
        }
    }

    /// Vanilla `Gene.byId`.
    #[must_use]
    pub fn by_id(id: i32) -> Self {
        Self::VALUES
            .into_iter()
            .find(|gene| gene.id() == id)
            .unwrap_or(Self::Normal)
    }

    /// Vanilla `Gene.isRecessive`.
    #[must_use]
    pub const fn is_recessive(self) -> bool {
        matches!(self, Self::Brown | Self::Weak)
    }

    /// Vanilla `Gene.getRandom`.
    #[must_use]
    pub fn random() -> Self {
        Self::by_id(rand::random_range(0..PANDA_GENE_COUNT))
    }

    /// Vanilla `Gene.getVariantFromGenes`.
    #[must_use]
    pub const fn variant_from_genes(main: Self, hidden: Self) -> Self {
        if main.is_recessive() {
            return hidden;
        }
        main
    }
}

/// Vanilla `Panda`.
#[entity_behavior(class = "Panda")]
pub struct PandaEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<PandaEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PandaEntity`.
unsafe impl DowncastType for PandaEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/panda");
}

impl PandaEntity {
    /// Creates a new panda entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved panda entity.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        mob_base.set_xp_reward(XP_REWARD);
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = PandaEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.main_gene.set(PandaGene::Normal.id() as i8);
        entity_data.hidden_gene.set(PandaGene::Normal.id() as i8);

        {
            // Vanilla `Panda.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(2, PanicGoal::new(2.0));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, MeleeAttackGoal::new(1.2, true));
            goal_selector.add_goal(
                4,
                TemptGoal::new(
                    1.0,
                    |item_stack: &ItemStack| {
                        !item_stack.is_empty()
                            && item_stack
                                .item()
                                .has_tag(&vanilla_item_tags::ItemTag::PANDA_FOOD)
                    },
                    false,
                ),
            );
            // Vanilla `PandaAvoidGoal` passes `EntitySelector.NO_SPECTATORS` as the
            // `predicateOnAvoidEntity` (creative players are still avoided).
            goal_selector.add_goal(
                6,
                AvoidEntityGoal::with_predicates(
                    8.0,
                    2.0,
                    2.0,
                    |target, _world| target.entity_type() == &vanilla_entities::PLAYER,
                    |target, _world| !target.is_spectator(),
                ),
            );
            goal_selector.add_goal(9, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(10, RandomLookAroundGoal::new());
            goal_selector.add_goal(13, FollowParentGoal::new(1.25));
            goal_selector.add_goal(14, WaterAvoidingRandomStrollGoal::new(1.0));

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
        }
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }

        let display = self.living_base.mob_effect_display_state();

        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }

        self.entity_data.set_base_invisible_flag(display.invisible);
    }

    /// Vanilla `Panda.getMainGene`.
    #[must_use]
    pub fn main_gene(&self) -> PandaGene {
        PandaGene::by_id(i32::from(*self.entity_data.lock().main_gene.get()))
    }

    /// Vanilla `Panda.setMainGene`.
    pub fn set_main_gene(&self, gene: PandaGene) {
        self.entity_data.lock().main_gene.set(gene.id() as i8);
    }

    /// Vanilla `Panda.getHiddenGene`.
    #[must_use]
    pub fn hidden_gene(&self) -> PandaGene {
        PandaGene::by_id(i32::from(*self.entity_data.lock().hidden_gene.get()))
    }

    /// Vanilla `Panda.setHiddenGene`.
    pub fn set_hidden_gene(&self, gene: PandaGene) {
        self.entity_data.lock().hidden_gene.set(gene.id() as i8);
    }

    /// Vanilla `Panda.getVariant`.
    #[must_use]
    pub fn variant(&self) -> PandaGene {
        PandaGene::variant_from_genes(self.main_gene(), self.hidden_gene())
    }

    /// Vanilla `Panda.isAggressive`.
    #[must_use]
    pub fn is_aggressive(&self) -> bool {
        self.variant() == PandaGene::Aggressive
    }

    /// Vanilla `Panda.isLazy`.
    #[must_use]
    pub fn is_lazy(&self) -> bool {
        self.variant() == PandaGene::Lazy
    }

    /// Vanilla `Panda.isWorried`.
    #[must_use]
    pub fn is_worried(&self) -> bool {
        self.variant() == PandaGene::Worried
    }

    /// Vanilla `Panda.isPlayful`.
    #[must_use]
    pub fn is_playful(&self) -> bool {
        self.variant() == PandaGene::Playful
    }

    /// Vanilla `Panda.isBrown`.
    #[must_use]
    pub fn is_brown(&self) -> bool {
        self.variant() == PandaGene::Brown
    }

    /// Vanilla `Panda.isWeak`.
    #[must_use]
    pub fn is_weak(&self) -> bool {
        self.variant() == PandaGene::Weak
    }

    /// Vanilla `Panda.setAttributes`: weak pandas have 10 health, aggressive ones 20 attack.
    fn apply_gene_attributes(&self) {
        let max_health = if self.is_weak() { 10.0 } else { 20.0 };
        let attack_damage = if self.is_aggressive() { 6.0 } else { 2.0 };
        let mut attributes = self.attributes().lock();
        attributes.set_base_value(vanilla_attributes::MAX_HEALTH, max_health);
        attributes.set_base_value(vanilla_attributes::ATTACK_DAMAGE, attack_damage);
        drop(attributes);

        if self.get_health() > self.get_max_health() {
            let max_health = self.get_max_health();
            self.set_health(max_health);
        }
    }
}

impl Entity for PandaEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(f64::from(DEFAULT_STEP_HEIGHT)) as f32
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        (
            &sound_events::ENTITY_PANDA_STEP,
            &sound_events::ENTITY_PANDA_STEP,
        )
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_PANDA_AMBIENT
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_PANDA_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("MainGene", self.main_gene().id() as i8);
        nbt.insert("HiddenGene", self.hidden_gene().id() as i8);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_main_gene(PandaGene::by_id(
            nbt.byte("MainGene").map(i32::from).unwrap_or(0),
        ));
        self.set_hidden_gene(PandaGene::by_id(
            nbt.byte("HiddenGene").map(i32::from).unwrap_or(0),
        ));
        self.apply_gene_attributes();
    }
}

impl LivingEntity for PandaEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PANDA_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PANDA_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.mob_ai_step(entity)
    }
}

impl Mob for PandaEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Panda.getAmbientSound`: aggressive pandas have their own call.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_aggressive() {
            &sound_events::ENTITY_PANDA_AGGRESSIVE_AMBIENT
        } else {
            &sound_events::ENTITY_PANDA_AMBIENT
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Panda.finalizeSpawn`: random main/hidden genes and the gene-derived attributes.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_main_gene(PandaGene::random());
        self.set_hidden_gene(PandaGene::random());
        self.apply_gene_attributes();
        let group_data = group_data.or_else(|| {
            Some(SpawnGroupData::AgeableMob(
                crate::entity::AgeableMobGroupData::with_baby_spawn_chance(PANDA_BABY_CHANCE),
            ))
        });
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }
}

impl AgeableMob for PandaEntity {
    fn ageable_base(&self) -> &AgeableMobBase {
        &self.ageable_base
    }

    fn is_age_locked(&self) -> bool {
        *self.entity_data.lock().ageable_mob().age_locked.get()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.entity_data
            .lock()
            .ageable_mob_mut()
            .age_locked
            .set(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.entity_data.lock().ageable_mob_mut().baby.set(baby);
    }
}

impl Animal for PandaEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `Panda.isFood` uses the `#minecraft:panda_food` tag.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        !item_stack.is_empty()
            && item_stack
                .item()
                .has_tag(&vanilla_item_tags::ItemTag::PANDA_FOOD)
    }
}

impl PathfinderMob for PandaEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::{PandaEntity, PandaGene};
    use crate::entity::Entity;

    #[test]
    fn panda_gene_inheritance_matches_vanilla() {
        init_vanilla_registry();
        let panda = PandaEntity::new(&vanilla_entities::PANDA, 1, DVec3::ZERO, Weak::new());

        panda.set_main_gene(PandaGene::Brown);
        panda.set_hidden_gene(PandaGene::Lazy);
        // A recessive main gene yields the hidden gene as the variant.
        assert_eq!(panda.variant(), PandaGene::Lazy);
        assert!(panda.is_lazy());

        panda.set_main_gene(PandaGene::Aggressive);
        assert!(panda.is_aggressive());
        assert_eq!(panda.entity_type(), &vanilla_entities::PANDA);
    }
}

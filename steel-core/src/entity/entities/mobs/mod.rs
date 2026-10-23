//! Living entity implementations grouped by behavior.

pub mod ambient;
pub mod bosses;
pub mod golem;
pub mod hostile;
pub mod illager;
pub mod jockeys;
pub mod neutral;
pub mod npc;
pub mod passive;
pub mod skeleton;
pub(crate) mod spawn_equipment;
pub mod water;
pub mod zombie;

pub use ambient::BatEntity;
pub use bosses::{
    EnderDragonEntity, EnderDragonPartEntity, EnderDragonPartKind, EnderDragonPhase,
    EnderDragonPhaseManager,
};
pub use golem::{CopperGolemEntity, IronGolemEntity, SnowGolemEntity};
pub use hostile::{
    BlazeEntity, CaveSpiderEntity, CreakingEntity, CreeperEntity, ElderGuardianEntity,
    EndermanEntity, EndermiteEntity, GiantEntity, GhastEntity, GuardianEntity, HoglinEntity,
    MagmaCubeEntity, PhantomEntity, PiglinBruteEntity, PiglinEntity, SilverfishEntity, SlimeEntity,
    SpiderEntity, VexEntity, WardenEntity, WitchEntity, WitherBossEntity, ZoglinEntity,
    ZombifiedPiglinEntity,
};
pub use illager::{
    EvokerEntity, IllagerArmPose, IllagerSpell, IllusionerEntity, PillagerEntity, RaiderKeys,
    RavagerEntity, VindicatorEntity,
};
pub use neutral::{
    BeeEntity, DolphinEntity, FoxEntity, GlowSquidEntity, PandaEntity, PolarBearEntity,
    SquidEntity, WolfEntity,
};
pub use npc::{VillagerEntity, WanderingTraderEntity};
pub use passive::{
    AllayEntity, ArmadilloEntity, AxolotlEntity, CamelEntity, CamelHuskEntity, CatEntity,
    ChickenEntity, CowEntity, DonkeyEntity, FrogEntity, GoatEntity, HorseEntity, LlamaEntity,
    MuleEntity, MushroomCowEntity, OcelotEntity, ParrotEntity, PigEntity, RabbitEntity,
    SheepEntity, SkeletonHorseEntity, SnifferEntity, StriderEntity, TadpoleEntity,
    TraderLlamaEntity, TurtleEntity, ZombieHorseEntity,
};
pub use skeleton::{
    BoggedEntity, ParchedEntity, SkeletonEntity, StrayEntity, WitherSkeletonEntity,
};
pub use zombie::{DrownedEntity, HuskEntity, ZombieEntity, ZombieVillagerEntity};

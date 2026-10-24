//! Steel capability traits for the mob state vanilla goals read from their concrete mob class.
//!
//! Vanilla goals are inner classes or take a concrete mob (`TamableAnimal`, `Creeper`,
//! `AbstractHorse`, `Llama`, `Cat`, `ShoulderRidingEntity`, `AbstractSchoolingFish`,
//! `RangedAttackMob`). Steel goals receive `&dyn PathfinderMob` per call, so the mob state
//! those goals need is expressed as a capability trait the mob implements, reached through
//! the matching `Entity::as_*` accessor. Every trait here mirrors vanilla method names and
//! signatures and documents the vanilla class it replaces.

use steel_registry::sound_event::SoundEventRef;

use crate::entity::SharedEntity;

/// Vanilla `TamableAnimal`: owner binding plus the sitting pose goals steer.
pub trait Tameable: Send + Sync + 'static {
    /// Vanilla `TamableAnimal.getOwner`.
    fn owner(&self) -> Option<SharedEntity>;

    /// Vanilla `TamableAnimal.isTame`.
    fn is_tame(&self) -> bool;

    /// Vanilla `TamableAnimal.isOrderedToSit`.
    fn is_ordered_to_sit(&self) -> bool;

    /// Vanilla `TamableAnimal.setInSittingPose`.
    fn set_in_sitting_pose(&self, sitting: bool);

    /// Vanilla `TamableAnimal.isInSittingPose`.
    fn is_in_sitting_pose(&self) -> bool;

    /// Vanilla `TamableAnimal.unableToMoveToOwner`.
    fn unable_to_move_to_owner(&self) -> bool;

    /// Vanilla `TamableAnimal.shouldTryTeleportToOwner`.
    fn should_try_teleport_to_owner(&self) -> bool;

    /// Vanilla `TamableAnimal.tryToTeleportToOwner`.
    fn try_to_teleport_to_owner(&self);

    /// Vanilla `TamableAnimal.wantsToAttack`, the "do not attack my owner's friends" veto.
    fn wants_to_attack(&self, target: &SharedEntity, owner: &SharedEntity) -> bool;

    /// Vanilla `Wolf.isInterested` (`BegGoal` keeps it set while a player holds food).
    ///
    /// Only wolves register `BegGoal`, so no other tameable needs to override this.
    fn is_interested(&self) -> bool {
        false
    }

    /// Vanilla `Wolf.setIsInterested`.
    fn set_is_interested(&self, _interested: bool) {}
}

/// Vanilla `RangedAttackMob`: a mob that shoots a projectile at its target.
pub trait RangedAttackMob: Send + Sync + 'static {
    /// Vanilla `RangedAttackMob.performRangedAttack`.
    ///
    /// Returns whether a projectile was released. Vanilla returns nothing, but Steel's
    /// projectile layer is factory-gated (`ENTITIES.has_factory(&vanilla_entities::ARROW)`
    /// until the arrow entity lands), so the ranged goals need to know when to keep their
    /// vanilla charge window instead of pretending a shot happened.
    fn perform_ranged_attack(&self, target: &SharedEntity, power: f32) -> bool;
}

/// Vanilla `AbstractSchoolingFish`: the flock membership state `FollowFlockLeaderGoal` drives.
pub trait SchoolingFish: Send + Sync + 'static {
    /// Vanilla `AbstractSchoolingFish.hasFollowers`.
    fn has_followers(&self) -> bool;

    /// Vanilla `AbstractSchoolingFish.isFollower`.
    fn is_follower(&self) -> bool;

    /// Vanilla `AbstractSchoolingFish.canBeFollowed`.
    fn can_be_followed(&self) -> bool;

    /// Vanilla `AbstractSchoolingFish.addFollowers`.
    fn add_followers(&self, followers: Vec<SharedEntity>);

    /// Vanilla `AbstractSchoolingFish.inRangeOfLeader`.
    fn in_range_of_leader(&self) -> bool;

    /// Vanilla `AbstractSchoolingFish.pathToLeader`; `mob` is this fish's own handle, which
    /// the club-membership bookkeeping needs to resolve the leader.
    fn path_to_leader(&self, mob: &SharedEntity);

    /// Vanilla `AbstractSchoolingFish.stopFollowing`.
    fn stop_following(&self);
}

/// Vanilla `Creeper`: the swell direction `SwellGoal` steers.
pub trait CreeperLike: Send + Sync + 'static {
    /// Vanilla `Creeper.getSwellDir`.
    fn swell_dir(&self) -> i32;

    /// Vanilla `Creeper.setSwellDir`.
    fn set_swell_dir(&self, swell_dir: i32);
}

/// Vanilla `Cat` state used by `CatSitOnBlockGoal` and `CatLieOnBlockGoal`.
pub trait Feline: Tameable {
    /// Vanilla `Cat.isLying`.
    fn is_lying(&self) -> bool;

    /// Vanilla `Cat.setLying`.
    fn set_lying(&self, lying: bool);
}

/// Vanilla `AbstractHorse`: the stand and buck-off state of the equine goals.
pub trait AbstractHorse: Send + Sync + 'static {
    /// Vanilla `AbstractHorse.standIfPossible`.
    fn stand_if_possible(&self);

    /// Vanilla `AbstractHorse.getAmbientStandInterval`.
    fn ambient_stand_interval(&self) -> i32;

    /// Vanilla `AbstractHorse.isImmobile`.
    fn is_immobile(&self) -> bool;

    /// Vanilla `AbstractHorse.getAmbientStandSound`.
    fn ambient_stand_sound(&self) -> Option<SoundEventRef>;

    /// Vanilla `AbstractHorse.isMobControlled`.
    fn is_mob_controlled(&self) -> bool;

    /// Vanilla `AbstractHorse.isTamed`.
    fn is_tamed(&self) -> bool;

    /// Vanilla `AbstractHorse.getTemper`.
    fn temper(&self) -> i32;

    /// Vanilla `AbstractHorse.getMaxTemper`.
    fn max_temper(&self) -> i32;

    /// Vanilla `AbstractHorse.modifyTemper`.
    fn modify_temper(&self, amount: i32);

    /// Vanilla `AbstractHorse.tameWithName`.
    fn tame_with_name(&self, player: &SharedEntity);

    /// Vanilla `AbstractHorse.ejectPassengers`.
    fn eject_passengers(&self);

    /// Vanilla `AbstractHorse.makeMad`.
    fn make_mad(&self, mob: &SharedEntity);
}

/// Vanilla `Llama` caravan state used by `LlamaFollowCaravanGoal`.
pub trait CaravanLlama: Send + Sync + 'static {
    /// Vanilla `Llama.inCaravan`.
    fn in_caravan(&self) -> bool;

    /// Vanilla `Llama.getCaravanHead`.
    fn caravan_head(&self) -> Option<SharedEntity>;

    /// Vanilla `Llama.hasCaravanTail`.
    fn has_caravan_tail(&self) -> bool;

    /// Vanilla `Llama.joinCaravan`.
    fn join_caravan(&self, head: &SharedEntity);

    /// Vanilla `Llama.leaveCaravan`.
    fn leave_caravan(&self);
}

/// Vanilla `ShoulderRidingEntity`: parrots perching on their owner's shoulder.
pub trait ShoulderRiding: Tameable {
    /// Vanilla `ShoulderRidingEntity.canSitOnShoulder`.
    fn can_sit_on_shoulder(&self) -> bool;

    /// Vanilla `ShoulderRidingEntity.setEntityOnShoulder`.
    fn set_entity_on_shoulder(&self, owner: &SharedEntity) -> bool;
}

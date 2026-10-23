//! Vanilla `goal/target` goals shared by neutral mobs and village defenders.

mod angry_at_players;
mod defend_village;
mod reset_universal_anger;

pub(crate) use angry_at_players::AngryAtPlayerTargetGoal;
pub(crate) use defend_village::DefendVillageTargetGoal;
pub(crate) use reset_universal_anger::ResetUniversalAngerTargetGoal;

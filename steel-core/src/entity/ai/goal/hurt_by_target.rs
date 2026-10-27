use steel_registry::vanilla_game_rules::UNIVERSAL_ANGER;
use steel_utils::{DowncastTypeKey, WorldAabb};

use super::capabilities::Tameable;
use super::selector::{Goal, GoalControls};
use super::target_goal::{TargetGoalBase, follow_distance};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{PathfinderMob, SharedEntity};

const HURT_BY_UNSEEN_MEMORY_TICKS: i32 = 300;
const ALERT_RANGE_Y: f64 = 10.0;

pub(crate) struct HurtByTargetGoal {
    target_goal: TargetGoalBase,
    targeting: TargetingConditions,
    timestamp: i32,
    ignore_damage_types: Vec<DowncastTypeKey>,
    alert_same_type: bool,
    ignore_alert_types: Vec<DowncastTypeKey>,
}

impl HurtByTargetGoal {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            target_goal: TargetGoalBase::new(true, false),
            targeting: TargetingConditions::for_combat()
                .ignore_line_of_sight()
                .ignore_invisibility_testing(),
            timestamp: 0,
            ignore_damage_types: Vec::new(),
            alert_same_type: false,
            ignore_alert_types: Vec::new(),
        }
    }

    #[must_use]
    pub(crate) fn with_ignored_damage_types(
        mut self,
        types: impl IntoIterator<Item = DowncastTypeKey>,
    ) -> Self {
        self.ignore_damage_types = types.into_iter().collect();
        self
    }

    #[must_use]
    pub(crate) fn set_alert_others(
        mut self,
        except_types: impl IntoIterator<Item = DowncastTypeKey>,
    ) -> Self {
        self.alert_same_type = true;
        self.ignore_alert_types = except_types.into_iter().collect();
        self
    }

    fn alert_others(&self, mob: &dyn PathfinderMob, hurt_by_mob: &SharedEntity) {
        let Some(world) = mob.level() else {
            return;
        };

        let within = follow_distance(mob);
        let position = mob.position();
        let search_box = WorldAabb::new(
            position.x,
            position.y,
            position.z,
            position.x + 1.0,
            position.y + 1.0,
            position.z + 1.0,
        )
        .inflate_xyz(within, ALERT_RANGE_Y, within);
        // Vanilla filters on `mob.getClass()`, which matches Java subclasses (a `Zombie`
        // alerts husks, drowned and zombie villagers too). Steel compares concrete downcast
        // type keys, so the alert stays within the mob's own Rust type; the vanilla
        // subclass families are only reproduced where the caller lists the extra types in
        // `set_alert_others`.
        let mob_type_key = mob.downcast_type_key();
        let mob_uuid = mob.uuid();
        // Vanilla `this.mob instanceof TamableAnimal ? tamableAnimal.getOwner() : null`; the
        // cast on `other` is safe because the entity class filter above already matched.
        // Both sides compare `Option<Uuid>`, so two untamed tamables (`None == None`) match.
        let mob_owner = mob
            .as_tameable()
            .map(|tameable| tameable.owner().map(|owner| owner.uuid()));

        for entity in world.get_entities_in_aabb_matching(&search_box, |entity| {
            if entity.uuid() == mob_uuid
                || entity.downcast_type_key() != mob_type_key
                || self
                    .ignore_alert_types
                    .contains(&entity.downcast_type_key())
            {
                return false;
            }

            let Some(other) = entity.as_mob() else {
                return false;
            };

            if let Some(mob_owner) = mob_owner {
                let other_owner = entity
                    .as_tameable()
                    .and_then(Tameable::owner)
                    .map(|owner| owner.uuid());
                if other_owner != mob_owner {
                    return false;
                }
            }

            other.target().is_none() && !other.is_allied_to(hurt_by_mob.as_ref())
        }) {
            if let Some(other) = entity.as_mob() {
                let _ = other.set_target(Some(hurt_by_mob));
            }
        }
    }
}

impl Default for HurtByTargetGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for HurtByTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let timestamp = mob.last_hurt_by_mob_timestamp();
        if timestamp == self.timestamp {
            return false;
        }

        let Some(hurt_by_mob) = mob.last_hurt_by_mob() else {
            return false;
        };
        if self
            .ignore_damage_types
            .contains(&hurt_by_mob.downcast_type_key())
        {
            return false;
        }

        if hurt_by_mob.as_player().is_some()
            && mob
                .level()
                .is_some_and(|world| world.get_game_rule(&UNIVERSAL_ANGER))
        {
            return false;
        }

        self.target_goal
            .can_attack(mob, hurt_by_mob.as_living_entity(), &self.targeting)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.target_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(hurt_by_mob) = mob.last_hurt_by_mob() else {
            return;
        };

        let _ = mob.set_target(Some(&hurt_by_mob));
        self.target_goal.set_target_mob(mob.target());
        self.timestamp = mob.last_hurt_by_mob_timestamp();
        self.target_goal
            .set_unseen_memory_ticks(HURT_BY_UNSEEN_MEMORY_TICKS);

        if self.alert_same_type {
            self.alert_others(mob, &hurt_by_mob);
        }

        self.target_goal.start();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.target_goal.stop(mob);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::entities::{CowEntity, PigEntity};
    use crate::entity::{Entity, LivingEntity, Mob};
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
    use steel_utils::ChunkPos;

    #[test]
    fn targets_attacker_and_alerts_unassigned_same_type_mobs() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("hurt_by_target_goal");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let hunter = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::new(8.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));
        let ally = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            2,
            DVec3::new(9.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));
        let attacker = Arc::new(CowEntity::new(
            &vanilla_entities::COW,
            3,
            DVec3::new(10.0, 65.0, 8.0),
            Arc::downgrade(world),
        ));

        let hunter_entity: SharedEntity = Arc::<PigEntity>::clone(&hunter);
        let ally_entity: SharedEntity = Arc::<PigEntity>::clone(&ally);
        let attacker_entity: SharedEntity = Arc::<CowEntity>::clone(&attacker);
        for entity in [hunter_entity, ally_entity, Arc::clone(&attacker_entity)] {
            world
                .try_add_entity(entity)
                .expect("test entity should attach to the loaded chunk");
        }

        hunter.advance_tick_count();
        hunter.set_last_hurt_by_mob(Some(&attacker_entity));
        let mut goal = HurtByTargetGoal::new().set_alert_others([]);

        assert!(goal.can_use(hunter.as_ref()));
        goal.start(hunter.as_ref());

        let Some(hunter_target) = hunter.target() else {
            panic!("hurt-by-target goal should assign the attacker");
        };
        let Some(ally_target) = ally.target() else {
            panic!("alerted same-type mob should receive the attacker");
        };
        assert_eq!(hunter_target.uuid(), attacker.uuid());
        assert_eq!(ally_target.uuid(), attacker.uuid());
    }

    /// Real-path hit retaliation: join a player so the chunk is entity-ticking, spawn hostile
    /// mobs next to it, damage them through the real damage path (`hurt` with a `player_attack`
    /// source) and tick the world. Vanilla `LivingEntity.hurtServer` records the attacker as
    /// `lastHurtByMob`; the mob's target selector then runs `HurtByTargetGoal`, which applies
    /// that attacker as the mob's target, and `TargetGoal.canContinueToUse` keeps it.
    ///
    /// Regression guard: before the `CreeperEntity::set_target_refusing_goat` fix this test
    /// aborted the test binary with a stack overflow, because that helper delegated through
    /// `Mob::set_target(self, ..)`, re-entering the creeper's own `set_target` override — the
    /// creeper's retaliation therefore crashed the server instead of acquiring the attacker.
    #[test]
    fn hit_mob_retaliates_against_the_player_that_hit_it() {
        use steel_registry::{vanilla_damage_types, vanilla_game_rules, vanilla_world_clocks};

        use crate::entity::damage::DamageSource;
        use crate::entity::entities::{CreeperEntity, ZombieEntity};
        use crate::entity::{Mob, SharedEntity, add_spawned_entity, init_entities};
        use crate::player::{Player, ResetReason};
        use crate::test_support::TestPlayerBuilder;
        use crate::world::World;

        init_vanilla_registry();
        init_entities();
        init_behaviors();
        let fixture = fresh_test_world("hurt_by_target_retaliation");
        let world = &fixture.world;
        // A joined player only makes a neighbourhood of chunks entity-ticking, and a mob only
        // runs its AI (and therefore its target selector) while it ticks.
        for chunk_x in -2..=2 {
            for chunk_z in -2..=2 {
                insert_ready_full_chunk(world, ChunkPos::new(chunk_x, chunk_z));
            }
        }
        world.set_game_rule(&vanilla_game_rules::SPAWN_MOBS, false);
        world.set_game_rule(&vanilla_game_rules::SPAWN_MONSTERS, false);
        // Night: zombies are in `BURN_IN_DAYLIGHT`, and a daytime burn roll would spend the
        // hurt cooldown on its own 1.0 `on_fire` hit before this test's 1.0 hit lands.
        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, 18_000);
        assert!(
            !world.monsters_burn(),
            "night must disable daylight burning"
        );

        let player = TestPlayerBuilder::new(Arc::clone(world), "Attacker", 1).build();
        player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        let player_uuid = player.uuid();

        let zombie = Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            2,
            DVec3::new(2.5, 64.0, 0.5),
            Arc::downgrade(world),
        ));
        let creeper = Arc::new(CreeperEntity::new(
            &vanilla_entities::CREEPER,
            3,
            DVec3::new(4.0, 64.0, 0.5),
            Arc::downgrade(world),
        ));
        let mobs: [(&str, SharedEntity); 2] = [
            (
                "zombie",
                Arc::<ZombieEntity>::clone(&zombie) as SharedEntity,
            ),
            (
                "creeper",
                Arc::<CreeperEntity>::clone(&creeper) as SharedEntity,
            ),
        ];
        for (name, mob) in &mobs {
            add_spawned_entity(world, Arc::clone(mob)).expect("test mob should join the world");
            assert!(mob.as_mob().is_some(), "{name} should be a mob");
        }

        let mut ticks = 0_u64;
        let tick = |world: &Arc<World>, ticks: &mut u64| {
            world.game_time.advance();
            world.tick_game(*ticks, true);
            *ticks += 1;
        };
        // The mobs must tick before the hit: vanilla `HurtByTargetGoal.canUse` compares the
        // victim's `getLastHurtByMobTimestamp` (the victim's own `tickCount` at the hit) against
        // the goal's stored timestamp, which starts at 0.
        for _ in 0..4 {
            tick(world, &mut ticks);
        }

        let player_entity: SharedEntity = Arc::<Player>::clone(&player) as SharedEntity;
        let source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::clone(&player_entity),
        );
        for (name, mob) in &mobs {
            assert!(
                mob.tick_count() > 0,
                "{name} must be ticking inside the player's chunks"
            );
            assert!(
                mob.hurt(world, &source, 1.0),
                "the test hit must damage the {name}"
            );
            assert!(
                mob.as_living_entity()
                    .is_some_and(|living| living.last_hurt_by_mob().is_some()),
                "vanilla `LivingEntity.hurtServer` must record the attacker as last-hurt-by"
            );
        }

        for _ in 0..8 {
            tick(world, &mut ticks);
        }

        for (name, mob) in &mobs {
            let target = mob.as_mob().and_then(Mob::target);
            assert_eq!(
                target.map(|target| target.uuid()),
                Some(player_uuid),
                "{name} must retaliate against the player that hit it"
            );
        }
    }
}

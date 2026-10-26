//! `/execute` operations that transform or fork the command source.

use steel_utils::translations;
use text_components::TextComponent;

use super::super::super::{
    brigadier::{CommandNodeBuilder, CommandRedirectTarget, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
};
use super::super::summon;
use crate::entity::Leashable;
use crate::entity::LivingEntity;
use crate::entity::Mob;
use crate::entity::ai::goal::Tameable;
use crate::entity::{EntityAnchor, SharedEntity};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

const EXECUTE_ROOT: CommandRedirectTarget = CommandRedirectTarget::CommandRoot;

pub(super) fn as_operation() -> Builder {
    literal("as").then(argument("targets", SteelArgumentType::entities()).forks(
        EXECUTE_ROOT,
        |context| {
            Ok(context
                .optional_entities("targets")?
                .into_iter()
                .map(|entity| context.source().with_entity(entity))
                .collect())
        },
    ))
}

pub(super) fn at_operation() -> Builder {
    literal("at").then(argument("targets", SteelArgumentType::entities()).forks(
        EXECUTE_ROOT,
        |context| {
            context
                .optional_entities("targets")?
                .into_iter()
                .map(|entity| {
                    let world = entity.level().ok_or_else(|| {
                        CommandSyntaxError::dynamic("Selected entity is not in a loaded world")
                    })?;
                    Ok(context
                        .source()
                        .with_world(world)?
                        .with_position(entity.position())
                        .with_rotation(entity.rotation()))
                })
                .collect::<Result<Vec<_>, CommandSyntaxError>>()
        },
    ))
}

pub(super) fn positioned_operation() -> Builder {
    literal("positioned")
        .then(
            argument("pos", SteelArgumentType::vec3(true)).redirects_with(
                EXECUTE_ROOT,
                |context: &SteelCommandContext<CommandSource>| {
                    let position = required_coordinates(context, "pos")?.position(context.source());
                    Ok(context
                        .source()
                        .with_position(position)
                        .with_anchor(EntityAnchor::Feet))
                },
            ),
        )
        .then(
            literal("as").then(argument("targets", SteelArgumentType::entities()).forks(
                EXECUTE_ROOT,
                |context| {
                    Ok(context
                        .optional_entities("targets")?
                        .into_iter()
                        .map(|entity| context.source().with_position(entity.position()))
                        .collect())
                },
            )),
        )
        .then(literal("over").then(
            argument("heightmap", SteelArgumentType::heightmap()).redirects_with(
                EXECUTE_ROOT,
                |context: &SteelCommandContext<CommandSource>| {
                    let source = context.source();
                    let position = source.position();
                    let heightmap = context.heightmap("heightmap")?;
                    let Some(height) = source.world().height_at(
                        heightmap,
                        position.x.floor() as i32,
                        position.z.floor() as i32,
                    ) else {
                        return Err(CommandSyntaxError::dynamic(TextComponent::from(
                            &translations::ARGUMENT_POS_UNLOADED,
                        )));
                    };
                    Ok(source.with_position(position.with_y(f64::from(height))))
                },
            ),
        ))
}

pub(super) fn rotated_operation() -> Builder {
    literal("rotated")
        .then(
            argument("rot", SteelArgumentType::rotation()).redirects_with(
                EXECUTE_ROOT,
                |context| {
                    let rotation = required_coordinates(context, "rot")?.rotation(context.source());
                    Ok(context.source().with_rotation(rotation))
                },
            ),
        )
        .then(
            literal("as").then(argument("targets", SteelArgumentType::entities()).forks(
                EXECUTE_ROOT,
                |context| {
                    Ok(context
                        .optional_entities("targets")?
                        .into_iter()
                        .map(|entity| context.source().with_rotation(entity.rotation()))
                        .collect())
                },
            )),
        )
}

pub(super) fn facing_operation() -> Builder {
    literal("facing")
        .then(
            literal("entity").then(argument("targets", SteelArgumentType::entities()).then(
                argument("anchor", SteelArgumentType::entity_anchor()).forks(
                    EXECUTE_ROOT,
                    |context| {
                        let anchor = context.entity_anchor("anchor")?;
                        Ok(context
                            .optional_entities("targets")?
                            .into_iter()
                            .map(|entity| {
                                context
                                    .source()
                                    .facing_position(anchor.position(entity.as_ref()))
                            })
                            .collect())
                    },
                ),
            )),
        )
        .then(
            argument("pos", SteelArgumentType::vec3(true)).redirects_with(
                EXECUTE_ROOT,
                |context| {
                    let position = required_coordinates(context, "pos")?.position(context.source());
                    Ok(context.source().facing_position(position))
                },
            ),
        )
}

pub(super) fn align_operation() -> Builder {
    literal("align").then(
        argument("axes", SteelArgumentType::swizzle()).redirects_with(
            EXECUTE_ROOT,
            |context: &SteelCommandContext<CommandSource>| {
                let axes = context.swizzle("axes")?;
                Ok(context
                    .source()
                    .with_position(axes.align(context.source().position())))
            },
        ),
    )
}

pub(super) fn anchored_operation() -> Builder {
    literal("anchored").then(
        argument("anchor", SteelArgumentType::entity_anchor()).redirects_with(
            EXECUTE_ROOT,
            |context: &SteelCommandContext<CommandSource>| {
                let anchor = context.entity_anchor("anchor")?;
                Ok(context.source().with_anchor(anchor))
            },
        ),
    )
}

pub(super) fn in_operation() -> Builder {
    literal("in").then(
        argument("dimension", SteelArgumentType::world()).redirects_with(
            EXECUTE_ROOT,
            |context: &SteelCommandContext<CommandSource>| {
                let world = context
                    .world_argument("dimension")?
                    .resolve(context.source())?;
                context.source().with_world(world)
            },
        ),
    )
}

pub(super) fn summon_operation() -> Builder {
    literal("summon").then(
        argument("entity", SteelArgumentType::summonable_entity()).redirects_with(
            EXECUTE_ROOT,
            |context| {
                let entity_type = context.entity_type("entity")?;
                let entity =
                    summon::create_entity(context, entity_type, context.source().position())?;
                Ok(context.source().with_entity(entity))
            },
        ),
    )
}

pub(super) fn on_relations() -> Builder {
    literal("on")
        .then(relation_operation("owner", EntityRelation::Owner))
        .then(relation_operation("leasher", EntityRelation::Leasher))
        .then(relation_operation("target", EntityRelation::Target))
        .then(relation_operation("attacker", EntityRelation::Attacker))
        .then(relation_operation("vehicle", EntityRelation::Vehicle))
        .then(relation_operation("controller", EntityRelation::Controller))
        .then(relation_operation("origin", EntityRelation::Origin))
        .then(literal("passengers").forks(EXECUTE_ROOT, |context| {
            Ok(passenger_sources(context.source()))
        }))
}

/// One vanilla `/execute on <relation>` fork over
/// [`EntityRelation::resolve`].
fn relation_operation(name: &'static str, relation: EntityRelation) -> Builder {
    literal(name).forks(EXECUTE_ROOT, move |context| {
        Ok(one_relation_sources(context.source(), relation))
    })
}

/// Vanilla's one-to-one entity relations from
/// `ExecuteCommand.createRelationOperations`.
#[derive(Clone, Copy)]
enum EntityRelation {
    /// Vanilla `e instanceof OwnableEntity ? getOwner()`: the taming owner.
    Owner,
    /// Vanilla `e instanceof Leashable ? getLeashHolder()`: the leash holder.
    Leasher,
    /// Vanilla `e instanceof Targeting ? getTarget()`: the mob's attack target.
    Target,
    /// Vanilla `e instanceof Attackable ? getLastAttacker()`: the last
    /// attacker inside the vanilla 100-tick window.
    Attacker,
    /// Vanilla `e.getVehicle()`.
    Vehicle,
    /// Vanilla `e.getControllingPassenger()`.
    Controller,
    /// Vanilla `e instanceof TraceableEntity ? getOwner()`: the entity's
    /// spawner/owner (e.g. a projectile's shooter).
    Origin,
}

impl EntityRelation {
    /// Resolves the relation target of `caller`, mirroring each vanilla
    /// `expandOneToOneEntityRelation` lambda.
    fn resolve(self, caller: &SharedEntity) -> Option<SharedEntity> {
        match self {
            Self::Owner => caller.as_tameable().and_then(Tameable::owner),
            Self::Leasher => caller.as_leashable().and_then(Leashable::leash_holder),
            Self::Target => caller.as_mob().and_then(Mob::target),
            Self::Attacker => caller
                .as_living_entity()
                .and_then(LivingEntity::last_hurt_by_mob),
            Self::Vehicle => caller.vehicle(),
            Self::Controller => caller.controlling_passenger(),
            Self::Origin => caller.projectile_owner(),
        }
    }
}

/// Vanilla `expandOneToOneEntityRelation`: the relation target of the
/// source's entity when it exists and is not removed.
fn one_relation_sources(source: &CommandSource, relation: EntityRelation) -> Vec<CommandSource> {
    related_entity(source.entity(), relation)
        .map_or_else(Vec::new, |entity| vec![source.with_entity(entity)])
}

/// The testable half of [`one_relation_sources`]: select the related entity
/// without forking a command source.
fn related_entity(caller: Option<&SharedEntity>, relation: EntityRelation) -> Option<SharedEntity> {
    let related = caller.and_then(|entity| relation.resolve(entity))?;
    (!related.is_removed()).then_some(related)
}

fn passenger_sources(source: &CommandSource) -> Vec<CommandSource> {
    source.entity().map_or_else(Vec::new, |entity| {
        entity
            .passengers()
            .into_iter()
            .filter(|passenger| !passenger.is_removed())
            .map(|passenger| source.with_entity(passenger))
            .collect()
    })
}

fn required_coordinates(
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<super::super::super::execution::Coordinates, CommandSyntaxError> {
    context.coordinates(name)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;
    use crate::entity::RemovalReason;
    use crate::entity::ai::goal::Tameable;
    use crate::entity::entities::PigEntity;
    use crate::entity::{Entity, EntityBase};
    use steel_registry::entity_type::EntityTypeRef;

    /// Minimal [`Tameable`] for the `owner` resolver plus a canned
    /// `TraceableEntity` for `origin`.
    struct TameTestEntity {
        base: EntityBase,
        relation_owner: Option<SharedEntity>,
    }

    impl TameTestEntity {
        fn shared(relation_owner: Option<SharedEntity>) -> SharedEntity {
            Arc::new(Self {
                base: EntityBase::new(
                    7,
                    DVec3::ZERO,
                    vanilla_entities::WOLF.dimensions,
                    Weak::new(),
                ),
                relation_owner,
            })
        }
    }

    crate::entity::impl_test_downcast_type!(TameTestEntity);

    impl Entity for TameTestEntity {
        fn base(&self) -> &EntityBase {
            &self.base
        }

        fn entity_type(&self) -> EntityTypeRef {
            &vanilla_entities::WOLF
        }

        fn projectile_owner(&self) -> Option<SharedEntity> {
            self.relation_owner.clone()
        }
    }

    impl Tameable for TameTestEntity {
        fn owner(&self) -> Option<SharedEntity> {
            self.relation_owner.clone()
        }

        fn is_tame(&self) -> bool {
            self.relation_owner.is_some()
        }

        fn is_ordered_to_sit(&self) -> bool {
            false
        }

        fn set_in_sitting_pose(&self, _sitting: bool) {}

        fn is_in_sitting_pose(&self) -> bool {
            false
        }

        fn unable_to_move_to_owner(&self) -> bool {
            false
        }

        fn should_try_teleport_to_owner(&self) -> bool {
            false
        }

        fn try_to_teleport_to_owner(&self) {}

        fn wants_to_attack(&self, _target: &SharedEntity, _owner: &SharedEntity) -> bool {
            false
        }
    }

    #[test]
    fn owner_and_origin_resolvers_select_the_stored_entity() {
        init_vanilla_registry();
        let other = test_pig(8);
        let caller = TameTestEntity::shared(Some(Arc::clone(&other)));

        let owner = related_entity(Some(&caller), EntityRelation::Owner)
            .expect("tame caller should resolve its owner");
        assert_ne!(caller.uuid(), owner.uuid());
        assert_eq!(owner.uuid(), other.uuid());

        let origin = related_entity(Some(&caller), EntityRelation::Origin)
            .expect("traceable caller should resolve its owner");
        assert_eq!(origin.uuid(), other.uuid());

        let untraced = TameTestEntity::shared(None);
        assert!(related_entity(Some(&untraced), EntityRelation::Owner).is_none());
        assert!(related_entity(Some(&untraced), EntityRelation::Origin).is_none());
        assert!(related_entity(Some(&test_pig(9)), EntityRelation::Owner).is_none());
    }

    #[test]
    fn relation_resolvers_select_the_related_entity_not_the_caller() {
        init_vanilla_registry();
        let owner = test_pig(101);
        let caller = test_pig(102);

        assert!(
            caller
                .as_leashable()
                .expect("pig should be leashable")
                .set_leashed_to(&owner)
        );
        let leasher = related_entity(Some(&caller), EntityRelation::Leasher)
            .expect("leashed caller should have a holder");
        assert_ne!(leasher.uuid(), caller.uuid());
        assert_eq!(leasher.uuid(), owner.uuid());

        assert!(
            caller
                .as_mob()
                .expect("pig should be a mob")
                .set_target(Some(&owner))
        );
        let target = related_entity(Some(&caller), EntityRelation::Target)
            .expect("caller with a target should resolve it");
        assert_ne!(target.uuid(), caller.uuid());
        assert_eq!(target.uuid(), owner.uuid());

        caller
            .as_living_entity()
            .expect("pig should be living")
            .set_last_hurt_by_mob(Some(&owner));
        let attacker = related_entity(Some(&caller), EntityRelation::Attacker)
            .expect("hurt caller should resolve its attacker");
        assert_ne!(attacker.uuid(), caller.uuid());
        assert_eq!(attacker.uuid(), owner.uuid());

        caller
            .as_living_entity()
            .expect("pig should be living")
            .set_last_hurt_by_mob(None);
        assert!(related_entity(Some(&caller), EntityRelation::Attacker).is_none());
        assert!(related_entity(None, EntityRelation::Attacker).is_none());
        assert!(related_entity(Some(&owner), EntityRelation::Leasher).is_none());
        assert!(related_entity(Some(&owner), EntityRelation::Target).is_none());

        let removed = related_entity(Some(&caller), EntityRelation::Leasher)
            .expect("leashed caller should have a holder for removal");
        owner.set_removed(RemovalReason::Discarded);
        assert!(related_entity(Some(&caller), EntityRelation::Leasher).is_none());
        let _ = removed;
    }

    #[test]
    fn leash_holder_helpers_report_the_holder_of_relation_entities() {
        init_vanilla_registry();
        let holder = test_pig(4);
        let leashable = test_pig(5);
        leashable
            .as_leashable()
            .expect("pig should be leashable")
            .set_leashed_to(&holder);

        let leasher = related_entity(Some(&leashable), EntityRelation::Leasher)
            .expect("leashed test pig should resolve its holder");
        assert_eq!(holder.uuid(), leasher.uuid());
        assert_ne!(leashable.uuid(), leasher.uuid());
    }

    fn test_pig(id: i32) -> SharedEntity {
        Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            id,
            DVec3::ZERO,
            Weak::new(),
        ))
    }
}

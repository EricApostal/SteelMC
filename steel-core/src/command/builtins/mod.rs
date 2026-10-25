//! Steel-owned built-in command declarations.

pub mod advancement;
mod attribute;
mod ban;
mod bossbar;
mod chase;
mod clear;
mod clone;
mod compute;
mod damage;
mod data;
mod datapack;
mod debug;
mod defaultgamemode;
mod dialog;
mod difficulty;
mod domain;
mod effect;
mod enchant;
mod execute;
mod experience;
mod fetchprofile;
mod fill;
mod fillbiome;
mod fly;
mod forceload;
mod function;
pub(crate) mod gamemode;
mod gamerule;
mod give;
mod help;
mod invsee;
mod item;
mod jfr;
mod kick;
mod kill;
mod list;
mod locate;
mod loot;
mod me;
mod msg;
mod operator;
mod particle;
mod perf;
mod perms;
mod place;
mod playsound;
mod posteffect;
mod raid;
mod random;
mod recipe;
mod reload;
mod return_command;
mod ride;
mod rotate;
mod save;
mod say;
mod schedule;
mod scoreboard;
mod seed;
mod setblock;
mod setidletimeout;
mod setworldspawn;
mod spawnpoint;
mod spectate;
mod spreadplayers;
mod stop;
mod stopsound;
mod stopwatch;
mod summon;
mod swing;
mod tag;
mod team;
mod teammsg;
mod teleport;
mod tellraw;
mod tick;
mod time;
mod title;
mod transfer;
mod trigger;
mod version;
mod waypoint;
mod weather;
mod whitelist;
mod worldborder;

pub(crate) use difficulty::player_can_change_difficulty;

use super::{
    CommandRegistry,
    execution::CommandSource,
    registration::{
        CommandDispatcherBuilder, CommandRegistration, CommandRegistrationError,
        ENTITY_SELECTOR_ADVANCED_PERMISSION_KEY, ENTITY_SELECTOR_PERMISSION_KEY,
        RegisteredCommandDispatcher,
    },
};
#[cfg(test)]
use super::{brigadier::CommandDispatcher, execution::SteelCommandRuntime};

#[cfg(test)]
pub(crate) fn create_dispatcher()
-> Result<CommandDispatcher<CommandSource, SteelCommandRuntime>, CommandRegistrationError> {
    create_registered_dispatcher(CommandRegistry::new()).map(|registered| registered.dispatcher)
}

/// Every built-in command declaration, in graph order.
///
/// This is the single source of truth for which built-ins exist: [`create_registered_dispatcher`]
/// builds the graph from it, and the graph-shape test derives its expected roots from it, so a
/// declaration cannot silently disagree with the dispatcher.
fn builtin_registrations()
-> Result<Vec<CommandRegistration<CommandSource>>, CommandRegistrationError> {
    Ok(vec![
        advancement::registration(),
        attribute::registration(),
        ban::registration(),
        ban::ban_ip_registration(),
        ban::ban_list_registration(),
        ban::pardon_registration(),
        ban::pardon_ip_registration(),
        bossbar::registration(),
        chase::registration(),
        clear::registration(),
        clone::registration(),
        compute::registration(),
        damage::registration(),
        data::registration(),
        datapack::registration(),
        debug::registration(),
        defaultgamemode::registration(),
        difficulty::registration(),
        dialog::registration(),
        domain::registration(),
        effect::registration(),
        enchant::registration(),
        execute::registration(),
        experience::registration(),
        fetchprofile::registration(),
        fill::registration(),
        fillbiome::registration(),
        fly::registration(),
        forceload::registration(),
        function::registration(),
        gamemode::registration()?,
        gamerule::registration(),
        give::registration(),
        help::registration(),
        invsee::registration()?,
        item::registration(),
        jfr::registration(),
        kick::registration(),
        kill::registration(),
        list::registration(),
        locate::registration(),
        loot::registration(),
        me::registration(),
        msg::registration(),
        operator::deop_registration(),
        operator::op_registration(),
        particle::registration(),
        perf::registration(),
        perms::registration(),
        place::registration(),
        playsound::registration(),
        posteffect::registration(),
        raid::registration(),
        random::registration(),
        recipe::registration(),
        reload::registration(),
        return_command::registration(),
        ride::registration(),
        rotate::registration(),
        save::registration(),
        save::save_off_registration(),
        save::save_on_registration(),
        say::registration(),
        schedule::registration(),
        scoreboard::registration(),
        seed::registration(),
        setblock::registration(),
        setidletimeout::registration(),
        setworldspawn::registration(),
        spawnpoint::registration(),
        spectate::registration(),
        spreadplayers::registration(),
        stop::registration(),
        stopsound::registration(),
        stopwatch::registration(),
        summon::registration(),
        swing::registration(),
        tag::registration(),
        team::registration(),
        teammsg::registration(),
        teleport::registration(),
        tellraw::registration(),
        tick::registration(),
        time::registration(),
        title::registration(),
        transfer::registration(),
        trigger::registration(),
        version::registration(),
        waypoint::registration(),
        weather::registration(),
        whitelist::registration(),
        worldborder::registration(),
    ])
}

pub(crate) fn create_registered_dispatcher(
    extension_commands: CommandRegistry,
) -> Result<RegisteredCommandDispatcher<CommandSource>, CommandRegistrationError> {
    let mut builder = CommandDispatcherBuilder::new();
    builder.declare_permission(ENTITY_SELECTOR_PERMISSION_KEY)?;
    builder.declare_permission(ENTITY_SELECTOR_ADVANCED_PERMISSION_KEY)?;
    builder.declare_permission(perms::MANAGE_ALL_PERMISSION)?;
    builder.declare_permission(perms::GROUP_ALL_PERMISSION)?;
    builder.declare_permission(perms::METADATA_PERMISSION)?;
    for registration in builtin_registrations()? {
        builder.register(registration)?;
    }
    builder.extend(extension_commands.into_inner())?;
    builder.build_with_permissions()
}

#[cfg(test)]
mod tests {
    use super::{builtin_registrations, create_dispatcher, create_registered_dispatcher};
    use crate::command::brigadier::ArgumentType;
    use crate::command::execution::SteelArgumentType;
    use crate::command::{
        CommandRegistration as ExtensionCommandRegistration, CommandRegistry,
        literal as extension_literal,
    };
    use crate::permission::PermissionKey;
    use steel_registry::init_vanilla_registry;
    use steel_utils::Identifier;

    #[test]
    fn first_builtin_slice_has_the_expected_graph_shape() {
        init_vanilla_registry();
        let Ok(registered) = create_registered_dispatcher(CommandRegistry::new()) else {
            panic!("built-in commands should register");
        };
        // The declared registrations are the source of truth for the roots, so adding or
        // removing a built-in updates the expectation with it instead of leaving a
        // hand-maintained copy behind to drift (`item` had already gone missing from one).
        let declared = builtin_registrations()
            .expect("built-in commands should declare")
            .iter()
            .flat_map(|registration| registration.root_names().map(ToOwned::to_owned))
            .collect::<Vec<_>>();
        assert!(
            declared.iter().all(|name| !name.is_empty()),
            "every declared command owns at least one root"
        );

        let dispatcher = registered.dispatcher;
        let Some(roots) = dispatcher.children(dispatcher.root()) else {
            panic!("dispatcher root should exist");
        };
        let names = roots
            .iter()
            .map(|root| {
                let Some(root) = dispatcher.node(*root) else {
                    panic!("registered root should exist");
                };
                root.name()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            names, declared,
            "the dispatcher must expose exactly the declared roots, in declaration order"
        );
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            names.len(),
            "no two registrations may claim the same root: {names:?}"
        );

        let Some(list) = roots.iter().copied().find(|root| {
            dispatcher
                .node(*root)
                .is_some_and(|node| node.name() == "list")
        }) else {
            panic!("list root should exist");
        };
        let Some(list_node) = dispatcher.node(list) else {
            panic!("list root should exist");
        };
        assert!(list_node.is_executable());
        let Some(list_children) = dispatcher.children(list) else {
            panic!("list children should exist");
        };
        assert_eq!(list_children.len(), 1);
        assert!(
            dispatcher
                .node(list_children[0])
                .is_some_and(|node| { node.name() == "uuids" && node.is_executable() })
        );

        let Some(weather) = roots.iter().copied().find(|root| {
            dispatcher
                .node(*root)
                .is_some_and(|node| node.name() == "weather")
        }) else {
            panic!("weather root should exist");
        };
        let Some(weather_children) = dispatcher.children(weather) else {
            panic!("weather children should exist");
        };
        let weather_names = weather_children
            .iter()
            .map(|child| {
                let Some(node) = dispatcher.node(*child) else {
                    panic!("weather literal should exist");
                };
                assert!(node.is_executable());
                let Some(duration_children) = dispatcher.children(*child) else {
                    panic!("weather duration child should exist");
                };
                assert_eq!(duration_children.len(), 1);
                let Some(duration) = dispatcher.node(duration_children[0]) else {
                    panic!("weather duration node should exist");
                };
                assert_eq!(duration.name(), "duration");
                assert!(duration.is_executable());
                assert_eq!(duration.argument_type(), Some(&SteelArgumentType::time(1)));
                node.name()
            })
            .collect::<Vec<_>>();
        assert_eq!(weather_names, ["clear", "rain", "thunder"]);
    }

    #[test]
    fn startup_extensions_merge_after_builtins_with_namespaced_collision_fallbacks() {
        init_vanilla_registry();
        let mut extensions = CommandRegistry::new();
        let registration =
            ExtensionCommandRegistration::new(Identifier::new("steel_test", "stop"), || {
                extension_literal("stop").executes(|_| Ok(1))
            });
        assert!(extensions.register(registration).is_ok());

        let Ok(registered) = create_registered_dispatcher(extensions) else {
            panic!("built-ins and extension should register atomically");
        };
        let dispatcher = registered.dispatcher;
        let Some(roots) = dispatcher.children(dispatcher.root()) else {
            panic!("dispatcher root should exist");
        };
        let names = roots
            .iter()
            .filter_map(|root| {
                let node = dispatcher.node(*root)?;
                Some(node.name())
            })
            .collect::<Vec<_>>();

        assert!(names.contains(&"stop"));
        assert!(names.contains(&"steel_test:stop"));
        assert!(
            registered
                .permissions
                .iter()
                .any(|permission| permission.as_str() == "steel_test.command.stop")
        );
    }

    #[test]
    fn invsee_discovers_only_steel_namespaced_permissions() {
        init_vanilla_registry();
        let Ok(registered) = create_registered_dispatcher(CommandRegistry::new()) else {
            panic!("built-in commands should register");
        };
        let permissions = registered
            .permissions
            .iter()
            .map(PermissionKey::as_str)
            .collect::<Vec<_>>();

        assert!(permissions.contains(&"steel.command.invsee"));
        assert!(permissions.contains(&"steel.command.invsee.modify"));
        assert!(!permissions.contains(&"minecraft.command.invsee"));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one graph-shape test keeps execute paths and redirects directly comparable"
    )]
    fn execute_graph_uses_expected_redirects_and_argument_types() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let Some(execute) = dispatcher.children(dispatcher.root()).and_then(|children| {
            children.iter().copied().find(|child| {
                dispatcher
                    .node(*child)
                    .is_some_and(|node| node.name() == "execute")
            })
        }) else {
            panic!("execute root should exist");
        };
        let Some(execute_node) = dispatcher.node(execute) else {
            panic!("execute root should exist");
        };
        assert!(!execute_node.is_executable());

        let child = |parent, name| {
            let Some(node) = dispatcher.children(parent).and_then(|children| {
                children.iter().copied().find(|child| {
                    dispatcher
                        .node(*child)
                        .is_some_and(|node| node.name() == name)
                })
            }) else {
                panic!("{name} should exist below {parent:?}");
            };
            node
        };

        let run = child(execute, "run");
        let Some(run_node) = dispatcher.node(run) else {
            panic!("execute run should exist");
        };
        assert_eq!(run_node.redirect(), Some(dispatcher.root()));

        for condition in ["if", "unless"] {
            for (path, expected_type) in [
                (
                    [condition, "entity", "entities"],
                    SteelArgumentType::entities(),
                ),
                ([condition, "loaded", "pos"], SteelArgumentType::block_pos()),
            ] {
                let terminal = path
                    .iter()
                    .fold(execute, |parent, name| child(parent, name));
                let Some(node) = dispatcher.node(terminal) else {
                    panic!("execute condition terminal should exist");
                };
                assert!(node.is_executable());
                assert_eq!(node.redirect(), Some(execute));
                assert_eq!(node.argument_type(), Some(&expected_type));
            }

            let biome = child(child(child(execute, condition), "biome"), "pos");
            let biome = child(biome, "biome");
            let Some(biome_node) = dispatcher.node(biome) else {
                panic!("execute biome condition terminal should exist");
            };
            assert!(biome_node.is_executable());
            assert_eq!(biome_node.redirect(), Some(execute));
            assert_eq!(
                biome_node.argument_type(),
                Some(&SteelArgumentType::biome_or_tag())
            );

            let block = child(child(child(execute, condition), "block"), "pos");
            let block = child(block, "block");
            let Some(block_node) = dispatcher.node(block) else {
                panic!("execute block condition terminal should exist");
            };
            assert!(block_node.is_executable());
            assert_eq!(block_node.redirect(), Some(execute));
            assert_eq!(
                block_node.argument_type(),
                Some(&SteelArgumentType::block_predicate())
            );

            let blocks = child(child(execute, condition), "blocks");
            let start = child(blocks, "start");
            let end = child(start, "end");
            let destination = child(end, "destination");
            for position in [start, end, destination] {
                assert_eq!(
                    dispatcher
                        .node(position)
                        .and_then(|node| node.argument_type()),
                    Some(&SteelArgumentType::block_pos())
                );
            }
            for mode in ["all", "masked"] {
                let terminal = child(destination, mode);
                let Some(node) = dispatcher.node(terminal) else {
                    panic!("execute blocks mode should exist");
                };
                assert!(node.is_executable());
                assert_eq!(node.redirect(), Some(execute));
            }

            let data = child(child(execute, condition), "data");
            for (provider, source_name, source_type) in [
                ("block", "sourcePos", SteelArgumentType::block_pos()),
                ("entity", "source", SteelArgumentType::entity()),
                ("storage", "source", SteelArgumentType::storage_key()),
            ] {
                let source = child(child(data, provider), source_name);
                assert_eq!(
                    dispatcher
                        .node(source)
                        .and_then(|node| node.argument_type()),
                    Some(&source_type)
                );
                let path = child(source, "path");
                let Some(node) = dispatcher.node(path) else {
                    panic!("execute data path terminal should exist");
                };
                assert!(node.is_executable());
                assert_eq!(node.redirect(), Some(execute));
                assert_eq!(node.argument_type(), Some(&SteelArgumentType::nbt_path()));
            }

            let dimension = child(child(execute, condition), "dimension");
            let dimension = child(dimension, "dimension");
            let Some(node) = dispatcher.node(dimension) else {
                panic!("execute dimension condition terminal should exist");
            };
            assert!(node.is_executable());
            assert_eq!(node.redirect(), Some(execute));
            assert_eq!(node.argument_type(), Some(&SteelArgumentType::world()));

            let score = child(child(execute, condition), "score");
            let target = child(score, "target");
            assert_eq!(
                dispatcher
                    .node(target)
                    .and_then(|node| node.argument_type()),
                Some(&SteelArgumentType::score_holder())
            );
            let target_objective = child(target, "targetObjective");
            assert_eq!(
                dispatcher
                    .node(target_objective)
                    .and_then(|node| node.argument_type()),
                Some(&SteelArgumentType::objective())
            );
            for comparison in ["=", "<", "<=", ">", ">="] {
                let source = child(child(target_objective, comparison), "source");
                let source_objective = child(source, "sourceObjective");
                let Some(node) = dispatcher.node(source_objective) else {
                    panic!("score comparison terminal should exist");
                };
                assert!(node.is_executable());
                assert_eq!(node.redirect(), Some(execute));
                assert_eq!(node.argument_type(), Some(&SteelArgumentType::objective()));
            }
            let range = child(child(target_objective, "matches"), "range");
            let Some(range_node) = dispatcher.node(range) else {
                panic!("score range terminal should exist");
            };
            assert!(range_node.is_executable());
            assert_eq!(range_node.redirect(), Some(execute));
            assert_eq!(
                range_node.argument_type(),
                Some(&SteelArgumentType::int_range())
            );
        }

        for store_kind in ["result", "success"] {
            let store = child(child(execute, "store"), store_kind);
            let targets = child(child(store, "score"), "targets");
            assert_eq!(
                dispatcher
                    .node(targets)
                    .and_then(|node| node.argument_type()),
                Some(&SteelArgumentType::score_holders())
            );
            let objective = child(targets, "objective");
            let Some(objective_node) = dispatcher.node(objective) else {
                panic!("execute store score objective should exist");
            };
            assert_eq!(objective_node.redirect(), Some(execute));
            assert_eq!(
                objective_node.argument_type(),
                Some(&SteelArgumentType::objective())
            );

            for (provider, target_name, target_type) in [
                ("block", "targetPos", SteelArgumentType::block_pos()),
                ("storage", "target", SteelArgumentType::storage_key()),
            ] {
                let target = child(child(store, provider), target_name);
                assert_eq!(
                    dispatcher
                        .node(target)
                        .and_then(|node| node.argument_type()),
                    Some(&target_type)
                );
                let path = child(target, "path");
                assert_eq!(
                    dispatcher.node(path).and_then(|node| node.argument_type()),
                    Some(&SteelArgumentType::nbt_path())
                );
                for data_type in ["int", "float", "short", "long", "double", "byte"] {
                    let scale = child(child(path, data_type), "scale");
                    let Some(scale_node) = dispatcher.node(scale) else {
                        panic!("execute store data scale should exist");
                    };
                    assert_eq!(scale_node.redirect(), Some(execute));
                    assert_eq!(
                        scale_node.argument_type(),
                        Some(&SteelArgumentType::from(ArgumentType::double(
                            f64::MIN,
                            f64::MAX,
                        )))
                    );
                }
            }
        }

        let modifier_paths: &[&[&str]] = &[
            &["as", "targets"],
            &["at", "targets"],
            &["positioned", "pos"],
            &["positioned", "as", "targets"],
            &["positioned", "over", "heightmap"],
            &["rotated", "rot"],
            &["rotated", "as", "targets"],
            &["facing", "pos"],
            &["facing", "entity", "targets", "anchor"],
            &["align", "axes"],
            &["anchored", "anchor"],
            &["in", "dimension"],
            &["summon", "entity"],
            &["on", "vehicle"],
            &["on", "controller"],
            &["on", "passengers"],
        ];
        for path in modifier_paths {
            let terminal = path
                .iter()
                .fold(execute, |parent, name| child(parent, name));
            let Some(terminal_node) = dispatcher.node(terminal) else {
                panic!("execute modifier terminal should exist");
            };
            assert_eq!(
                terminal_node.redirect(),
                Some(execute),
                "execute {} should redirect to the execute root",
                path.join(" ")
            );
        }

        let argument_types: &[(&[&str], SteelArgumentType)] = &[
            (&["as", "targets"], SteelArgumentType::entities()),
            (&["at", "targets"], SteelArgumentType::entities()),
            (&["positioned", "pos"], SteelArgumentType::vec3(true)),
            (
                &["positioned", "over", "heightmap"],
                SteelArgumentType::heightmap(),
            ),
            (&["rotated", "rot"], SteelArgumentType::rotation()),
            (&["facing", "pos"], SteelArgumentType::vec3(true)),
            (
                &["facing", "entity", "targets", "anchor"],
                SteelArgumentType::entity_anchor(),
            ),
            (&["align", "axes"], SteelArgumentType::swizzle()),
            (&["anchored", "anchor"], SteelArgumentType::entity_anchor()),
            (
                &["summon", "entity"],
                SteelArgumentType::summonable_entity(),
            ),
        ];
        for (path, expected) in argument_types {
            let argument = path
                .iter()
                .fold(execute, |parent, name| child(parent, name));
            assert_eq!(
                dispatcher
                    .node(argument)
                    .and_then(|node| node.argument_type()),
                Some(expected),
                "execute {} should use the expected argument parser",
                path.join(" ")
            );
        }
    }
}

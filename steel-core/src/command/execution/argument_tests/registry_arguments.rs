//! Vanilla brigade of registry-backed and descriptor-string command arguments.

use super::*;
use crate::command::execution::argument::{
    Angle, AngleValue, ColumnPosValue, FloatNumberProviderArg, FloatNumberProviderValue,
    IntNumberProviderArg, IntNumberProviderValue, MessageValue, Operation,
};
use steel_registry::{
    data_components::vanilla_components::SwingAnimationType, loot_table::LootContext,
    vanilla_attributes, vanilla_mob_effects,
};
use steel_utils::{Identifier, rotation::Rotation};
use steel_worldgen::structure::StructureMirror;

fn parsed_value<T: DowncastType>(
    chain: &crate::command::brigadier::ContextChain<TestSource, SteelCommandRuntime>,
) -> Result<&T, CommandSyntaxError> {
    chain
        .top_context()
        .argument("value")?
        .downcast_ref::<T>()
        .ok_or_else(|| {
            CommandSyntaxError::dynamic(TextComponent::plain("unexpected argument value"))
        })
}

fn parse_value(
    dispatcher: &TestDispatcher,
    command: &str,
) -> crate::command::brigadier::ContextChain<TestSource, SteelCommandRuntime> {
    let parse = dispatcher.parse(command, TestSource::new());
    match dispatcher.context_chain(parse) {
        Ok(chain) => chain,
        Err(error) => panic!("{command} should parse: {error}"),
    }
}

#[test]
fn angle_argument_retains_relative_yaw_and_wraps() {
    let dispatcher = resource_dispatcher(SteelArgumentType::angle());

    for (input, value, relative) in [
        ("resource 90", 90.0f32, false),
        ("resource ~-5", -5.0f32, true),
        ("resource ~", 0.0f32, true),
    ] {
        let chain = parse_value(&dispatcher, input);
        let Ok(parsed) = parsed_value::<AngleValue>(&chain) else {
            panic!("{input} should retain an angle value");
        };
        assert_eq!(parsed.0, Angle::new(value, relative));
    }

    // Vanilla `Mth.wrapDegrees`: the relative yaw is added to the source yaw and wrapped.
    assert_eq!(Angle::new(90.0, false).resolve(0.0), 90.0);
    assert_eq!(Angle::new(-5.0, true).resolve(10.0), 5.0);
    assert_eq!(Angle::new(270.0, false).resolve(0.0), -90.0);
    assert_eq!(Angle::new(180.0, false).resolve(0.0), -180.0);

    for input in ["resource", "resource nan", "resource inf"] {
        let parse = dispatcher.parse(input, TestSource::new());
        assert!(
            dispatcher.context_chain(parse).is_err(),
            "{input} should reject an incomplete or non-finite angle"
        );
    }
}

#[test]
fn message_argument_retains_greedy_text_and_selectors() {
    let dispatcher = resource_dispatcher(SteelArgumentType::message());

    let chain = parse_value(&dispatcher, "resource Hello @e world");
    let Ok(message) = parsed_value::<MessageValue>(&chain) else {
        panic!("message should be retained");
    };
    assert_eq!(message.0.text(), "Hello @e world");
    assert_eq!(message.0.parts().len(), 1);
    assert_eq!(message.0.parts()[0].start(), 6);
    assert_eq!(message.0.parts()[0].end(), 8);

    // An `@` without a selector type stays literal text, like vanilla.
    let chain = parse_value(&dispatcher, "resource mail@zample");
    let Ok(message) = parsed_value::<MessageValue>(&chain) else {
        panic!("message should be retained");
    };
    assert!(message.0.parts().is_empty());
    assert_eq!(message.0.text(), "mail@zample");

    let too_long = format!("resource {}", "a".repeat(257));
    let parse = dispatcher.parse(&too_long, TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());
}

#[test]
fn hex_color_argument_parses_compact_and_full_forms() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::hex_color());

    for (input, expected) in [
        ("resource F00", 0xFFFF_0000u32 as i32),
        ("resource FF0000", 0xFFFF_0000u32 as i32),
        ("resource 00ff00", 0xFF00_FF00u32 as i32),
    ] {
        let chain = parse_value(&dispatcher, input);
        assert_eq!(chain.top_context().hex_color("value"), Ok(expected));
    }

    let parse = dispatcher.parse("resource F0", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());
}

#[test]
fn mob_effect_argument_resolves_registry_entries_and_suggests() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::mob_effect());

    let chain = parse_value(&dispatcher, "resource minecraft:speed");
    assert_eq!(
        chain.top_context().mob_effect("value"),
        Ok(&*vanilla_mob_effects::SPEED)
    );

    let parse = dispatcher.parse("resource missing_effect", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());

    let parse = dispatcher.parse("resource speed", TestSource::new());
    let Ok(suggestions) = dispatcher.completion_suggestions(&parse) else {
        panic!("mob effect suggestions should build");
    };
    assert!(
        suggestions
            .list()
            .iter()
            .any(|suggestion| suggestion.text() == "minecraft:speed")
    );
}

#[test]
fn attribute_argument_resolves_registry_entries_and_suggests() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::attribute());

    let chain = parse_value(&dispatcher, "resource minecraft:movement_speed");
    assert_eq!(
        chain.top_context().attribute("value"),
        Ok(&*vanilla_attributes::MOVEMENT_SPEED)
    );

    let parse = dispatcher.parse("resource missing_attribute", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());
}

#[test]
fn operation_argument_parses_vanilla_symbols_and_suggests_all() {
    let dispatcher = resource_dispatcher(SteelArgumentType::operation());

    for input in [
        "resource =",
        "resource +=",
        "resource -=",
        "resource *=",
        "resource /=",
        "resource %=",
        "resource ><",
        "resource <",
        "resource >",
    ] {
        let chain = parse_value(&dispatcher, input);
        assert!(chain.top_context().operation("value").is_ok());
    }

    let chain = parse_value(&dispatcher, "resource ><");
    assert_eq!(chain.top_context().operation("value"), Ok(Operation::Swap));
    assert!(Operation::Swap.is_swap());
    assert!(!Operation::Add.is_swap());

    for (left, right, symbol, expected) in [
        (7, 3, "=", 3),
        (7, 3, "+=", 10),
        (7, 3, "-=", 4),
        (7, 3, "*=", 21),
        // Vanilla `floorDiv` rounds toward negative infinity.
        (-7, 3, "/=", -3),
        (-7, 3, "%=", 2),
        (7, 3, "<", 3),
        (7, 3, ">", 7),
    ] {
        let chain = parse_value(&dispatcher, &format!("resource {symbol}"));
        let Ok(operation) = chain.top_context().operation("value") else {
            panic!("operation should be retained");
        };
        assert_eq!(operation.apply_values(left, right), Ok(expected));
    }

    for symbol in ["/=", "%="] {
        let chain = parse_value(&dispatcher, &format!("resource {symbol}"));
        let Ok(operation) = chain.top_context().operation("value") else {
            panic!("operation should be retained");
        };
        assert!(operation.apply_values(1, 0).is_err());
    }

    let parse = dispatcher.parse("resource !!", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());

    let parse = dispatcher.parse("resource ", TestSource::new());
    let Ok(suggestions) = dispatcher.completion_suggestions(&parse) else {
        panic!("operation suggestions should build");
    };
    assert_eq!(suggestions.list().len(), 9);
}

#[test]
fn objective_criteria_argument_validates_builtin_stat_and_team_names() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::objective_criteria());

    for input in [
        "resource dummy",
        "resource health",
        "resource teamkill.red",
        "resource killedByTeam.blue",
        "resource minecraft.mined:minecraft.diamond_ore",
    ] {
        let chain = parse_value(&dispatcher, input);
        assert!(chain.top_context().objective_criteria("value").is_ok());
    }

    for input in [
        "resource missing",
        "resource teamkill.missing",
        "resource minecraft.missing:minecraft.diamond_ore",
    ] {
        let parse = dispatcher.parse(input, TestSource::new());
        assert!(
            dispatcher.context_chain(parse).is_err(),
            "{input} should reject an unknown criteria"
        );
    }

    let parse = dispatcher.parse("resource team", TestSource::new());
    let Ok(suggestions) = dispatcher.completion_suggestions(&parse) else {
        panic!("criteria suggestions should build");
    };
    assert!(
        suggestions
            .list()
            .iter()
            .any(|suggestion| suggestion.text() == "teamkill.red")
    );
}

#[test]
fn scoreboard_slot_argument_retains_vanilla_slots_and_suggests() {
    let dispatcher = resource_dispatcher(SteelArgumentType::scoreboard_slot());

    let chain = parse_value(&dispatcher, "resource sidebar");
    let Ok(slot) = chain.top_context().scoreboard_slot("value") else {
        panic!("display slot should parse");
    };
    use crate::scoreboard::DisplaySlot;
    assert_eq!(slot, DisplaySlot::Sidebar);

    let chain = parse_value(&dispatcher, "resource sidebar.team.red");
    assert_eq!(
        chain.top_context().scoreboard_slot("value"),
        Ok(DisplaySlot::SidebarTeamRed)
    );

    let parse = dispatcher.parse("resource missing", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());

    let parse = dispatcher.parse("resource sidebar.team.b", TestSource::new());
    let Ok(suggestions) = dispatcher.completion_suggestions(&parse) else {
        panic!("display slot suggestions should build");
    };
    assert!(
        suggestions
            .list()
            .iter()
            .any(|suggestion| suggestion.text() == "sidebar.team.blue")
    );
}

#[test]
fn team_and_team_color_arguments_parse_vanilla_names() {
    let dispatcher = resource_dispatcher(SteelArgumentType::team_color());

    let chain = parse_value(&dispatcher, "resource red");
    use crate::scoreboard::{DisplaySlot, TeamColor};
    assert_eq!(chain.top_context().team_color("value"), Ok(TeamColor::Red));
    assert_eq!(TeamColor::Red.rgb() & 0xFF_FFFF, 0xFF_5555);
    assert_eq!(TeamColor::Red.display_slot(), DisplaySlot::SidebarTeamRed);

    let parse = dispatcher.parse("resource missing", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());

    let parse = dispatcher.parse("resource r", TestSource::new());
    let Ok(suggestions) = dispatcher.completion_suggestions(&parse) else {
        panic!("team color suggestions should build");
    };
    assert!(
        suggestions
            .list()
            .iter()
            .any(|suggestion| suggestion.text() == "red")
    );
}

#[test]
fn uuid_argument_parses_hyphenated_values_and_rejects_garbage() {
    let dispatcher = resource_dispatcher(SteelArgumentType::uuid());

    let chain = parse_value(&dispatcher, "resource 123e4567-e89b-12d3-a456-426614174000");
    let Ok(uuid) = chain.top_context().uuid("value") else {
        panic!("UUID should parse");
    };
    assert_eq!(uuid.to_string(), "123e4567-e89b-12d3-a456-426614174000");

    for input in ["resource missing", "resource 123e4567-e89b"] {
        let parse = dispatcher.parse(input, TestSource::new());
        assert!(
            dispatcher.context_chain(parse).is_err(),
            "{input} should reject a malformed UUID"
        );
    }
}

#[test]
fn double_range_argument_retains_bounds_and_matches_floats() {
    let dispatcher = resource_dispatcher(SteelArgumentType::double_range());

    for (input, matches, misses) in [
        ("resource 5", 5.0, 4.0),
        ("resource -5.5..10.25", 0.0, 11.0),
        ("resource ..100.5", f64::MIN, 101.0),
        ("resource -5.5..", f64::MAX, -6.0),
    ] {
        let chain = parse_value(&dispatcher, input);
        let Ok(value) = chain.top_context().double_range("value") else {
            panic!("double range should be retained");
        };
        assert!(value.matches(matches));
        assert!(!value.matches(misses));
    }

    for input in ["resource ..", "resource 5..2", "resource 5...10"] {
        let parse = dispatcher.parse(input, TestSource::new());
        assert!(
            dispatcher.context_chain(parse).is_err(),
            "{input} should reject an invalid double range"
        );
    }
}

#[test]
fn nbt_tag_argument_retains_any_snbt_value() {
    use simdnbt::owned::NbtTag;

    fn is_int(tag: &NbtTag) -> bool {
        matches!(tag, NbtTag::Int(3))
    }
    fn is_string(tag: &NbtTag) -> bool {
        matches!(tag, NbtTag::String(_))
    }
    fn is_compound(tag: &NbtTag) -> bool {
        matches!(tag, NbtTag::Compound(_))
    }
    fn is_list(tag: &NbtTag) -> bool {
        matches!(tag, NbtTag::List(_))
    }

    let dispatcher = resource_dispatcher(SteelArgumentType::nbt_tag());
    let cases: [(&str, fn(&NbtTag) -> bool); 4] = [
        ("resource 3", is_int),
        ("resource \"text\"", is_string),
        ("resource {value:3b}", is_compound),
        ("resource [1,2]", is_list),
    ];

    for (input, check) in cases {
        let chain = parse_value(&dispatcher, input);
        let Ok(tag) = chain.top_context().nbt_tag("value") else {
            panic!("SNBT value should be retained");
        };
        assert!(check(tag), "{input} should parse as {tag:?}");
    }

    for input in ["resource {value:", "resource [1,"] {
        let parse = dispatcher.parse(input, TestSource::new());
        assert!(
            dispatcher.context_chain(parse).is_err(),
            "{input} should reject malformed SNBT"
        );
    }
}

#[test]
fn swing_animation_argument_parses_vanilla_names_and_suggests() {
    let dispatcher = resource_dispatcher(SteelArgumentType::swing_animation());

    for (input, expected) in [
        ("resource none", SwingAnimationType::None),
        ("resource whack", SwingAnimationType::Whack),
        ("resource stab", SwingAnimationType::Stab),
    ] {
        let chain = parse_value(&dispatcher, input);
        assert_eq!(chain.top_context().swing_animation("value"), Ok(expected));
    }

    let parse = dispatcher.parse("resource missing", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());

    let parse = dispatcher.parse("resource ", TestSource::new());
    let Ok(suggestions) = dispatcher.completion_suggestions(&parse) else {
        panic!("swing suggestions should build");
    };
    assert_eq!(suggestions.list().len(), 3);
}

#[test]
fn identifier_and_slots_arguments_parse_vanilla_names() {
    let dispatcher = resource_dispatcher(SteelArgumentType::identifier_argument());
    let chain = parse_value(&dispatcher, "resource foo:bar.baz-qux");
    let Ok(identifier) = chain.top_context().identifier_argument("value") else {
        panic!("identifier should parse");
    };
    assert_eq!(identifier.to_string(), "foo:bar.baz-qux");

    let dispatcher = resource_dispatcher(SteelArgumentType::slots());
    let chain = parse_value(&dispatcher, "resource container.5");
    let Ok(slots) = chain.top_context().slots("value") else {
        panic!("slot range should parse");
    };
    assert_eq!(slots.name(), "container.5");
    assert_eq!(slots.slots().to_vec(), vec![5]);

    let parse = dispatcher.parse("resource missing", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());

    let parse = dispatcher.parse("resource armor", TestSource::new());
    let Ok(suggestions) = dispatcher.completion_suggestions(&parse) else {
        panic!("slot suggestions should build");
    };
    assert!(
        suggestions
            .list()
            .iter()
            .any(|suggestion| suggestion.text() == "armor.head")
    );
}

#[test]
fn column_pos_argument_requires_two_integer_components() {
    let dispatcher = resource_dispatcher(SteelArgumentType::column_pos());

    for input in ["resource 1 2", "resource ~ ~", "resource ~1 ~-2"] {
        let chain = parse_value(&dispatcher, input);
        assert!(
            parsed_value::<ColumnPosValue>(&chain).is_ok(),
            "{input} should retain column coordinates"
        );
    }

    for input in ["resource 1", "resource 1 2 3", "resource ^ ^"] {
        let parse = dispatcher.parse(input, TestSource::new());
        assert!(
            dispatcher.context_chain(parse).is_err(),
            "{input} should reject a non-column coordinate"
        );
    }
}

#[test]
fn template_mirror_and_rotation_arguments_parse_vanilla_names() {
    let dispatcher = resource_dispatcher(SteelArgumentType::template_mirror());
    for (input, expected) in [
        ("resource none", StructureMirror::None),
        ("resource left_right", StructureMirror::LeftRight),
        ("resource front_back", StructureMirror::FrontBack),
    ] {
        let chain = parse_value(&dispatcher, input);
        assert_eq!(chain.top_context().template_mirror("value"), Ok(expected));
    }

    let dispatcher = resource_dispatcher(SteelArgumentType::template_rotation());
    for (input, expected) in [
        ("resource none", Rotation::None),
        ("resource clockwise_90", Rotation::Clockwise90),
        ("resource 180", Rotation::Clockwise180),
        ("resource counterclockwise_90", Rotation::CounterClockwise90),
    ] {
        let chain = parse_value(&dispatcher, input);
        assert_eq!(chain.top_context().template_rotation("value"), Ok(expected));
    }
}

#[test]
fn style_argument_parses_format_fields_and_rejects_unknown() {
    let dispatcher = resource_dispatcher(SteelArgumentType::style());

    let chain = parse_value(&dispatcher, "resource {bold:1b,color:\"red\"}");
    let Ok(style) = chain.top_context().style("value") else {
        panic!("style should parse");
    };
    assert_eq!(style.bold, Some(true));
    assert!(style.color.is_some());

    let parse = dispatcher.parse("resource {click_event:1b}", TestSource::new());
    assert!(dispatcher.context_chain(parse).is_err());
}

#[test]
fn new_arguments_serialize_with_vanilla_protocol_ids() {
    init_vanilla_registry();

    fn discriminant(argument: &SteelArgumentType) -> u8 {
        let (argument, _) = argument.protocol_argument();
        let mut encoded = Vec::new();
        assert!(argument.write(&mut encoded).is_ok());
        encoded[0]
    }

    for (argument, expected) in [
        (SteelArgumentType::angle(), 28u8),             // minecraft:angle
        (SteelArgumentType::message(), 20),             // minecraft:message
        (SteelArgumentType::hex_color(), 17),           // minecraft:hex_color
        (SteelArgumentType::objective_criteria(), 25),  // minecraft:objective_criteria
        (SteelArgumentType::operation(), 26),           // minecraft:operation
        (SteelArgumentType::scoreboard_slot(), 30),     // minecraft:scoreboard_slot
        (SteelArgumentType::team(), 33),                // minecraft:team
        (SteelArgumentType::team_color(), 16),          // minecraft:team_color
        (SteelArgumentType::column_pos(), 9),           // minecraft:column_pos
        (SteelArgumentType::double_range(), 40),        // minecraft:float_range
        (SteelArgumentType::nbt_tag(), 22),             // minecraft:nbt_tag
        (SteelArgumentType::style(), 19),               // minecraft:style
        (SteelArgumentType::slots(), 35),               // minecraft:item_slots
        (SteelArgumentType::template_mirror(), 49),     // minecraft:template_mirror
        (SteelArgumentType::template_rotation(), 50),   // minecraft:template_rotation
        (SteelArgumentType::identifier_argument(), 36), // minecraft:resource_location
        (SteelArgumentType::mob_effect(), 46),          // minecraft:resource
        (SteelArgumentType::attribute(), 46),           // minecraft:resource
        (SteelArgumentType::particle(), 27),            // minecraft:particle
        (SteelArgumentType::swing_animation(), 60),     // minecraft:swing_animation
    ] {
        assert_eq!(discriminant(&argument), expected);
    }

    let repeated = SteelArgumentType::angle();
    let other = SteelArgumentType::angle();
    assert_eq!(repeated, other);
}

#[test]
fn int_number_provider_argument_parses_constants_and_registry_entries() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::int_number_provider());

    let chain = parse_value(&dispatcher, "resource 7");
    let Ok(value) = parsed_value::<IntNumberProviderValue>(&chain) else {
        panic!("bare integer should parse as a constant int provider");
    };
    assert!(matches!(value.0, IntNumberProviderArg::Constant(7)));

    // `/compute integer default 7` parses as `minecraft:constant` (Steel keeps only
    // the value, since a bare number cannot name an entry).
    assert!(value.0.identifier().is_none());

    let chain = parse_value(&dispatcher, "resource minecraft:brewing/uses_default");
    let Ok(value) = parsed_value::<IntNumberProviderValue>(&chain) else {
        panic!("registered id should parse as a named int provider");
    };
    let IntNumberProviderArg::Named(entry) = &value.0 else {
        panic!("registered id should parse as a named int provider");
    };
    assert_eq!(
        entry.key,
        Identifier::vanilla_static("brewing/uses_default")
    );

    // The named provider evaluates against a bare context like vanilla's
    // `ContextIntProvider` holder.
    let mut rng = rand::rng();
    let mut context = LootContext::new(&mut rng);
    assert_eq!(value.0.get_int_unsafe(&mut context), Some(20));

    let parse = dispatcher.parse("resource minecraft:not_a_real_provider", TestSource::new());
    assert!(
        dispatcher.context_chain(parse).is_err(),
        "an unknown provider id should be an invalid-argument error"
    );
}

#[test]
fn float_number_provider_argument_parses_constants_and_registry_entries() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::float_number_provider());

    let chain = parse_value(&dispatcher, "resource 1.5");
    let Ok(value) = parsed_value::<FloatNumberProviderValue>(&chain) else {
        panic!("bare float should parse as a constant float provider");
    };
    assert!(matches!(value.0, FloatNumberProviderArg::Constant(1.5)));
    assert!(value.0.identifier().is_none());

    let chain = parse_value(&dispatcher, "resource minecraft:brewing/speed_default");
    let Ok(value) = parsed_value::<FloatNumberProviderValue>(&chain) else {
        panic!("registered id should parse as a named float provider");
    };
    let FloatNumberProviderArg::Named(entry) = &value.0 else {
        panic!("registered id should parse as a named float provider");
    };
    assert_eq!(
        entry.key,
        Identifier::vanilla_static("brewing/speed_default")
    );

    let mut rng = rand::rng();
    let mut context = LootContext::new(&mut rng);
    assert_eq!(value.0.get_float_unsafe(&mut context), 1.0);

    let parse = dispatcher.parse("resource minecraft:not_a_real_provider", TestSource::new());
    assert!(
        dispatcher.context_chain(parse).is_err(),
        "an unknown provider id should be an invalid-argument error"
    );
}

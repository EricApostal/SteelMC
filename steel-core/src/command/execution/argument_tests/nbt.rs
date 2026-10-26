//! Vanilla SNBT command arguments: `NbtTagArgument` values and
//! `CompoundTagArgument` compounds.
//!
//! The argument parsers sit on `steel_utils::nbt`'s packrat-style SNBT reader,
//! which mirrors `net.minecraft.nbt.TagParser` plus `SnbtGrammar`: quoted and
//! unquoted strings, every numeric suffix, byte/int/long arrays, nested
//! compounds and lists. These tests pin the command-visible surface of both
//! argument types at the brigadier layer.

use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

use super::*;
use crate::command::brigadier::ContextChain;

fn compound_value(
    chain: &ContextChain<TestSource, SteelCommandRuntime>,
) -> Result<&NbtCompound, CommandSyntaxError> {
    chain.top_context().nbt_compound("value")
}

#[test]
fn snbt_value_argument_parses_vanilla_tag_kinds() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::nbt_tag());

    for (input, check) in [
        ("resource 3b", NbtTag::Byte(3)),
        ("resource 3s", NbtTag::Short(3)),
        ("resource 3", NbtTag::Int(3)),
        ("resource 3l", NbtTag::Long(3)),
        ("resource 3.5f", NbtTag::Float(3.5)),
        ("resource 3.5", NbtTag::Double(3.5)),
        ("resource 3.5d", NbtTag::Double(3.5)),
        ("resource true", NbtTag::Byte(1)),
        ("resource \"text\"", NbtTag::String("text".into())),
    ] {
        let chain = parse_snbt_value(&dispatcher, input);
        let Ok(tag) = chain.top_context().nbt_tag("value") else {
            panic!("{input} should parse an SNBT value");
        };
        assert_eq!(*tag, check, "{input} should parse as {check:?}");
    }

    let chain = parse_snbt_value(&dispatcher, "resource {name:text,count:2b}");
    let Ok(tag) = chain.top_context().nbt_tag("value") else {
        panic!("unquoted compound keys and values should parse");
    };
    let NbtTag::Compound(compound) = tag else {
        panic!("compound literal should stay a compound");
    };
    assert_eq!(
        compound.string("name").map(ToString::to_string),
        Some("text".to_owned())
    );
    assert_eq!(compound.byte("count"), Some(2));

    let chain = parse_snbt_value(&dispatcher, "resource [[1,2],[3]]");
    let Ok(tag) = chain.top_context().nbt_tag("value") else {
        panic!("nested lists should parse");
    };
    assert!(matches!(tag, NbtTag::List(NbtList::List(_))));

    let chain = parse_snbt_value(&dispatcher, "resource [B;1b,2b]");
    let Ok(tag) = chain.top_context().nbt_tag("value") else {
        panic!("byte arrays should parse");
    };
    assert!(matches!(tag, NbtTag::ByteArray(_)));

    let chain = parse_snbt_value(&dispatcher, "resource [I;1,2]");
    let Ok(tag) = chain.top_context().nbt_tag("value") else {
        panic!("int arrays should parse");
    };
    assert!(matches!(tag, NbtTag::IntArray(_)));
}

#[test]
fn snbt_value_argument_rejects_malformed_input() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::nbt_tag());

    for input in [
        "resource {value:",
        "resource [1,",
        "resource \"unterminated",
        "resource {value:}",
    ] {
        let parse = dispatcher.parse(input, TestSource::new());
        assert!(
            dispatcher.context_chain(parse).is_err(),
            "{input} should reject malformed SNBT"
        );
    }
}

/// Vanilla `TagParser` (packrat grammar) accepts a trailing comma in a list,
/// typed array, or compound, so `[B;1b,]` parses instead of erroring.
#[test]
fn snbt_value_argument_accepts_trailing_comma() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::nbt_tag());

    let parse = dispatcher.parse("resource [B;1b,]", TestSource::new());
    dispatcher
        .context_chain(parse)
        .expect("vanilla accepts a trailing comma");
}

#[test]
fn snbt_compound_argument_accepts_compounds_only() {
    init_vanilla_registry();
    let dispatcher = resource_dispatcher(SteelArgumentType::nbt_compound());

    let chain = parse_snbt_value(&dispatcher, "resource {value:3}");
    let Ok(compound) = compound_value(&chain) else {
        panic!("compound literal should parse");
    };
    assert_eq!(compound.int("value"), Some(3));

    // Vanilla `TagParser.castToCompoundOrThrow`: any other tag parses as a
    // value first and then fails with `argument.nbt.expected.compound`.
    for input in ["resource 5", "resource \"text\"", "resource [1,2]"] {
        let parse = dispatcher.parse(input, TestSource::new());
        assert!(
            dispatcher.context_chain(parse).is_err(),
            "{input} should reject a non-compound with the compound argument"
        );
    }

    let parse = dispatcher.parse("resource {value:", TestSource::new());
    assert!(
        dispatcher.context_chain(parse).is_err(),
        "malformed compound should fail the compound argument"
    );
}

fn parse_snbt_value(
    dispatcher: &TestDispatcher,
    command: &str,
) -> ContextChain<TestSource, SteelCommandRuntime> {
    let parse = dispatcher.parse(command, TestSource::new());
    match dispatcher.context_chain(parse) {
        Ok(chain) => chain,
        Err(error) => panic!("{command} should parse: {error}"),
    }
}

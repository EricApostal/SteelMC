//! Vanilla `/help` command.
//!
//! Mirrors `HelpCommand`: without an argument the command lists the usage of
//! every root the source may run, and with a greedy `command` argument it
//! re-parses that input and lists the usage of the deepest node it reached.
//! Both forms send one `/usage` line per entry and return the number of lines.
//!
//! Brigadier's `CommandDispatcher.getSmartUsage` has no Steel counterpart yet,
//! so the same algorithm is implemented here over the live dispatcher graph.

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    CommandDispatcher,
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError, NodeId, NodeKind},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, argument, literal},
    registration::CommandRegistration,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("help"), |_| command()).default_access()
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("help")
        .executes(show_all)
        .then(argument("command", ArgumentType::greedy_string()).executes(show_command))
}

fn show_all(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let dispatcher = source.server().command_dispatcher();
    let usages = smart_usages(&dispatcher, dispatcher.root(), source);
    for usage in &usages {
        source.send_success(&TextComponent::plain(format!("/{usage}")), false);
    }
    result_count(usages.len())
}

fn show_command(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let input = context.string("command")?;
    let dispatcher = source.server().command_dispatcher();
    let parse = dispatcher.parse(input, source.clone());
    let Some(deepest) = parse.context().nodes().last() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_HELP_FAILED,
        )));
    };
    let usages = smart_usages(&dispatcher, deepest.node(), source);
    for usage in &usages {
        source.send_success(&TextComponent::plain(format!("/{input} {usage}")), false);
    }
    result_count(usages.len())
}

fn result_count(usages: usize) -> Result<i32, CommandSyntaxError> {
    i32::try_from(usages).map_err(|_| {
        CommandSyntaxError::dynamic("Command usage count exceeds the command result range")
    })
}

/// Mirrors `CommandDispatcher.getSmartUsage(node, source)`.
fn smart_usages(
    dispatcher: &CommandDispatcher,
    node: NodeId,
    source: &CommandSource,
) -> Vec<String> {
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the private CommandNode type cannot be named from this module"
    )]
    let inner = dispatcher
        .node(node)
        .is_some_and(|node| node.is_executable());
    let Some(children) = dispatcher.children(node) else {
        return Vec::new();
    };
    let children = children.to_vec();
    children
        .into_iter()
        .filter_map(|child| smart_usage(dispatcher, child, source, inner, false))
        .collect()
}

/// Mirrors Brigadier's private `CommandDispatcher.getSmartUsage(node, source, inner, optional)`.
fn smart_usage(
    dispatcher: &CommandDispatcher,
    node_id: NodeId,
    source: &CommandSource,
    inner: bool,
    optional: bool,
) -> Option<String> {
    let node = dispatcher.node(node_id)?;
    if !node.allows(source) {
        return None;
    }
    let node_usage = usage_text(dispatcher, node_id)?;
    let usage = if inner {
        format!("[{node_usage}]")
    } else {
        node_usage
    };
    let has_command = node.is_executable();
    let (open, close) = if has_command { ("[", "]") } else { ("(", ")") };
    if optional {
        return Some(usage);
    }

    if let Some(redirect) = node.redirect() {
        let target = if redirect == dispatcher.root() {
            "...".to_owned()
        } else {
            format!("->{}", usage_text(dispatcher, redirect)?)
        };
        return Some(format!("{usage} {target}"));
    }

    let children = dispatcher
        .children(node_id)?
        .iter()
        .copied()
        .filter(|child| {
            dispatcher
                .node(*child)
                .is_some_and(|node| node.allows(source))
        })
        .collect::<Vec<_>>();

    match children.as_slice() {
        [] => {}
        [only] => {
            if let Some(child_usage) =
                smart_usage(dispatcher, *only, source, has_command, has_command)
            {
                return Some(format!("{usage} {child_usage}"));
            }
        }
        _ => {
            let mut optional_usages = Vec::new();
            for child in &children {
                if let Some(child_usage) =
                    smart_usage(dispatcher, *child, source, has_command, true)
                    && !optional_usages.contains(&child_usage)
                {
                    optional_usages.push(child_usage);
                }
            }
            if let [only] = optional_usages.as_slice() {
                return Some(if has_command {
                    format!("{usage} [{only}]")
                } else {
                    format!("{usage} {only}")
                });
            }
            if optional_usages.len() > 1 {
                let mut combined = String::from(open);
                let mut count = 0;
                for child in &children {
                    if count > 0 {
                        combined.push('|');
                    }
                    combined.push_str(&usage_text(dispatcher, *child)?);
                    count += 1;
                }
                if count > 0 {
                    combined.push_str(close);
                    return Some(format!("{usage} {combined}"));
                }
            }
        }
    }

    Some(usage)
}

/// Mirrors Brigadier's `CommandNode.getUsageText`.
fn usage_text(dispatcher: &CommandDispatcher, node_id: NodeId) -> Option<String> {
    let node = dispatcher.node(node_id)?;
    Some(match node.kind() {
        NodeKind::Root => String::new(),
        NodeKind::Literal => node.name().to_owned(),
        NodeKind::Argument => format!("<{}>", node.name()),
    })
}

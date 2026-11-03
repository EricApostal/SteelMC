//! Generated Dart host API over the Steel C ABI.
//!
//! Plugin code calls typed methods (`Player.position`, `World.spawn`,
//! `Server.findPlayer`, ...); the `NativePluginHost` implementation in
//! `clients/dart` maps those methods to `SteelInterface` function pointers.

/// Generated output path for the Dart host API.
pub const PLUGIN_API_PATH: &str = "clients/dart/lib/src/plugin/generated_api.dart";

struct Method {
    name: &'static str,
    params: &'static str,
    return_type: &'static str,
    body: &'static str,
    returns_value: bool,
    doc: &'static str,
}

struct Class {
    name: &'static str,
    doc: &'static str,
    fields: &'static str,
    constructor: &'static str,
    extra: &'static str,
    methods: &'static [Method],
}

const PLAYER_METHODS: &[Method] = &[
    Method {
        name: "sendMessage",
        params: "String message",
        return_type: "void",
        body: "_host.playerSendMessage(world, entity, message)",
        returns_value: false,
        doc: "Sends a chat message to this player.",
    },
    Method {
        name: "teleport",
        params: "Vec3 position, {double yaw = 0, double pitch = 0}",
        return_type: "void",
        body: "_host.entityTeleport(world, entity, position, yaw: yaw, pitch: pitch)",
        returns_value: false,
        doc: "Teleports this player to [position].",
    },
    Method {
        name: "position",
        params: "",
        return_type: "Vec3",
        body: "_host.entityPosition(world, entity)",
        returns_value: true,
        doc: "Returns the player's current position.",
    },
    Method {
        name: "health",
        params: "",
        return_type: "double",
        body: "_host.entityHealth(world, entity)",
        returns_value: true,
        doc: "Returns the player's current health.",
    },
    Method {
        name: "setHealth",
        params: "double health",
        return_type: "void",
        body: "_host.entitySetHealth(world, entity, health)",
        returns_value: false,
        doc: "Sets the player's health.",
    },
];

const WORLD_METHODS: &[Method] = &[
    Method {
        name: "spawn",
        params: "",
        return_type: "Vec3",
        body: "_host.worldSpawn(id)",
        returns_value: true,
        doc: "Returns the world's spawn position.",
    },
    Method {
        name: "players",
        params: "",
        return_type: "List<Player>",
        body: "_host.worldPlayers(id)",
        returns_value: true,
        doc: "Returns every player currently in this world.",
    },
];

const SERVER_METHODS: &[Method] = &[Method {
    name: "findPlayer",
    params: "String name, {int world = 0}",
    return_type: "Player?",
    body: "_host.findPlayerInWorld(world, name)",
    returns_value: true,
    doc: "Finds an online player by name in [world].",
}];

const CLASSES: &[Class] = &[
    Class {
        name: "Player",
        doc: "A player as described by the host.\n///\n/// The descriptor fields are a snapshot. Every operation is performed through\n/// the host, so a [Player] stays useful for as long as the player is online.",
        fields: "  /// Entity id within [world].\n  final int entity;\n\n  /// World id the player is currently in.\n  final int world;\n\n  /// Display name.\n  final String name;\n\n  /// Persistent UUID, used for storage such as player homes.\n  final String uuid;",
        constructor: "  Player({\n    required this.entity,\n    required this.world,\n    required this.name,\n    required this.uuid,\n    required PluginHost host,\n  }) : _host = host;",
        extra: r#"  @override
  bool operator ==(Object other) =>
      other is Player &&
      other.entity == entity &&
      other.world == world &&
      other.uuid == uuid;

  @override
  int get hashCode => Object.hash(entity, world, uuid);

  @override
  String toString() => 'Player($name, entity: $entity, world: $world)';"#,
        methods: PLAYER_METHODS,
    },
    Class {
        name: "World",
        doc: "An addressable loaded world.",
        fields: "  /// Host world id.\n  final int id;",
        constructor: "  World(this._host, this.id);",
        extra: "",
        methods: WORLD_METHODS,
    },
    Class {
        name: "Server",
        doc: "Server-wide queries.",
        fields: "",
        constructor: "  Server(this._host);",
        extra: "",
        methods: SERVER_METHODS,
    },
];

fn emit_method(method: &Method) -> String {
    let signature = if method.params.is_empty() {
        format!("{}()", method.name)
    } else {
        format!("{}({})", method.name, method.params)
    };
    if method.returns_value {
        format!(
            "  /// {}\n  {} {} => {};\n",
            method.doc, method.return_type, signature, method.body
        )
    } else {
        format!(
            "  /// {}\n  {} {} {{\n    {};\n  }}\n",
            method.doc, method.return_type, signature, method.body
        )
    }
}

fn emit_class(class: &Class) -> String {
    let mut output = format!("/// {}\nfinal class {} {{\n", class.doc, class.name);
    output.push_str(class.constructor);
    output.push_str("\n\n");
    if !class.fields.is_empty() {
        output.push_str(class.fields);
        output.push_str("\n\n");
    }
    output.push_str("  final PluginHost _host;\n\n");
    if !class.extra.is_empty() {
        output.push_str(class.extra);
        output.push_str("\n\n");
    }
    for method in class.methods {
        output.push_str(&emit_method(method));
        output.push('\n');
    }
    output.push_str("}\n");
    output
}

/// Renders `clients/dart/lib/src/plugin/generated_api.dart`.
#[must_use]
pub fn emit_plugin_api() -> String {
    let mut output =
        String::from("// Generated by steel-plugin-gen. Do not edit.\n\nimport 'model.dart';\n\n");
    for class in CLASSES {
        output.push_str(&emit_class(class));
        output.push('\n');
    }
    output
}

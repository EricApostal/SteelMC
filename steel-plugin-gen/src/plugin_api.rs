//! Generated Dart host API for the out-of-process plugin runtime.
//!
//! The wire protocol lives in the Rust bridge and the Dart `protocol.dart`
//! models. This module is the single source of truth for the typed convenience
//! methods (`Player.position`, `World.spawn`, `Server.findPlayer`, ...) so
//! plugin code never writes `host.call('...')` by hand.

/// Generated output path for the Dart host API.
pub const PLUGIN_API_PATH: &str = "clients/dart/lib/src/plugin/generated_api.dart";

struct Method {
    name: &'static str,
    wire: &'static str,
    params: &'static str,
    return_type: &'static str,
    body: &'static str,
    wrap: Wrap,
    doc: &'static str,
}

enum Wrap {
    Void,
    Vec3,
    Double,
    PlayerOpt,
    PlayerList,
}

struct Class {
    name: &'static str,
    doc: &'static str,
    fields: &'static str,
    constructor: &'static str,
    target: Option<&'static str>,
    extra: &'static str,
    methods: &'static [Method],
}

const PLAYER_METHODS: &[Method] = &[
    Method {
        name: "sendMessage",
        wire: "player.send_message",
        params: "String message",
        return_type: "void",
        body: "{..._target, 'message': message}",
        wrap: Wrap::Void,
        doc: "Sends a chat message to this player.",
    },
    Method {
        name: "teleport",
        wire: "player.teleport",
        params: "Vec3 position, {double yaw = 0, double pitch = 0}",
        return_type: "void",
        body: "{..._target, ...position.toMap(), 'yaw': yaw, 'pitch': pitch}",
        wrap: Wrap::Void,
        doc: "Teleports this player to [position].",
    },
    Method {
        name: "position",
        wire: "player.position",
        params: "",
        return_type: "Vec3",
        body: "_target",
        wrap: Wrap::Vec3,
        doc: "Returns the player's current position.",
    },
    Method {
        name: "health",
        wire: "player.health",
        params: "",
        return_type: "double",
        body: "_target",
        wrap: Wrap::Double,
        doc: "Returns the player's current health.",
    },
    Method {
        name: "setHealth",
        wire: "player.set_health",
        params: "double health",
        return_type: "void",
        body: "{..._target, 'health': health}",
        wrap: Wrap::Void,
        doc: "Sets the player's health.",
    },
];

const WORLD_METHODS: &[Method] = &[
    Method {
        name: "spawn",
        wire: "world.spawn",
        params: "",
        return_type: "Vec3",
        body: "_target",
        wrap: Wrap::Vec3,
        doc: "Returns the world's spawn position.",
    },
    Method {
        name: "players",
        wire: "world.players",
        params: "",
        return_type: "List<Player>",
        body: "_target",
        wrap: Wrap::PlayerList,
        doc: "Returns every player currently in this world.",
    },
];

const SERVER_METHODS: &[Method] = &[Method {
    name: "findPlayer",
    wire: "server.find_player",
    params: "String name, {int world = 0}",
    return_type: "Player?",
    body: "{'world': world, 'name': name}",
    wrap: Wrap::PlayerOpt,
    doc: "Finds an online player by name in [world].",
}];

const CLASSES: &[Class] = &[
    Class {
        name: "Player",
        doc: "A player as described by the host.\n///\n/// The descriptor fields are a snapshot. Every operation is performed through\n/// the host, so a [Player] stays useful for as long as the player is online.",
        fields: "  /// Entity id within [world].\n  final int entity;\n\n  /// World id the player is currently in.\n  final int world;\n\n  /// Display name.\n  final String name;\n\n  /// Persistent UUID, used for storage such as player homes.\n  final String uuid;",
        constructor: "  Player({\n    required this.entity,\n    required this.world,\n    required this.name,\n    required this.uuid,\n    required PluginHost host,\n  }) : _host = host;",
        target: Some("  Map<String, Object?> get _target =>\n      <String, Object?>{'entity': entity, 'world': world};"),
        extra: r#"  /// Decodes a player from a host payload.
  factory Player.fromData(PlayerData data, PluginHost host) => Player(
        entity: data.entity,
        world: data.world,
        name: data.name,
        uuid: data.uuid,
        host: host,
      );

  /// Decodes a player from a JSON object.
  factory Player.fromMap(Map<String, Object?> json, PluginHost host) =>
      Player.fromData(PlayerDataMapper.fromMap(json.cast<String, dynamic>()), host);

  /// Decodes a player from a decoded host reply.
  factory Player.fromResult(
    Object? result,
    PluginHost host, {
    String context = 'player',
  }) =>
      Player.fromMap(asJsonObject(result, context: context), host);

  @override
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
        target: Some("  Map<String, Object?> get _target =>\n      <String, Object?>{'world': id};"),
        extra: "",
        methods: WORLD_METHODS,
    },
    Class {
        name: "Server",
        doc: "Server-wide queries.",
        fields: "",
        constructor: "  Server(this._host);",
        target: None,
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
    let call = format!("_host.call('{}', {})", method.wire, method.body);
    let doc = format!("  /// {}\n", method.doc);
    match method.wrap {
        Wrap::Void => format!(
            "{doc}  {} {} {{\n    {call};\n  }}\n",
            method.return_type, signature
        ),
        Wrap::Vec3 => format!(
            "{doc}  {} {} =>\n      Vec3Mapper.fromMap(asJsonObject({call}, context: '{}'));\n",
            method.return_type, signature, method.wire
        ),
        Wrap::Double => format!(
            "{doc}  {} {} {{\n    final result = {call};\n    if (result is num) {{\n      return result.toDouble();\n    }}\n    throw FormatException(\n      'Expected {} to return a number, got ${{result.runtimeType}}',\n    );\n  }}\n",
            method.return_type, signature, method.wire
        ),
        Wrap::PlayerOpt => format!(
            "{doc}  {} {} {{\n    final result = {call};\n    if (result == null) return null;\n    return Player.fromResult(result, _host, context: '{}');\n  }}\n",
            method.return_type, signature, method.wire
        ),
        Wrap::PlayerList => format!(
            "{doc}  {} {} => <Player>[\n        for (final entry in asJsonList({call}, context: '{}'))\n          Player.fromResult(entry, _host, context: '{}'),\n      ];\n",
            method.return_type, signature, method.wire, method.wire
        ),
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
    if let Some(target) = class.target {
        output.push_str(target);
        output.push_str("\n\n");
    }
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
    let mut output = String::from(
        "// Generated by steel-plugin-gen. Do not edit.\n\nimport 'json.dart';\nimport 'model.dart';\nimport 'protocol.dart';\n\n",
    );
    for class in CLASSES {
        output.push_str(&emit_class(class));
        output.push('\n');
    }
    output
}

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

import '../bindings.g.dart';
import 'model.dart';

/// Thrown when a host ABI call fails.
final class SteelHostException implements Exception {
  SteelHostException(this.message);
  final String message;

  @override
  String toString() => 'SteelHostException: $message';
}

typedef _EntityGetPosition = int Function(SteelEntityRef, Pointer<SteelVec3>);
typedef _EntityTeleport = int Function(
    SteelEntityRef, SteelVec3, double, double);
typedef _EntityGetHealth = int Function(SteelEntityRef, Pointer<Float>);
typedef _EntitySetHealth = int Function(SteelEntityRef, double);
typedef _EntityGetName = int Function(SteelEntityRef, Pointer<SteelStrView>);
typedef _EntityGetUuid = int Function(SteelEntityRef, Pointer<SteelStrView>);
typedef _PlayerSendMessage = int Function(SteelEntityRef, SteelStrView);
typedef _WorldGetSpawn = int Function(SteelWorldRef, Pointer<SteelVec3>);
typedef _WorldPlayerCount = int Function(SteelWorldRef);
typedef _WorldPlayerAt = int Function(
    SteelWorldRef, int, Pointer<SteelOptEntityRef>);
typedef _WorldFindPlayer = int Function(
    SteelWorldRef, SteelStrView, Pointer<SteelOptEntityRef>);
typedef _ExtensionPath = SteelStrView Function();

typedef _ShimRegisterCommandNative = Int32 Function(
    Uint64, Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>);
typedef _ShimRegisterCommandDart = int Function(
    int, Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>);
typedef _ShimRegisterEventNative = Int32 Function(Uint32);
typedef _ShimRegisterEventDart = int Function(int);
typedef _ShimLogNative = Int32 Function(Uint32, Pointer<Utf8>);
typedef _ShimLogDart = int Function(int, Pointer<Utf8>);

final class _OwnedStrView {
  _OwnedStrView(this.view, this.bytes);

  final Pointer<SteelStrView> view;
  final Pointer<Uint8> bytes;

  SteelStrView get value => view.ref;

  void free() {
    malloc.free(bytes);
    calloc.free(view);
  }
}

/// [PluginHost] backed directly by the `SteelInterface` C function table.
final class NativePluginHost implements PluginHost {
  NativePluginHost._(this._interface, this._shim);

  /// Builds a host from the interface address passed to `steelSetInterface`.
  factory NativePluginHost.fromAddress(int address) {
    if (address == 0) {
      throw SteelHostException('Steel interface address is not set');
    }
    final interface = Pointer<SteelInterface>.fromAddress(address);
    final path = _extensionPath(interface);
    final libraryPath = path.isNotEmpty
        ? path
        : Platform.environment['STEEL_DART_SHIM'];
    if (libraryPath == null || libraryPath.isEmpty) {
      throw SteelHostException(
        'extension_path is empty and STEEL_DART_SHIM is not set',
      );
    }
    final shim = DynamicLibrary.open(libraryPath);
    final host = NativePluginHost._(interface, shim);
    host._registerCommand = shim.lookupFunction<
        _ShimRegisterCommandNative, _ShimRegisterCommandDart>(
      'steel_shim_register_command',
    );
    host._registerEvent = shim.lookupFunction<
        _ShimRegisterEventNative, _ShimRegisterEventDart>(
      'steel_shim_register_event',
    );
    host._log = shim.lookupFunction<_ShimLogNative, _ShimLogDart>(
      'steel_shim_log',
    );
    return host;
  }

  final Pointer<SteelInterface> _interface;
  // Keeps the shim library loaded for the lifetime of the host.
  // ignore: unused_field
  final DynamicLibrary _shim;
  late final _ShimRegisterCommandDart _registerCommand;
  late final _ShimRegisterEventDart _registerEvent;
  late final _ShimLogDart _log;

  static String _extensionPath(Pointer<SteelInterface> interface) {
    final function = interface.ref.extension_path;
    if (function == nullptr) {
      return '';
    }
    final view = function.asFunction<_ExtensionPath>()();
    return _readView(view);
  }

  static String _readView(SteelStrView view) {
    if (view.ptr == nullptr || view.len == 0) {
      return '';
    }
    return view.ptr.cast<Utf8>().toDartString(length: view.len);
  }

  T _withEntity<T>(int world, int entity, T Function(SteelEntityRef) body) {
    final pointer = calloc<SteelEntityRef>();
    try {
      pointer.ref.world.field0 = world;
      pointer.ref.entity = entity;
      return body(pointer.ref);
    } finally {
      calloc.free(pointer);
    }
  }

  T _withWorld<T>(int world, T Function(SteelWorldRef) body) {
    final pointer = calloc<SteelWorldRef>();
    try {
      pointer.ref.field0 = world;
      return body(pointer.ref);
    } finally {
      calloc.free(pointer);
    }
  }

  _OwnedStrView _string(String value) {
    final bytes = utf8.encode(value);
    final pointer = malloc<Uint8>(bytes.length);
    pointer.asTypedList(bytes.length).setAll(0, bytes);
    final view = calloc<SteelStrView>();
    view.ref.ptr = pointer;
    view.ref.len = bytes.length;
    return _OwnedStrView(view, pointer);
  }

  Never _fail(String operation, int status) {
    throw SteelHostException('$operation failed with status $status');
  }

  @override
  Player? playerData(int world, int entity) {
    return _withEntity(world, entity, (ref) {
      final name = _entityString(ref, name: true);
      final uuid = _entityString(ref, name: false);
      if (name == null || uuid == null) {
        return null;
      }
      return Player(
        entity: entity,
        world: world,
        name: name,
        uuid: uuid,
        host: this,
      );
    });
  }

  String? _entityString(SteelEntityRef entity, {required bool name}) {
    final function = name
        ? _interface.ref.entity_get_name
        : _interface.ref.entity_get_uuid;
    if (function == nullptr) {
      return null;
    }
    final out = calloc<SteelStrView>();
    try {
      final status = name
          ? function.asFunction<_EntityGetName>()(entity, out)
          : function.asFunction<_EntityGetUuid>()(entity, out);
      if (status != 0) {
        return null;
      }
      return _readView(out.ref);
    } finally {
      calloc.free(out);
    }
  }

  @override
  Vec3 entityPosition(int world, int entity) {
    final function = _interface.ref.entity_get_position;
    if (function == nullptr) {
      throw SteelHostException('entity_get_position is unavailable');
    }
    final out = calloc<SteelVec3>();
    try {
      final status = _withEntity(
        world,
        entity,
        (ref) => function.asFunction<_EntityGetPosition>()(ref, out),
      );
      if (status != 0) {
        _fail('entity_get_position', status);
      }
      return Vec3(out.ref.x, out.ref.y, out.ref.z);
    } finally {
      calloc.free(out);
    }
  }

  @override
  void entityTeleport(
    int world,
    int entity,
    Vec3 position, {
    double yaw = 0,
    double pitch = 0,
  }) {
    final function = _interface.ref.entity_teleport;
    if (function == nullptr) {
      throw SteelHostException('entity_teleport is unavailable');
    }
    final target = calloc<SteelVec3>();
    try {
      target.ref.x = position.x;
      target.ref.y = position.y;
      target.ref.z = position.z;
      final status = _withEntity(
        world,
        entity,
        (ref) => function.asFunction<_EntityTeleport>()(
          ref,
          target.ref,
          yaw,
          pitch,
        ),
      );
      if (status != 0) {
        _fail('entity_teleport', status);
      }
    } finally {
      calloc.free(target);
    }
  }

  @override
  void playerSendMessage(int world, int entity, String message) {
    final function = _interface.ref.player_send_message;
    if (function == nullptr) {
      throw SteelHostException('player_send_message is unavailable');
    }
    final text = _string(message);
    try {
      final status = _withEntity(
        world,
        entity,
        (ref) => function.asFunction<_PlayerSendMessage>()(ref, text.value),
      );
      if (status != 0) {
        _fail('player_send_message', status);
      }
    } finally {
      text.free();
    }
  }

  @override
  double entityHealth(int world, int entity) {
    final function = _interface.ref.entity_get_health;
    if (function == nullptr) {
      throw SteelHostException('entity_get_health is unavailable');
    }
    final out = calloc<Float>();
    try {
      final status = _withEntity(
        world,
        entity,
        (ref) => function.asFunction<_EntityGetHealth>()(ref, out),
      );
      if (status != 0) {
        _fail('entity_get_health', status);
      }
      return out.value;
    } finally {
      calloc.free(out);
    }
  }

  @override
  void entitySetHealth(int world, int entity, double health) {
    final function = _interface.ref.entity_set_health;
    if (function == nullptr) {
      throw SteelHostException('entity_set_health is unavailable');
    }
    final status = _withEntity(
      world,
      entity,
      (ref) => function.asFunction<_EntitySetHealth>()(ref, health),
    );
    if (status != 0) {
      _fail('entity_set_health', status);
    }
  }

  @override
  Vec3 worldSpawn(int world) {
    final function = _interface.ref.world_get_spawn;
    if (function == nullptr) {
      throw SteelHostException('world_get_spawn is unavailable');
    }
    final out = calloc<SteelVec3>();
    try {
      final status = _withWorld(
        world,
        (ref) => function.asFunction<_WorldGetSpawn>()(ref, out),
      );
      if (status != 0) {
        _fail('world_get_spawn', status);
      }
      return Vec3(out.ref.x, out.ref.y, out.ref.z);
    } finally {
      calloc.free(out);
    }
  }

  @override
  List<Player> worldPlayers(int world) {
    final countFunction = _interface.ref.world_player_count;
    final atFunction = _interface.ref.world_player_at;
    if (countFunction == nullptr || atFunction == nullptr) {
      throw SteelHostException('world player query is unavailable');
    }
    return _withWorld(world, (worldRef) {
      final count =
          countFunction.asFunction<_WorldPlayerCount>()(worldRef);
      final players = <Player>[];
      for (var index = 0; index < count; index++) {
        final out = calloc<SteelOptEntityRef>();
        try {
          final status = atFunction
              .asFunction<_WorldPlayerAt>()(worldRef, index, out);
          if (status == 0 && out.ref.present != 0) {
            final player = playerData(world, out.ref.value.entity);
            if (player != null) {
              players.add(player);
            }
          }
        } finally {
          calloc.free(out);
        }
      }
      return players;
    });
  }

  @override
  Player? findPlayerInWorld(int world, String name) {
    final function = _interface.ref.world_find_player;
    if (function == nullptr) {
      throw SteelHostException('world_find_player is unavailable');
    }
    final text = _string(name);
    final out = calloc<SteelOptEntityRef>();
    try {
      final status = _withWorld(
        world,
        (ref) => function.asFunction<_WorldFindPlayer>()(
          ref,
          text.value,
          out,
        ),
      );
      if (status != 0 || out.ref.present == 0) {
        return null;
      }
      return playerData(world, out.ref.value.entity);
    } finally {
      text.free();
      calloc.free(out);
    }
  }

  @override
  void log(String message, {LogLevel level = LogLevel.info}) {
    final pointer = message.toNativeUtf8();
    try {
      _log(level.index, pointer);
    } finally {
      malloc.free(pointer);
    }
  }

  @override
  void registerCommand(int id, CommandRegistration command) {
    final name = command.name.toNativeUtf8();
    final description = command.description.toNativeUtf8();
    final permission = (command.permission ?? '').toNativeUtf8();
    try {
      final status = _registerCommand(id, name, description, permission);
      if (status != 0) {
        _fail('register_command', status);
      }
    } finally {
      malloc.free(name);
      malloc.free(description);
      malloc.free(permission);
    }
  }

  @override
  void registerEvent(int kind) {
    final status = _registerEvent(kind);
    if (status != 0) {
      _fail('register_event', status);
    }
  }
}

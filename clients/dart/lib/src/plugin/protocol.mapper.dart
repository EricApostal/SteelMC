// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
// ignore_for_file: type=lint
// ignore_for_file: invalid_use_of_protected_member
// ignore_for_file: unused_element, unnecessary_cast, override_on_non_overriding_member
// ignore_for_file: strict_raw_type, inference_failure_on_untyped_parameter

part of 'protocol.dart';

class PlayerDataMapper extends ClassMapperBase<PlayerData> {
  PlayerDataMapper._();

  static PlayerDataMapper? _instance;
  static PlayerDataMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = PlayerDataMapper._());
    }
    return _instance!;
  }

  @override
  final String id = 'PlayerData';

  static int _$entity(PlayerData v) => v.entity;
  static const Field<PlayerData, int> _f$entity = Field('entity', _$entity);
  static int _$world(PlayerData v) => v.world;
  static const Field<PlayerData, int> _f$world = Field('world', _$world);
  static String _$name(PlayerData v) => v.name;
  static const Field<PlayerData, String> _f$name = Field('name', _$name);
  static String _$uuid(PlayerData v) => v.uuid;
  static const Field<PlayerData, String> _f$uuid = Field('uuid', _$uuid);

  @override
  final MappableFields<PlayerData> fields = const {
    #entity: _f$entity,
    #world: _f$world,
    #name: _f$name,
    #uuid: _f$uuid,
  };

  static PlayerData _instantiate(DecodingData data) {
    return PlayerData(
      entity: data.dec(_f$entity),
      world: data.dec(_f$world),
      name: data.dec(_f$name),
      uuid: data.dec(_f$uuid),
    );
  }

  @override
  final Function instantiate = _instantiate;

  static PlayerData fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<PlayerData>(map);
  }

  static PlayerData fromJson(String json) {
    return ensureInitialized().decodeJson<PlayerData>(json);
  }
}

mixin PlayerDataMappable {
  String toJson() {
    return PlayerDataMapper.ensureInitialized().encodeJson<PlayerData>(
      this as PlayerData,
    );
  }

  Map<String, dynamic> toMap() {
    return PlayerDataMapper.ensureInitialized().encodeMap<PlayerData>(
      this as PlayerData,
    );
  }

  PlayerDataCopyWith<PlayerData, PlayerData, PlayerData> get copyWith =>
      _PlayerDataCopyWithImpl<PlayerData, PlayerData>(
        this as PlayerData,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return PlayerDataMapper.ensureInitialized().stringifyValue(
      this as PlayerData,
    );
  }

  @override
  bool operator ==(Object other) {
    return PlayerDataMapper.ensureInitialized().equalsValue(
      this as PlayerData,
      other,
    );
  }

  @override
  int get hashCode {
    return PlayerDataMapper.ensureInitialized().hashValue(this as PlayerData);
  }
}

extension PlayerDataValueCopy<$R, $Out>
    on ObjectCopyWith<$R, PlayerData, $Out> {
  PlayerDataCopyWith<$R, PlayerData, $Out> get $asPlayerData =>
      $base.as((v, t, t2) => _PlayerDataCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class PlayerDataCopyWith<$R, $In extends PlayerData, $Out>
    implements ClassCopyWith<$R, $In, $Out> {
  $R call({int? entity, int? world, String? name, String? uuid});
  PlayerDataCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t);
}

class _PlayerDataCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, PlayerData, $Out>
    implements PlayerDataCopyWith<$R, PlayerData, $Out> {
  _PlayerDataCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<PlayerData> $mapper =
      PlayerDataMapper.ensureInitialized();
  @override
  $R call({int? entity, int? world, String? name, String? uuid}) => $apply(
    FieldCopyWithData({
      if (entity != null) #entity: entity,
      if (world != null) #world: world,
      if (name != null) #name: name,
      if (uuid != null) #uuid: uuid,
    }),
  );
  @override
  PlayerData $make(CopyWithData data) => PlayerData(
    entity: data.get(#entity, or: $value.entity),
    world: data.get(#world, or: $value.world),
    name: data.get(#name, or: $value.name),
    uuid: data.get(#uuid, or: $value.uuid),
  );

  @override
  PlayerDataCopyWith<$R2, PlayerData, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _PlayerDataCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class HostMessageMapper extends ClassMapperBase<HostMessage> {
  HostMessageMapper._();

  static HostMessageMapper? _instance;
  static HostMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = HostMessageMapper._());
      InitMessageMapper.ensureInitialized();
      CommandMessageMapper.ensureInitialized();
      EventMessageMapper.ensureInitialized();
      HostResultMessageMapper.ensureInitialized();
      HostErrorMessageMapper.ensureInitialized();
      ShutdownMessageMapper.ensureInitialized();
    }
    return _instance!;
  }

  @override
  final String id = 'HostMessage';

  @override
  final MappableFields<HostMessage> fields = const {};

  static HostMessage _instantiate(DecodingData data) {
    throw MapperException.missingSubclass(
      'HostMessage',
      't',
      '${data.value['t']}',
    );
  }

  @override
  final Function instantiate = _instantiate;

  static HostMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<HostMessage>(map);
  }

  static HostMessage fromJson(String json) {
    return ensureInitialized().decodeJson<HostMessage>(json);
  }
}

mixin HostMessageMappable {
  String toJson();
  Map<String, dynamic> toMap();
  HostMessageCopyWith<HostMessage, HostMessage, HostMessage> get copyWith;
}

abstract class HostMessageCopyWith<$R, $In extends HostMessage, $Out>
    implements ClassCopyWith<$R, $In, $Out> {
  $R call();
  HostMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t);
}

class InitMessageMapper extends SubClassMapperBase<InitMessage> {
  InitMessageMapper._();

  static InitMessageMapper? _instance;
  static InitMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = InitMessageMapper._());
      HostMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'InitMessage';

  static int _$level(InitMessage v) => v.level;
  static const Field<InitMessage, int> _f$level = Field('level', _$level);

  @override
  final MappableFields<InitMessage> fields = const {#level: _f$level};

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'init';
  @override
  late final ClassMapperBase superMapper =
      HostMessageMapper.ensureInitialized();

  static InitMessage _instantiate(DecodingData data) {
    return InitMessage(level: data.dec(_f$level));
  }

  @override
  final Function instantiate = _instantiate;

  static InitMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<InitMessage>(map);
  }

  static InitMessage fromJson(String json) {
    return ensureInitialized().decodeJson<InitMessage>(json);
  }
}

mixin InitMessageMappable {
  String toJson() {
    return InitMessageMapper.ensureInitialized().encodeJson<InitMessage>(
      this as InitMessage,
    );
  }

  Map<String, dynamic> toMap() {
    return InitMessageMapper.ensureInitialized().encodeMap<InitMessage>(
      this as InitMessage,
    );
  }

  InitMessageCopyWith<InitMessage, InitMessage, InitMessage> get copyWith =>
      _InitMessageCopyWithImpl<InitMessage, InitMessage>(
        this as InitMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return InitMessageMapper.ensureInitialized().stringifyValue(
      this as InitMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return InitMessageMapper.ensureInitialized().equalsValue(
      this as InitMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return InitMessageMapper.ensureInitialized().hashValue(this as InitMessage);
  }
}

extension InitMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, InitMessage, $Out> {
  InitMessageCopyWith<$R, InitMessage, $Out> get $asInitMessage =>
      $base.as((v, t, t2) => _InitMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class InitMessageCopyWith<$R, $In extends InitMessage, $Out>
    implements HostMessageCopyWith<$R, $In, $Out> {
  @override
  $R call({int? level});
  InitMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t);
}

class _InitMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, InitMessage, $Out>
    implements InitMessageCopyWith<$R, InitMessage, $Out> {
  _InitMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<InitMessage> $mapper =
      InitMessageMapper.ensureInitialized();
  @override
  $R call({int? level}) =>
      $apply(FieldCopyWithData({if (level != null) #level: level}));
  @override
  InitMessage $make(CopyWithData data) =>
      InitMessage(level: data.get(#level, or: $value.level));

  @override
  InitMessageCopyWith<$R2, InitMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _InitMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class CommandMessageMapper extends SubClassMapperBase<CommandMessage> {
  CommandMessageMapper._();

  static CommandMessageMapper? _instance;
  static CommandMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = CommandMessageMapper._());
      HostMessageMapper.ensureInitialized().addSubMapper(_instance!);
      PlayerDataMapper.ensureInitialized();
    }
    return _instance!;
  }

  @override
  final String id = 'CommandMessage';

  static int _$id(CommandMessage v) => v.id;
  static const Field<CommandMessage, int> _f$id = Field('id', _$id);
  static PlayerData? _$player(CommandMessage v) => v.player;
  static const Field<CommandMessage, PlayerData> _f$player = Field(
    'player',
    _$player,
    opt: true,
  );
  static String _$command(CommandMessage v) => v.command;
  static const Field<CommandMessage, String> _f$command = Field(
    'command',
    _$command,
  );
  static String _$args(CommandMessage v) => v.args;
  static const Field<CommandMessage, String> _f$args = Field('args', _$args);

  @override
  final MappableFields<CommandMessage> fields = const {
    #id: _f$id,
    #player: _f$player,
    #command: _f$command,
    #args: _f$args,
  };

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'command';
  @override
  late final ClassMapperBase superMapper =
      HostMessageMapper.ensureInitialized();

  static CommandMessage _instantiate(DecodingData data) {
    return CommandMessage(
      id: data.dec(_f$id),
      player: data.dec(_f$player),
      command: data.dec(_f$command),
      args: data.dec(_f$args),
    );
  }

  @override
  final Function instantiate = _instantiate;

  static CommandMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<CommandMessage>(map);
  }

  static CommandMessage fromJson(String json) {
    return ensureInitialized().decodeJson<CommandMessage>(json);
  }
}

mixin CommandMessageMappable {
  String toJson() {
    return CommandMessageMapper.ensureInitialized().encodeJson<CommandMessage>(
      this as CommandMessage,
    );
  }

  Map<String, dynamic> toMap() {
    return CommandMessageMapper.ensureInitialized().encodeMap<CommandMessage>(
      this as CommandMessage,
    );
  }

  CommandMessageCopyWith<CommandMessage, CommandMessage, CommandMessage>
  get copyWith => _CommandMessageCopyWithImpl<CommandMessage, CommandMessage>(
    this as CommandMessage,
    $identity,
    $identity,
  );
  @override
  String toString() {
    return CommandMessageMapper.ensureInitialized().stringifyValue(
      this as CommandMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return CommandMessageMapper.ensureInitialized().equalsValue(
      this as CommandMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return CommandMessageMapper.ensureInitialized().hashValue(
      this as CommandMessage,
    );
  }
}

extension CommandMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, CommandMessage, $Out> {
  CommandMessageCopyWith<$R, CommandMessage, $Out> get $asCommandMessage =>
      $base.as((v, t, t2) => _CommandMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class CommandMessageCopyWith<$R, $In extends CommandMessage, $Out>
    implements HostMessageCopyWith<$R, $In, $Out> {
  PlayerDataCopyWith<$R, PlayerData, PlayerData>? get player;
  @override
  $R call({int? id, PlayerData? player, String? command, String? args});
  CommandMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  );
}

class _CommandMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, CommandMessage, $Out>
    implements CommandMessageCopyWith<$R, CommandMessage, $Out> {
  _CommandMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<CommandMessage> $mapper =
      CommandMessageMapper.ensureInitialized();
  @override
  PlayerDataCopyWith<$R, PlayerData, PlayerData>? get player =>
      $value.player?.copyWith.$chain((v) => call(player: v));
  @override
  $R call({int? id, Object? player = $none, String? command, String? args}) =>
      $apply(
        FieldCopyWithData({
          if (id != null) #id: id,
          if (player != $none) #player: player,
          if (command != null) #command: command,
          if (args != null) #args: args,
        }),
      );
  @override
  CommandMessage $make(CopyWithData data) => CommandMessage(
    id: data.get(#id, or: $value.id),
    player: data.get(#player, or: $value.player),
    command: data.get(#command, or: $value.command),
    args: data.get(#args, or: $value.args),
  );

  @override
  CommandMessageCopyWith<$R2, CommandMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _CommandMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class EventMessageMapper extends SubClassMapperBase<EventMessage> {
  EventMessageMapper._();

  static EventMessageMapper? _instance;
  static EventMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = EventMessageMapper._());
      HostMessageMapper.ensureInitialized().addSubMapper(_instance!);
      PlayerDataMapper.ensureInitialized();
    }
    return _instance!;
  }

  @override
  final String id = 'EventMessage';

  static int _$id(EventMessage v) => v.id;
  static const Field<EventMessage, int> _f$id = Field('id', _$id);
  static String _$event(EventMessage v) => v.event;
  static const Field<EventMessage, String> _f$event = Field('event', _$event);
  static PlayerData? _$player(EventMessage v) => v.player;
  static const Field<EventMessage, PlayerData> _f$player = Field(
    'player',
    _$player,
    opt: true,
  );
  static int _$world(EventMessage v) => v.world;
  static const Field<EventMessage, int> _f$world = Field('world', _$world);
  static double _$x(EventMessage v) => v.x;
  static const Field<EventMessage, double> _f$x = Field('x', _$x);
  static double _$y(EventMessage v) => v.y;
  static const Field<EventMessage, double> _f$y = Field('y', _$y);
  static double _$z(EventMessage v) => v.z;
  static const Field<EventMessage, double> _f$z = Field('z', _$z);
  static int _$blockState(EventMessage v) => v.blockState;
  static const Field<EventMessage, int> _f$blockState = Field(
    'blockState',
    _$blockState,
    key: r'block_state',
  );
  static String _$text(EventMessage v) => v.text;
  static const Field<EventMessage, String> _f$text = Field('text', _$text);

  @override
  final MappableFields<EventMessage> fields = const {
    #id: _f$id,
    #event: _f$event,
    #player: _f$player,
    #world: _f$world,
    #x: _f$x,
    #y: _f$y,
    #z: _f$z,
    #blockState: _f$blockState,
    #text: _f$text,
  };

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'event';
  @override
  late final ClassMapperBase superMapper =
      HostMessageMapper.ensureInitialized();

  static EventMessage _instantiate(DecodingData data) {
    return EventMessage(
      id: data.dec(_f$id),
      event: data.dec(_f$event),
      player: data.dec(_f$player),
      world: data.dec(_f$world),
      x: data.dec(_f$x),
      y: data.dec(_f$y),
      z: data.dec(_f$z),
      blockState: data.dec(_f$blockState),
      text: data.dec(_f$text),
    );
  }

  @override
  final Function instantiate = _instantiate;

  static EventMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<EventMessage>(map);
  }

  static EventMessage fromJson(String json) {
    return ensureInitialized().decodeJson<EventMessage>(json);
  }
}

mixin EventMessageMappable {
  String toJson() {
    return EventMessageMapper.ensureInitialized().encodeJson<EventMessage>(
      this as EventMessage,
    );
  }

  Map<String, dynamic> toMap() {
    return EventMessageMapper.ensureInitialized().encodeMap<EventMessage>(
      this as EventMessage,
    );
  }

  EventMessageCopyWith<EventMessage, EventMessage, EventMessage> get copyWith =>
      _EventMessageCopyWithImpl<EventMessage, EventMessage>(
        this as EventMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return EventMessageMapper.ensureInitialized().stringifyValue(
      this as EventMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return EventMessageMapper.ensureInitialized().equalsValue(
      this as EventMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return EventMessageMapper.ensureInitialized().hashValue(
      this as EventMessage,
    );
  }
}

extension EventMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, EventMessage, $Out> {
  EventMessageCopyWith<$R, EventMessage, $Out> get $asEventMessage =>
      $base.as((v, t, t2) => _EventMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class EventMessageCopyWith<$R, $In extends EventMessage, $Out>
    implements HostMessageCopyWith<$R, $In, $Out> {
  PlayerDataCopyWith<$R, PlayerData, PlayerData>? get player;
  @override
  $R call({
    int? id,
    String? event,
    PlayerData? player,
    int? world,
    double? x,
    double? y,
    double? z,
    int? blockState,
    String? text,
  });
  EventMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t);
}

class _EventMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, EventMessage, $Out>
    implements EventMessageCopyWith<$R, EventMessage, $Out> {
  _EventMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<EventMessage> $mapper =
      EventMessageMapper.ensureInitialized();
  @override
  PlayerDataCopyWith<$R, PlayerData, PlayerData>? get player =>
      $value.player?.copyWith.$chain((v) => call(player: v));
  @override
  $R call({
    int? id,
    String? event,
    Object? player = $none,
    int? world,
    double? x,
    double? y,
    double? z,
    int? blockState,
    String? text,
  }) => $apply(
    FieldCopyWithData({
      if (id != null) #id: id,
      if (event != null) #event: event,
      if (player != $none) #player: player,
      if (world != null) #world: world,
      if (x != null) #x: x,
      if (y != null) #y: y,
      if (z != null) #z: z,
      if (blockState != null) #blockState: blockState,
      if (text != null) #text: text,
    }),
  );
  @override
  EventMessage $make(CopyWithData data) => EventMessage(
    id: data.get(#id, or: $value.id),
    event: data.get(#event, or: $value.event),
    player: data.get(#player, or: $value.player),
    world: data.get(#world, or: $value.world),
    x: data.get(#x, or: $value.x),
    y: data.get(#y, or: $value.y),
    z: data.get(#z, or: $value.z),
    blockState: data.get(#blockState, or: $value.blockState),
    text: data.get(#text, or: $value.text),
  );

  @override
  EventMessageCopyWith<$R2, EventMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _EventMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class HostResultMessageMapper extends SubClassMapperBase<HostResultMessage> {
  HostResultMessageMapper._();

  static HostResultMessageMapper? _instance;
  static HostResultMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = HostResultMessageMapper._());
      HostMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'HostResultMessage';

  static int _$id(HostResultMessage v) => v.id;
  static const Field<HostResultMessage, int> _f$id = Field('id', _$id);
  static bool _$ok(HostResultMessage v) => v.ok;
  static const Field<HostResultMessage, bool> _f$ok = Field('ok', _$ok);
  static Object? _$result(HostResultMessage v) => v.result;
  static const Field<HostResultMessage, Object> _f$result = Field(
    'result',
    _$result,
    opt: true,
  );

  @override
  final MappableFields<HostResultMessage> fields = const {
    #id: _f$id,
    #ok: _f$ok,
    #result: _f$result,
  };

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'host_result';
  @override
  late final ClassMapperBase superMapper =
      HostMessageMapper.ensureInitialized();

  static HostResultMessage _instantiate(DecodingData data) {
    return HostResultMessage(
      id: data.dec(_f$id),
      ok: data.dec(_f$ok),
      result: data.dec(_f$result),
    );
  }

  @override
  final Function instantiate = _instantiate;

  static HostResultMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<HostResultMessage>(map);
  }

  static HostResultMessage fromJson(String json) {
    return ensureInitialized().decodeJson<HostResultMessage>(json);
  }
}

mixin HostResultMessageMappable {
  String toJson() {
    return HostResultMessageMapper.ensureInitialized()
        .encodeJson<HostResultMessage>(this as HostResultMessage);
  }

  Map<String, dynamic> toMap() {
    return HostResultMessageMapper.ensureInitialized()
        .encodeMap<HostResultMessage>(this as HostResultMessage);
  }

  HostResultMessageCopyWith<
    HostResultMessage,
    HostResultMessage,
    HostResultMessage
  >
  get copyWith =>
      _HostResultMessageCopyWithImpl<HostResultMessage, HostResultMessage>(
        this as HostResultMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return HostResultMessageMapper.ensureInitialized().stringifyValue(
      this as HostResultMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return HostResultMessageMapper.ensureInitialized().equalsValue(
      this as HostResultMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return HostResultMessageMapper.ensureInitialized().hashValue(
      this as HostResultMessage,
    );
  }
}

extension HostResultMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, HostResultMessage, $Out> {
  HostResultMessageCopyWith<$R, HostResultMessage, $Out>
  get $asHostResultMessage => $base.as(
    (v, t, t2) => _HostResultMessageCopyWithImpl<$R, $Out>(v, t, t2),
  );
}

abstract class HostResultMessageCopyWith<
  $R,
  $In extends HostResultMessage,
  $Out
>
    implements HostMessageCopyWith<$R, $In, $Out> {
  @override
  $R call({int? id, bool? ok, Object? result});
  HostResultMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  );
}

class _HostResultMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, HostResultMessage, $Out>
    implements HostResultMessageCopyWith<$R, HostResultMessage, $Out> {
  _HostResultMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<HostResultMessage> $mapper =
      HostResultMessageMapper.ensureInitialized();
  @override
  $R call({int? id, bool? ok, Object? result = $none}) => $apply(
    FieldCopyWithData({
      if (id != null) #id: id,
      if (ok != null) #ok: ok,
      if (result != $none) #result: result,
    }),
  );
  @override
  HostResultMessage $make(CopyWithData data) => HostResultMessage(
    id: data.get(#id, or: $value.id),
    ok: data.get(#ok, or: $value.ok),
    result: data.get(#result, or: $value.result),
  );

  @override
  HostResultMessageCopyWith<$R2, HostResultMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _HostResultMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class HostErrorMessageMapper extends SubClassMapperBase<HostErrorMessage> {
  HostErrorMessageMapper._();

  static HostErrorMessageMapper? _instance;
  static HostErrorMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = HostErrorMessageMapper._());
      HostMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'HostErrorMessage';

  static int _$id(HostErrorMessage v) => v.id;
  static const Field<HostErrorMessage, int> _f$id = Field('id', _$id);
  static String _$error(HostErrorMessage v) => v.error;
  static const Field<HostErrorMessage, String> _f$error = Field(
    'error',
    _$error,
  );

  @override
  final MappableFields<HostErrorMessage> fields = const {
    #id: _f$id,
    #error: _f$error,
  };

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'host_error';
  @override
  late final ClassMapperBase superMapper =
      HostMessageMapper.ensureInitialized();

  static HostErrorMessage _instantiate(DecodingData data) {
    return HostErrorMessage(id: data.dec(_f$id), error: data.dec(_f$error));
  }

  @override
  final Function instantiate = _instantiate;

  static HostErrorMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<HostErrorMessage>(map);
  }

  static HostErrorMessage fromJson(String json) {
    return ensureInitialized().decodeJson<HostErrorMessage>(json);
  }
}

mixin HostErrorMessageMappable {
  String toJson() {
    return HostErrorMessageMapper.ensureInitialized()
        .encodeJson<HostErrorMessage>(this as HostErrorMessage);
  }

  Map<String, dynamic> toMap() {
    return HostErrorMessageMapper.ensureInitialized()
        .encodeMap<HostErrorMessage>(this as HostErrorMessage);
  }

  HostErrorMessageCopyWith<HostErrorMessage, HostErrorMessage, HostErrorMessage>
  get copyWith =>
      _HostErrorMessageCopyWithImpl<HostErrorMessage, HostErrorMessage>(
        this as HostErrorMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return HostErrorMessageMapper.ensureInitialized().stringifyValue(
      this as HostErrorMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return HostErrorMessageMapper.ensureInitialized().equalsValue(
      this as HostErrorMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return HostErrorMessageMapper.ensureInitialized().hashValue(
      this as HostErrorMessage,
    );
  }
}

extension HostErrorMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, HostErrorMessage, $Out> {
  HostErrorMessageCopyWith<$R, HostErrorMessage, $Out>
  get $asHostErrorMessage =>
      $base.as((v, t, t2) => _HostErrorMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class HostErrorMessageCopyWith<$R, $In extends HostErrorMessage, $Out>
    implements HostMessageCopyWith<$R, $In, $Out> {
  @override
  $R call({int? id, String? error});
  HostErrorMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  );
}

class _HostErrorMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, HostErrorMessage, $Out>
    implements HostErrorMessageCopyWith<$R, HostErrorMessage, $Out> {
  _HostErrorMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<HostErrorMessage> $mapper =
      HostErrorMessageMapper.ensureInitialized();
  @override
  $R call({int? id, String? error}) => $apply(
    FieldCopyWithData({
      if (id != null) #id: id,
      if (error != null) #error: error,
    }),
  );
  @override
  HostErrorMessage $make(CopyWithData data) => HostErrorMessage(
    id: data.get(#id, or: $value.id),
    error: data.get(#error, or: $value.error),
  );

  @override
  HostErrorMessageCopyWith<$R2, HostErrorMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _HostErrorMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class ShutdownMessageMapper extends SubClassMapperBase<ShutdownMessage> {
  ShutdownMessageMapper._();

  static ShutdownMessageMapper? _instance;
  static ShutdownMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = ShutdownMessageMapper._());
      HostMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'ShutdownMessage';

  @override
  final MappableFields<ShutdownMessage> fields = const {};

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'shutdown';
  @override
  late final ClassMapperBase superMapper =
      HostMessageMapper.ensureInitialized();

  static ShutdownMessage _instantiate(DecodingData data) {
    return ShutdownMessage();
  }

  @override
  final Function instantiate = _instantiate;

  static ShutdownMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<ShutdownMessage>(map);
  }

  static ShutdownMessage fromJson(String json) {
    return ensureInitialized().decodeJson<ShutdownMessage>(json);
  }
}

mixin ShutdownMessageMappable {
  String toJson() {
    return ShutdownMessageMapper.ensureInitialized()
        .encodeJson<ShutdownMessage>(this as ShutdownMessage);
  }

  Map<String, dynamic> toMap() {
    return ShutdownMessageMapper.ensureInitialized().encodeMap<ShutdownMessage>(
      this as ShutdownMessage,
    );
  }

  ShutdownMessageCopyWith<ShutdownMessage, ShutdownMessage, ShutdownMessage>
  get copyWith =>
      _ShutdownMessageCopyWithImpl<ShutdownMessage, ShutdownMessage>(
        this as ShutdownMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return ShutdownMessageMapper.ensureInitialized().stringifyValue(
      this as ShutdownMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return ShutdownMessageMapper.ensureInitialized().equalsValue(
      this as ShutdownMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return ShutdownMessageMapper.ensureInitialized().hashValue(
      this as ShutdownMessage,
    );
  }
}

extension ShutdownMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, ShutdownMessage, $Out> {
  ShutdownMessageCopyWith<$R, ShutdownMessage, $Out> get $asShutdownMessage =>
      $base.as((v, t, t2) => _ShutdownMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class ShutdownMessageCopyWith<$R, $In extends ShutdownMessage, $Out>
    implements HostMessageCopyWith<$R, $In, $Out> {
  @override
  $R call();
  ShutdownMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  );
}

class _ShutdownMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, ShutdownMessage, $Out>
    implements ShutdownMessageCopyWith<$R, ShutdownMessage, $Out> {
  _ShutdownMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<ShutdownMessage> $mapper =
      ShutdownMessageMapper.ensureInitialized();
  @override
  $R call() => $apply(FieldCopyWithData({}));
  @override
  ShutdownMessage $make(CopyWithData data) => ShutdownMessage();

  @override
  ShutdownMessageCopyWith<$R2, ShutdownMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _ShutdownMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class ClientMessageMapper extends ClassMapperBase<ClientMessage> {
  ClientMessageMapper._();

  static ClientMessageMapper? _instance;
  static ClientMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = ClientMessageMapper._());
      RegisterCommandMessageMapper.ensureInitialized();
      RegisterEventMessageMapper.ensureInitialized();
      HostCallMessageMapper.ensureInitialized();
      ResponseMessageMapper.ensureInitialized();
      LogMessageMapper.ensureInitialized();
      ReadyMessageMapper.ensureInitialized();
    }
    return _instance!;
  }

  @override
  final String id = 'ClientMessage';

  @override
  final MappableFields<ClientMessage> fields = const {};

  static ClientMessage _instantiate(DecodingData data) {
    throw MapperException.missingSubclass(
      'ClientMessage',
      't',
      '${data.value['t']}',
    );
  }

  @override
  final Function instantiate = _instantiate;

  static ClientMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<ClientMessage>(map);
  }

  static ClientMessage fromJson(String json) {
    return ensureInitialized().decodeJson<ClientMessage>(json);
  }
}

mixin ClientMessageMappable {
  String toJson();
  Map<String, dynamic> toMap();
  ClientMessageCopyWith<ClientMessage, ClientMessage, ClientMessage>
  get copyWith;
}

abstract class ClientMessageCopyWith<$R, $In extends ClientMessage, $Out>
    implements ClassCopyWith<$R, $In, $Out> {
  $R call();
  ClientMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t);
}

class RegisterCommandMessageMapper
    extends SubClassMapperBase<RegisterCommandMessage> {
  RegisterCommandMessageMapper._();

  static RegisterCommandMessageMapper? _instance;
  static RegisterCommandMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = RegisterCommandMessageMapper._());
      ClientMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'RegisterCommandMessage';

  static String _$name(RegisterCommandMessage v) => v.name;
  static const Field<RegisterCommandMessage, String> _f$name = Field(
    'name',
    _$name,
  );
  static String _$description(RegisterCommandMessage v) => v.description;
  static const Field<RegisterCommandMessage, String> _f$description = Field(
    'description',
    _$description,
  );
  static String? _$permission(RegisterCommandMessage v) => v.permission;
  static const Field<RegisterCommandMessage, String> _f$permission = Field(
    'permission',
    _$permission,
    opt: true,
  );

  @override
  final MappableFields<RegisterCommandMessage> fields = const {
    #name: _f$name,
    #description: _f$description,
    #permission: _f$permission,
  };

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'register_command';
  @override
  late final ClassMapperBase superMapper =
      ClientMessageMapper.ensureInitialized();

  static RegisterCommandMessage _instantiate(DecodingData data) {
    return RegisterCommandMessage(
      name: data.dec(_f$name),
      description: data.dec(_f$description),
      permission: data.dec(_f$permission),
    );
  }

  @override
  final Function instantiate = _instantiate;

  static RegisterCommandMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<RegisterCommandMessage>(map);
  }

  static RegisterCommandMessage fromJson(String json) {
    return ensureInitialized().decodeJson<RegisterCommandMessage>(json);
  }
}

mixin RegisterCommandMessageMappable {
  String toJson() {
    return RegisterCommandMessageMapper.ensureInitialized()
        .encodeJson<RegisterCommandMessage>(this as RegisterCommandMessage);
  }

  Map<String, dynamic> toMap() {
    return RegisterCommandMessageMapper.ensureInitialized()
        .encodeMap<RegisterCommandMessage>(this as RegisterCommandMessage);
  }

  RegisterCommandMessageCopyWith<
    RegisterCommandMessage,
    RegisterCommandMessage,
    RegisterCommandMessage
  >
  get copyWith =>
      _RegisterCommandMessageCopyWithImpl<
        RegisterCommandMessage,
        RegisterCommandMessage
      >(this as RegisterCommandMessage, $identity, $identity);
  @override
  String toString() {
    return RegisterCommandMessageMapper.ensureInitialized().stringifyValue(
      this as RegisterCommandMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return RegisterCommandMessageMapper.ensureInitialized().equalsValue(
      this as RegisterCommandMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return RegisterCommandMessageMapper.ensureInitialized().hashValue(
      this as RegisterCommandMessage,
    );
  }
}

extension RegisterCommandMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, RegisterCommandMessage, $Out> {
  RegisterCommandMessageCopyWith<$R, RegisterCommandMessage, $Out>
  get $asRegisterCommandMessage => $base.as(
    (v, t, t2) => _RegisterCommandMessageCopyWithImpl<$R, $Out>(v, t, t2),
  );
}

abstract class RegisterCommandMessageCopyWith<
  $R,
  $In extends RegisterCommandMessage,
  $Out
>
    implements ClientMessageCopyWith<$R, $In, $Out> {
  @override
  $R call({String? name, String? description, String? permission});
  RegisterCommandMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  );
}

class _RegisterCommandMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, RegisterCommandMessage, $Out>
    implements
        RegisterCommandMessageCopyWith<$R, RegisterCommandMessage, $Out> {
  _RegisterCommandMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<RegisterCommandMessage> $mapper =
      RegisterCommandMessageMapper.ensureInitialized();
  @override
  $R call({String? name, String? description, Object? permission = $none}) =>
      $apply(
        FieldCopyWithData({
          if (name != null) #name: name,
          if (description != null) #description: description,
          if (permission != $none) #permission: permission,
        }),
      );
  @override
  RegisterCommandMessage $make(CopyWithData data) => RegisterCommandMessage(
    name: data.get(#name, or: $value.name),
    description: data.get(#description, or: $value.description),
    permission: data.get(#permission, or: $value.permission),
  );

  @override
  RegisterCommandMessageCopyWith<$R2, RegisterCommandMessage, $Out2>
  $chain<$R2, $Out2>(Then<$Out2, $R2> t) =>
      _RegisterCommandMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class RegisterEventMessageMapper
    extends SubClassMapperBase<RegisterEventMessage> {
  RegisterEventMessageMapper._();

  static RegisterEventMessageMapper? _instance;
  static RegisterEventMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = RegisterEventMessageMapper._());
      ClientMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'RegisterEventMessage';

  static String _$event(RegisterEventMessage v) => v.event;
  static const Field<RegisterEventMessage, String> _f$event = Field(
    'event',
    _$event,
  );

  @override
  final MappableFields<RegisterEventMessage> fields = const {#event: _f$event};

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'register_event';
  @override
  late final ClassMapperBase superMapper =
      ClientMessageMapper.ensureInitialized();

  static RegisterEventMessage _instantiate(DecodingData data) {
    return RegisterEventMessage(event: data.dec(_f$event));
  }

  @override
  final Function instantiate = _instantiate;

  static RegisterEventMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<RegisterEventMessage>(map);
  }

  static RegisterEventMessage fromJson(String json) {
    return ensureInitialized().decodeJson<RegisterEventMessage>(json);
  }
}

mixin RegisterEventMessageMappable {
  String toJson() {
    return RegisterEventMessageMapper.ensureInitialized()
        .encodeJson<RegisterEventMessage>(this as RegisterEventMessage);
  }

  Map<String, dynamic> toMap() {
    return RegisterEventMessageMapper.ensureInitialized()
        .encodeMap<RegisterEventMessage>(this as RegisterEventMessage);
  }

  RegisterEventMessageCopyWith<
    RegisterEventMessage,
    RegisterEventMessage,
    RegisterEventMessage
  >
  get copyWith =>
      _RegisterEventMessageCopyWithImpl<
        RegisterEventMessage,
        RegisterEventMessage
      >(this as RegisterEventMessage, $identity, $identity);
  @override
  String toString() {
    return RegisterEventMessageMapper.ensureInitialized().stringifyValue(
      this as RegisterEventMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return RegisterEventMessageMapper.ensureInitialized().equalsValue(
      this as RegisterEventMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return RegisterEventMessageMapper.ensureInitialized().hashValue(
      this as RegisterEventMessage,
    );
  }
}

extension RegisterEventMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, RegisterEventMessage, $Out> {
  RegisterEventMessageCopyWith<$R, RegisterEventMessage, $Out>
  get $asRegisterEventMessage => $base.as(
    (v, t, t2) => _RegisterEventMessageCopyWithImpl<$R, $Out>(v, t, t2),
  );
}

abstract class RegisterEventMessageCopyWith<
  $R,
  $In extends RegisterEventMessage,
  $Out
>
    implements ClientMessageCopyWith<$R, $In, $Out> {
  @override
  $R call({String? event});
  RegisterEventMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  );
}

class _RegisterEventMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, RegisterEventMessage, $Out>
    implements RegisterEventMessageCopyWith<$R, RegisterEventMessage, $Out> {
  _RegisterEventMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<RegisterEventMessage> $mapper =
      RegisterEventMessageMapper.ensureInitialized();
  @override
  $R call({String? event}) =>
      $apply(FieldCopyWithData({if (event != null) #event: event}));
  @override
  RegisterEventMessage $make(CopyWithData data) =>
      RegisterEventMessage(event: data.get(#event, or: $value.event));

  @override
  RegisterEventMessageCopyWith<$R2, RegisterEventMessage, $Out2>
  $chain<$R2, $Out2>(Then<$Out2, $R2> t) =>
      _RegisterEventMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class HostCallMessageMapper extends SubClassMapperBase<HostCallMessage> {
  HostCallMessageMapper._();

  static HostCallMessageMapper? _instance;
  static HostCallMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = HostCallMessageMapper._());
      ClientMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'HostCallMessage';

  static int _$id(HostCallMessage v) => v.id;
  static const Field<HostCallMessage, int> _f$id = Field('id', _$id);
  static String _$method(HostCallMessage v) => v.method;
  static const Field<HostCallMessage, String> _f$method = Field(
    'method',
    _$method,
  );
  static Map<String, Object?> _$params(HostCallMessage v) => v.params;
  static const Field<HostCallMessage, Map<String, Object?>> _f$params = Field(
    'params',
    _$params,
    opt: true,
    def: const <String, Object?>{},
  );

  @override
  final MappableFields<HostCallMessage> fields = const {
    #id: _f$id,
    #method: _f$method,
    #params: _f$params,
  };

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'host_call';
  @override
  late final ClassMapperBase superMapper =
      ClientMessageMapper.ensureInitialized();

  static HostCallMessage _instantiate(DecodingData data) {
    return HostCallMessage(
      id: data.dec(_f$id),
      method: data.dec(_f$method),
      params: data.dec(_f$params),
    );
  }

  @override
  final Function instantiate = _instantiate;

  static HostCallMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<HostCallMessage>(map);
  }

  static HostCallMessage fromJson(String json) {
    return ensureInitialized().decodeJson<HostCallMessage>(json);
  }
}

mixin HostCallMessageMappable {
  String toJson() {
    return HostCallMessageMapper.ensureInitialized()
        .encodeJson<HostCallMessage>(this as HostCallMessage);
  }

  Map<String, dynamic> toMap() {
    return HostCallMessageMapper.ensureInitialized().encodeMap<HostCallMessage>(
      this as HostCallMessage,
    );
  }

  HostCallMessageCopyWith<HostCallMessage, HostCallMessage, HostCallMessage>
  get copyWith =>
      _HostCallMessageCopyWithImpl<HostCallMessage, HostCallMessage>(
        this as HostCallMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return HostCallMessageMapper.ensureInitialized().stringifyValue(
      this as HostCallMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return HostCallMessageMapper.ensureInitialized().equalsValue(
      this as HostCallMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return HostCallMessageMapper.ensureInitialized().hashValue(
      this as HostCallMessage,
    );
  }
}

extension HostCallMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, HostCallMessage, $Out> {
  HostCallMessageCopyWith<$R, HostCallMessage, $Out> get $asHostCallMessage =>
      $base.as((v, t, t2) => _HostCallMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class HostCallMessageCopyWith<$R, $In extends HostCallMessage, $Out>
    implements ClientMessageCopyWith<$R, $In, $Out> {
  MapCopyWith<$R, String, Object?, ObjectCopyWith<$R, Object?, Object?>?>
  get params;
  @override
  $R call({int? id, String? method, Map<String, Object?>? params});
  HostCallMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  );
}

class _HostCallMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, HostCallMessage, $Out>
    implements HostCallMessageCopyWith<$R, HostCallMessage, $Out> {
  _HostCallMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<HostCallMessage> $mapper =
      HostCallMessageMapper.ensureInitialized();
  @override
  MapCopyWith<$R, String, Object?, ObjectCopyWith<$R, Object?, Object?>?>
  get params => MapCopyWith(
    $value.params,
    (v, t) => ObjectCopyWith(v, $identity, t),
    (v) => call(params: v),
  );
  @override
  $R call({int? id, String? method, Map<String, Object?>? params}) => $apply(
    FieldCopyWithData({
      if (id != null) #id: id,
      if (method != null) #method: method,
      if (params != null) #params: params,
    }),
  );
  @override
  HostCallMessage $make(CopyWithData data) => HostCallMessage(
    id: data.get(#id, or: $value.id),
    method: data.get(#method, or: $value.method),
    params: data.get(#params, or: $value.params),
  );

  @override
  HostCallMessageCopyWith<$R2, HostCallMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _HostCallMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class ResponseMessageMapper extends SubClassMapperBase<ResponseMessage> {
  ResponseMessageMapper._();

  static ResponseMessageMapper? _instance;
  static ResponseMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = ResponseMessageMapper._());
      ClientMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'ResponseMessage';

  static int _$id(ResponseMessage v) => v.id;
  static const Field<ResponseMessage, int> _f$id = Field('id', _$id);
  static bool _$cancel(ResponseMessage v) => v.cancel;
  static const Field<ResponseMessage, bool> _f$cancel = Field(
    'cancel',
    _$cancel,
    opt: true,
    def: false,
  );

  @override
  final MappableFields<ResponseMessage> fields = const {
    #id: _f$id,
    #cancel: _f$cancel,
  };

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'response';
  @override
  late final ClassMapperBase superMapper =
      ClientMessageMapper.ensureInitialized();

  static ResponseMessage _instantiate(DecodingData data) {
    return ResponseMessage(id: data.dec(_f$id), cancel: data.dec(_f$cancel));
  }

  @override
  final Function instantiate = _instantiate;

  static ResponseMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<ResponseMessage>(map);
  }

  static ResponseMessage fromJson(String json) {
    return ensureInitialized().decodeJson<ResponseMessage>(json);
  }
}

mixin ResponseMessageMappable {
  String toJson() {
    return ResponseMessageMapper.ensureInitialized()
        .encodeJson<ResponseMessage>(this as ResponseMessage);
  }

  Map<String, dynamic> toMap() {
    return ResponseMessageMapper.ensureInitialized().encodeMap<ResponseMessage>(
      this as ResponseMessage,
    );
  }

  ResponseMessageCopyWith<ResponseMessage, ResponseMessage, ResponseMessage>
  get copyWith =>
      _ResponseMessageCopyWithImpl<ResponseMessage, ResponseMessage>(
        this as ResponseMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return ResponseMessageMapper.ensureInitialized().stringifyValue(
      this as ResponseMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return ResponseMessageMapper.ensureInitialized().equalsValue(
      this as ResponseMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return ResponseMessageMapper.ensureInitialized().hashValue(
      this as ResponseMessage,
    );
  }
}

extension ResponseMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, ResponseMessage, $Out> {
  ResponseMessageCopyWith<$R, ResponseMessage, $Out> get $asResponseMessage =>
      $base.as((v, t, t2) => _ResponseMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class ResponseMessageCopyWith<$R, $In extends ResponseMessage, $Out>
    implements ClientMessageCopyWith<$R, $In, $Out> {
  @override
  $R call({int? id, bool? cancel});
  ResponseMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  );
}

class _ResponseMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, ResponseMessage, $Out>
    implements ResponseMessageCopyWith<$R, ResponseMessage, $Out> {
  _ResponseMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<ResponseMessage> $mapper =
      ResponseMessageMapper.ensureInitialized();
  @override
  $R call({int? id, bool? cancel}) => $apply(
    FieldCopyWithData({
      if (id != null) #id: id,
      if (cancel != null) #cancel: cancel,
    }),
  );
  @override
  ResponseMessage $make(CopyWithData data) => ResponseMessage(
    id: data.get(#id, or: $value.id),
    cancel: data.get(#cancel, or: $value.cancel),
  );

  @override
  ResponseMessageCopyWith<$R2, ResponseMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _ResponseMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class LogMessageMapper extends SubClassMapperBase<LogMessage> {
  LogMessageMapper._();

  static LogMessageMapper? _instance;
  static LogMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = LogMessageMapper._());
      ClientMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'LogMessage';

  static String _$level(LogMessage v) => v.level;
  static const Field<LogMessage, String> _f$level = Field('level', _$level);
  static String _$message(LogMessage v) => v.message;
  static const Field<LogMessage, String> _f$message = Field(
    'message',
    _$message,
  );

  @override
  final MappableFields<LogMessage> fields = const {
    #level: _f$level,
    #message: _f$message,
  };

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'log';
  @override
  late final ClassMapperBase superMapper =
      ClientMessageMapper.ensureInitialized();

  static LogMessage _instantiate(DecodingData data) {
    return LogMessage(level: data.dec(_f$level), message: data.dec(_f$message));
  }

  @override
  final Function instantiate = _instantiate;

  static LogMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<LogMessage>(map);
  }

  static LogMessage fromJson(String json) {
    return ensureInitialized().decodeJson<LogMessage>(json);
  }
}

mixin LogMessageMappable {
  String toJson() {
    return LogMessageMapper.ensureInitialized().encodeJson<LogMessage>(
      this as LogMessage,
    );
  }

  Map<String, dynamic> toMap() {
    return LogMessageMapper.ensureInitialized().encodeMap<LogMessage>(
      this as LogMessage,
    );
  }

  LogMessageCopyWith<LogMessage, LogMessage, LogMessage> get copyWith =>
      _LogMessageCopyWithImpl<LogMessage, LogMessage>(
        this as LogMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return LogMessageMapper.ensureInitialized().stringifyValue(
      this as LogMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return LogMessageMapper.ensureInitialized().equalsValue(
      this as LogMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return LogMessageMapper.ensureInitialized().hashValue(this as LogMessage);
  }
}

extension LogMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, LogMessage, $Out> {
  LogMessageCopyWith<$R, LogMessage, $Out> get $asLogMessage =>
      $base.as((v, t, t2) => _LogMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class LogMessageCopyWith<$R, $In extends LogMessage, $Out>
    implements ClientMessageCopyWith<$R, $In, $Out> {
  @override
  $R call({String? level, String? message});
  LogMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t);
}

class _LogMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, LogMessage, $Out>
    implements LogMessageCopyWith<$R, LogMessage, $Out> {
  _LogMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<LogMessage> $mapper =
      LogMessageMapper.ensureInitialized();
  @override
  $R call({String? level, String? message}) => $apply(
    FieldCopyWithData({
      if (level != null) #level: level,
      if (message != null) #message: message,
    }),
  );
  @override
  LogMessage $make(CopyWithData data) => LogMessage(
    level: data.get(#level, or: $value.level),
    message: data.get(#message, or: $value.message),
  );

  @override
  LogMessageCopyWith<$R2, LogMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _LogMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}

class ReadyMessageMapper extends SubClassMapperBase<ReadyMessage> {
  ReadyMessageMapper._();

  static ReadyMessageMapper? _instance;
  static ReadyMessageMapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = ReadyMessageMapper._());
      ClientMessageMapper.ensureInitialized().addSubMapper(_instance!);
    }
    return _instance!;
  }

  @override
  final String id = 'ReadyMessage';

  @override
  final MappableFields<ReadyMessage> fields = const {};

  @override
  final String discriminatorKey = 't';
  @override
  final dynamic discriminatorValue = 'ready';
  @override
  late final ClassMapperBase superMapper =
      ClientMessageMapper.ensureInitialized();

  static ReadyMessage _instantiate(DecodingData data) {
    return ReadyMessage();
  }

  @override
  final Function instantiate = _instantiate;

  static ReadyMessage fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<ReadyMessage>(map);
  }

  static ReadyMessage fromJson(String json) {
    return ensureInitialized().decodeJson<ReadyMessage>(json);
  }
}

mixin ReadyMessageMappable {
  String toJson() {
    return ReadyMessageMapper.ensureInitialized().encodeJson<ReadyMessage>(
      this as ReadyMessage,
    );
  }

  Map<String, dynamic> toMap() {
    return ReadyMessageMapper.ensureInitialized().encodeMap<ReadyMessage>(
      this as ReadyMessage,
    );
  }

  ReadyMessageCopyWith<ReadyMessage, ReadyMessage, ReadyMessage> get copyWith =>
      _ReadyMessageCopyWithImpl<ReadyMessage, ReadyMessage>(
        this as ReadyMessage,
        $identity,
        $identity,
      );
  @override
  String toString() {
    return ReadyMessageMapper.ensureInitialized().stringifyValue(
      this as ReadyMessage,
    );
  }

  @override
  bool operator ==(Object other) {
    return ReadyMessageMapper.ensureInitialized().equalsValue(
      this as ReadyMessage,
      other,
    );
  }

  @override
  int get hashCode {
    return ReadyMessageMapper.ensureInitialized().hashValue(
      this as ReadyMessage,
    );
  }
}

extension ReadyMessageValueCopy<$R, $Out>
    on ObjectCopyWith<$R, ReadyMessage, $Out> {
  ReadyMessageCopyWith<$R, ReadyMessage, $Out> get $asReadyMessage =>
      $base.as((v, t, t2) => _ReadyMessageCopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class ReadyMessageCopyWith<$R, $In extends ReadyMessage, $Out>
    implements ClientMessageCopyWith<$R, $In, $Out> {
  @override
  $R call();
  ReadyMessageCopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t);
}

class _ReadyMessageCopyWithImpl<$R, $Out>
    extends ClassCopyWithBase<$R, ReadyMessage, $Out>
    implements ReadyMessageCopyWith<$R, ReadyMessage, $Out> {
  _ReadyMessageCopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<ReadyMessage> $mapper =
      ReadyMessageMapper.ensureInitialized();
  @override
  $R call() => $apply(FieldCopyWithData({}));
  @override
  ReadyMessage $make(CopyWithData data) => ReadyMessage();

  @override
  ReadyMessageCopyWith<$R2, ReadyMessage, $Out2> $chain<$R2, $Out2>(
    Then<$Out2, $R2> t,
  ) => _ReadyMessageCopyWithImpl<$R2, $Out2>($value, $cast, t);
}


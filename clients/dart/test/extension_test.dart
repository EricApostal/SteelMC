import 'package:steel_extension/steel_plugin.dart';
import 'package:test/test.dart';

/// One class registration observed by [RecordingShim].
final class ClassRegistrationCall {
  ClassRegistrationCall({
    required this.name,
    required this.inherits,
    required this.methodNames,
    required this.methodHashes,
    required this.argCounts,
    required this.argTypes,
    required this.returnTypes,
  });

  final String name;
  final String inherits;
  final List<String> methodNames;
  final List<int> methodHashes;
  final List<int> argCounts;
  final List<int> argTypes;
  final List<int> returnTypes;
}

/// In-memory [NativeShim] that records registration metadata.
final class RecordingShim implements NativeShim {
  final List<ClassRegistrationCall> classes = <ClassRegistrationCall>[];

  @override
  int registerClass({
    required String name,
    required String inherits,
    required List<String> methodNames,
    required List<int> methodHashes,
    required List<int> argCounts,
    required List<int> argTypes,
    required List<int> returnTypes,
  }) {
    classes.add(
      ClassRegistrationCall(
        name: name,
        inherits: inherits,
        methodNames: methodNames,
        methodHashes: methodHashes,
        argCounts: argCounts,
        argTypes: argTypes,
        returnTypes: returnTypes,
      ),
    );
    return 0;
  }

  @override
  int registerCommand(
    int id,
    String name,
    String description,
    String permission,
  ) =>
      0;

  @override
  int registerEvent(int kind) => 0;

  @override
  int log(int level, String message) => 0;
}

/// The class exercised through `steelConstructExtension`/`steelInvokeExtension`.
final class Greeter {
  int _count = 0;

  String greet(String name) => 'Hello, $name!';

  int count() => _count;

  int increment(int amount) => _count += amount;

  void reset() {
    _count = 0;
  }

  bool negate(bool value) => !value;

  double half(double value) => value / 2;
}

SteelMethodRegistration _method(
  String name,
  String signature,
  List<SteelValueType> argTypes,
  SteelValueType returnType,
  Object? Function(Greeter instance, List<Object?> args) invoke,
) =>
    SteelMethodRegistration(
      name: name,
      hash: steelMethodHash(signature),
      argTypes: argTypes,
      returnType: returnType,
      invoke: (Object instance, List<Object?> args) =>
          invoke(instance as Greeter, args),
    );

void main() {
  late RecordingShim shim;
  late int classId;
  late int instanceId;

  setUpAll(() {
    shim = RecordingShim();
    final registration = SteelClassRegistration(
      name: 'Greeter',
      inherits: 'Object',
      construct: Greeter.new,
      methods: <SteelMethodRegistration>[
        _method(
          'greet',
          'Greeter::greet(String)->String',
          const <SteelValueType>[SteelValueType.string],
          SteelValueType.string,
          (instance, args) => instance.greet(args.single! as String),
        ),
        _method(
          'count',
          'Greeter::count()->int',
          const <SteelValueType>[],
          SteelValueType.i32,
          (instance, args) => instance.count(),
        ),
        _method(
          'increment',
          'Greeter::increment(int)->int',
          const <SteelValueType>[SteelValueType.i32],
          SteelValueType.i32,
          (instance, args) => instance.increment(args.single! as int),
        ),
        _method(
          'reset',
          'Greeter::reset()->void',
          const <SteelValueType>[],
          SteelValueType.voidType,
          (instance, args) {
            instance.reset();
            return null;
          },
        ),
        _method(
          'negate',
          'Greeter::negate(bool)->bool',
          const <SteelValueType>[SteelValueType.boolean],
          SteelValueType.boolean,
          (instance, args) => instance.negate(args.single! as bool),
        ),
        _method(
          'half',
          'Greeter::half(double)->double',
          const <SteelValueType>[SteelValueType.f64],
          SteelValueType.f64,
          (instance, args) => instance.half(args.single! as double),
        ),
      ],
    );
    classId = registerExtensionClasses(
      <SteelClassRegistration>[registration],
      shim,
    ).single;
    instanceId = steelConstructExtension(classId);
  });

  test('registers the class and its method metadata with the shim', () {
    final call = shim.classes.single;
    expect(call.name, 'Greeter');
    expect(call.inherits, 'Object');
    expect(
      call.methodNames,
      <String>['greet', 'count', 'increment', 'reset', 'negate', 'half'],
    );
    expect(call.argCounts, <int>[1, 0, 1, 0, 1, 1]);
    expect(call.argTypes, <int>[
      SteelValueType.string.index,
      SteelValueType.i32.index,
      SteelValueType.boolean.index,
      SteelValueType.f64.index,
    ]);
    expect(call.returnTypes, <int>[
      SteelValueType.string.index,
      SteelValueType.i32.index,
      SteelValueType.i32.index,
      SteelValueType.voidType.index,
      SteelValueType.boolean.index,
      SteelValueType.f64.index,
    ]);
    expect(call.methodHashes.first, 0x69664864);
    expect(steelMethodHash('Greeter::greet(String)->String'), 0x69664864);
  });

  test('invokes string and integer methods on a constructed instance', () {
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'greet')!,
        instanceId,
        <Object?>['Ada'],
      ),
      'Hello, Ada!',
    );
    final increment = steelExtensionMethodId('Greeter', 'increment')!;
    expect(steelInvokeExtension(increment, instanceId, <Object?>[2]), 2);
    expect(steelInvokeExtension(increment, instanceId, <Object?>[3]), 5);
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'count')!,
        instanceId,
        const <Object?>[],
      ),
      5,
    );
  });

  test('invokes boolean, double and void methods', () {
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'negate')!,
        instanceId,
        <Object?>[true],
      ),
      isFalse,
    );
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'half')!,
        instanceId,
        <Object?>[7.0],
      ),
      3.5,
    );
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'reset')!,
        instanceId,
        const <Object?>[],
      ),
      isNull,
    );
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'count')!,
        instanceId,
        const <Object?>[],
      ),
      0,
    );
  });

  test('instances keep their own state', () {
    final other = steelConstructExtension(classId);
    expect(other, isNot(instanceId));

    final increment = steelExtensionMethodId('Greeter', 'increment')!;
    steelInvokeExtension(increment, other, <Object?>[4]);
    expect(steelInvokeExtension(increment, other, <Object?>[0]), 4);
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'count')!,
        instanceId,
        const <Object?>[],
      ),
      0,
    );
  });

  test('rejects unknown ids and wrong argument counts', () {
    expect(() => steelConstructExtension(9999), throwsStateError);
    expect(
      () => steelInvokeExtension(9999, instanceId, const <Object?>[]),
      throwsStateError,
    );
    expect(
      () => steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'greet')!,
        instanceId,
        const <Object?>[],
      ),
      throwsArgumentError,
    );
  });

  test('void is rejected as a parameter type', () {
    final shim = RecordingShim();
    expect(
      () => registerExtensionClasses(
        <SteelClassRegistration>[
          SteelClassRegistration(
            name: 'Broken',
            inherits: 'Object',
            construct: Greeter.new,
            methods: <SteelMethodRegistration>[
              SteelMethodRegistration(
                name: 'broken',
                hash: 0,
                argTypes: const <SteelValueType>[SteelValueType.voidType],
                returnType: SteelValueType.voidType,
                invoke: (instance, args) => null,
              ),
            ],
          ),
        ],
        shim,
      ),
      throwsArgumentError,
    );
    expect(shim.classes, isEmpty);
  });
}
// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format width=80

part of 'greeter.dart';

// **************************************************************************
// SteelClassGenerator
// **************************************************************************

/// Extension classes declared in this library.
List<SteelClassRegistration> get steelClasses => <SteelClassRegistration>[
      SteelClassRegistration(
        name: 'Greeter',
        inherits: 'Object',
        construct: () => Greeter(),
        methods: <SteelMethodRegistration>[
          SteelMethodRegistration(
            name: 'greet',
            hash: 1768310884,
            argTypes: const <SteelValueType>[SteelValueType.string],
            returnType: SteelValueType.string,
            invoke: (Object instance, List<Object?> args) {
              return (instance as Greeter).greet(args[0] as String);
            },
          ),
          SteelMethodRegistration(
            name: 'count',
            hash: 3076273273,
            argTypes: const <SteelValueType>[],
            returnType: SteelValueType.i32,
            invoke: (Object instance, List<Object?> args) {
              return (instance as Greeter).count();
            },
          ),
          SteelMethodRegistration(
            name: 'increment',
            hash: 2681002766,
            argTypes: const <SteelValueType>[SteelValueType.i32],
            returnType: SteelValueType.i32,
            invoke: (Object instance, List<Object?> args) {
              return (instance as Greeter).increment(args[0] as int);
            },
          ),
          SteelMethodRegistration(
            name: 'half',
            hash: 3262667310,
            argTypes: const <SteelValueType>[SteelValueType.f64],
            returnType: SteelValueType.f64,
            invoke: (Object instance, List<Object?> args) {
              return (instance as Greeter).half(args[0] as double);
            },
          ),
          SteelMethodRegistration(
            name: 'toggle',
            hash: 1561724341,
            argTypes: const <SteelValueType>[SteelValueType.boolean],
            returnType: SteelValueType.boolean,
            invoke: (Object instance, List<Object?> args) {
              return (instance as Greeter).toggle(args[0] as bool);
            },
          ),
          SteelMethodRegistration(
            name: 'reset',
            hash: 2168424486,
            argTypes: const <SteelValueType>[],
            returnType: SteelValueType.voidType,
            invoke: (Object instance, List<Object?> args) {
              (instance as Greeter).reset();
              return null;
            },
          ),
        ],
      ),
    ];

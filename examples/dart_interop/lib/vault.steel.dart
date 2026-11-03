// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format width=80

part of 'vault.dart';

// **************************************************************************
// SteelClassGenerator
// **************************************************************************

/// Extension classes declared in this library.
List<SteelClassRegistration> get steelClasses => <SteelClassRegistration>[
      SteelClassRegistration(
        name: 'Vault',
        inherits: 'Object',
        construct: () => Vault(),
        methods: <SteelMethodRegistration>[
          SteelMethodRegistration(
            name: 'deposit',
            hash: 1760947538,
            argTypes: const <SteelValueType>[
              SteelValueType.string,
              SteelValueType.f64
            ],
            returnType: SteelValueType.f64,
            invoke: (Object instance, List<Object?> args) {
              return (instance as Vault)
                  .deposit(args[0] as String, args[1] as double);
            },
          ),
          SteelMethodRegistration(
            name: 'balance',
            hash: 3722798171,
            argTypes: const <SteelValueType>[SteelValueType.string],
            returnType: SteelValueType.f64,
            invoke: (Object instance, List<Object?> args) {
              return (instance as Vault).balance(args[0] as String);
            },
          ),
        ],
      ),
    ];

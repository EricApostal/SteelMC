# Two Dart plugins talking through ClassDB

`vault` and `economy` are separate Dart plugins with separate kernels and
separate copies of the runtime shim. They communicate in-process through the
host ClassDB; there is no RPC, socket, or subprocess boundary.

- `lib/vault.dart` declares `@SteelClass('Vault')`. Its `deposit` and `balance`
  methods are registered in the host ClassDB when the vault plugin starts.
- `lib/vault_client.dart` is the consumer-side wrapper. It calls
  `classdb_construct('Vault')` and then `method_bind_ptrcall` with the shared
  method hash.
- `bin/vault.dart` is the provider entry point.
- `bin/economy.dart` is the consumer entry point. `onEnable` constructs the
  vault, deposits 15.5, and logs the returned balance.

Both sides must agree on the class name, method name, argument types, and hash.
The conventional hash is:

```dart
steelMethodHash('Vault::deposit(String,double)->double')
```

A shared interface package is the clean way to keep provider and consumer in
sync. Passing `0` as the hash resolves by class/method name only and avoids
version skew at the cost of not checking the signature.

## Run the example

```sh
dart pub get
dart run build_runner build
dart analyze
dart test
```

Build the two kernels with the Dart SDK source build that produced
`libdart_engine_jit_shared.so`:

```sh
SDK=/path/to/dart-sdk/out/ReleaseX64
dart pub get
"$SDK/bootstrap_gen_kernel.exe" \
  --platform="$SDK/vm_platform.dill" --no-aot --no-embed-sources \
  --no-link-platform --packages="$PWD/.dart_tool/package_config.json" \
  --output=/tmp/dartembed/interop/vault.dill --link-platform "$PWD/bin/vault.dart"
"$SDK/bootstrap_gen_kernel.exe" \
  --platform="$SDK/vm_platform.dill" --no-aot --no-embed-sources \
  --no-link-platform --packages="$PWD/.dart_tool/package_config.json" \
  --output=/tmp/dartembed/interop/economy.dill --link-platform "$PWD/bin/economy.dart"
```

Run the host integration test:

```sh
STEEL_DART_VM_LIB="$SDK/libdart_engine_jit_shared.so" \
STEEL_DART_VAULT_KERNEL=/tmp/dartembed/interop/vault.dill \
STEEL_DART_ECONOMY_KERNEL=/tmp/dartembed/interop/economy.dill \
cargo test -p steel-plugin-host --test dart_interop -- --nocapture
```

The test assembles this `plugins/` tree and loads it with
`ExtensionManager::load_dir`:

```text
plugins/
  01_vault/
    plugin.so          # copy of target/debug/libsteel_dart_embed.so
    plugin.so.json     # {"id":"vault",...,"kernel":"vault.dill",...}
  02_economy/
    plugin.so
    plugin.so.json
```

Directory names sort provider-first here because the consumer calls the vault
during its own `onEnable`. In a real server, resolve class/method binds lazily
on first use so load order does not matter.

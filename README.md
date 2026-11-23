# RustGen

Fabric mod for Minecraft 1.18.2 that runs the noise stage of world generation in native Rust code. The output matches vanilla bit for bit. If no native library exists for your platform, or the native code fails, RustGen falls back to vanilla generation.

## Requirements

- Minecraft 1.18.2, Fabric Loader >= 0.14.0, Java 17+
- To build: JDK 17+, Rust toolchain (`cargo`)
- Optional Windows x64 cross-build from Linux: `rustup target add x86_64-pc-windows-gnu` and `gcc-mingw-w64-x86-64`

## Building

```sh
./gradlew build
```

The native library is compiled with cargo and packaged into the mod jar under `/natives/<platform>/`.

## License

Copyright (C) nanoave

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).

SPDX-License-Identifier: Apache-2.0

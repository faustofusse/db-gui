# DBGui

Native database client. macOS frontend in SwiftUI, Linux frontend (later) in GPUI.
Both share one Rust core.

## Layout

```
crates/
  dbcore/        # shared core, pure Rust: models, Driver trait, drivers (mock for now), own tokio runtime
  dbcore-ffi/    # UniFFI wrapper over dbcore → Swift bindings (staticlib)
tools/
  uniffi-bindgen/  # bindings generator CLI (kept separate so its deps don't leak into the lib)
apps/
  macos/         # SwiftUI app (Swift package)
    Sources/DBKit/      # Swift models + DatabaseDriver protocol; RustDriver adapts the FFI
    Sources/DBGuiMac/   # UI (Mail-style 3-column NavigationSplitView)
    Sources/DBCoreFFI/  # generated, git-ignored
    Frameworks/         # generated DBCoreFFI.xcframework, git-ignored
  linux/         # GPUI app (todo), depends on crates/dbcore directly
scripts/
  build-core.sh  # cargo build dbcore-ffi → xcframework + Swift bindings
  bundle-mac.sh  # build-core + swift build → build/DBGui.app
```

```
SwiftUI views ─▶ DBKit (DatabaseDriver) ─▶ RustDriver ─▶ DBCoreFFI (UniFFI) ─┐
                                                                              ├─▶ dbcore
GPUI views ───────────────────────────────────────────────────────────────────┘
```

Rules:
- Database logic (drivers, value decoding, SQL handling, connection storage) goes in `dbcore`.
- `dbcore` has no FFI or UI dependencies. `dbcore-ffi` mirrors its types and is the only FFI surface.
- Only `apps/macos/Sources/DBKit/RustDriver.swift` imports `DBCoreFFI`.
- `dbcore::Connection` runs work on the core's own tokio runtime, so you can await it from any
  executor (Swift concurrency, GPUI). Dropping the future aborts the work.
- Rows cross the boundary in pages (`QueryResult`), never one cell at a time.

## Dev shell

```sh
nix develop        # Rust toolchain from rust-toolchain.toml (+ rust-analyzer), pkg-config
```

- macOS: Swift, `xcodebuild`, `lipo` and `codesign` come from the installed Xcode. The shell
  doesn't add a Nix C compiler or SDK, and points cargo at `/usr/bin/clang`.
- Linux: also gives you clang, mold and the native libraries GPUI needs (Wayland/X11, Vulkan,
  fonts), with `LD_LIBRARY_PATH` set.

## Run

```sh
cargo test -p dbcore                               # core tests
./scripts/build-core.sh                            # needed once before opening apps/macos in Xcode
(cd apps/macos && swift test)                      # Swift ⇄ Rust bridge tests
./scripts/bundle-mac.sh && open build/DBGui.app    # build + run the macOS app
```

Rerun `scripts/build-core.sh` after changing anything in `crates/`. `bundle-mac.sh` does this for you.
Set `UNIVERSAL=1` to also build x86_64. The toolchain is pinned in `rust-toolchain.toml`.

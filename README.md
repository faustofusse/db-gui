# DBGui

Native database client. macOS frontend in SwiftUI, Linux frontend (later) in GPUI.
Both share one Rust core.

## Layout

```
crates/
  dbcore/        # shared core, pure Rust: models, Driver trait, drivers (Postgres + mock), own tokio runtime
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
dev/postgres/init.sql  # seed for the dev database
scripts/
  build-core.sh      # cargo build dbcore-ffi → xcframework + Swift bindings
  bundle-mac.sh      # build-core + swift build → build/DBGui.app
  dev-db.sh          # dev Postgres in an Apple `container` (up/down/reset/psql/logs)
  test-postgres.sh   # dev-db up + core integration tests
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

## Dev database

```sh
./scripts/dev-db.sh up     # postgres://postgres:postgres@localhost:54329/app_dev
```

Add it with **File ▸ Add Sample Connections** (debug builds only). This adds the sample
connections, and `app_dev` among them points at the dev database. All other samples use the mock driver. The seed (`dev/postgres/init.sql`) covers the types and relation kinds the driver has to
handle: uuid, jsonb, arrays, enums, inet, bytea, intervals, big NUMERICs, views, a materialized
view, a partitioned table, a table without a primary key and an empty schema.
`./scripts/dev-db.sh reset` recreates it from scratch.

## Saved connections

- The core (`crates/dbcore/src/store.rs`) keeps connections in a versioned JSON file:
  `~/Library/Application Support/DBGui/connections.json` on macOS and
  `$XDG_CONFIG_HOME/dbgui/connections.json` on Linux. Writes are atomic and the file mode is 0600.
- Passwords never go in that file. The frontend keeps them in the platform keychain (macOS
  Keychain, service `dev.fausto.dbgui.connection`, account = connection id) and passes them in
  when it connects. The Keychain is only read when a connection opens or a test runs.
- URL parsing (`postgres://user:pass@host:port/db?sslmode=…`), validation and "Copy URL" also live
  in the core, so every frontend's connection form behaves the same.
- In the app: **File ▸ New Connection… (⇧⌘N)**, **Edit Connection… (⇧⌘E)**, the sidebar context
  menu (Edit / Duplicate / Copy URL / Delete), and ⌫ to delete the selected connection.
- Set `DBGUI_CONNECTIONS_FILE=/path/to/file.json` to use a different file, for example
  `open --env DBGUI_CONNECTIONS_FILE=/tmp/c.json build/DBGui.app`.
- The app is ad-hoc signed, so after a rebuild macOS may ask once to let DBGui read a saved
  password. Click "Always Allow".

## Postgres driver

- Values come back in Postgres' text format, so every type (extensions included) renders like psql.
  Column types from `prepare` turn ints/floats/bools into typed values and NUMERIC into an exact
  `Decimal` string.
- Each connection opens two server sessions: one for browsing and one for scripts, so a long
  script doesn't block browsing.
- Table pages are ordered by primary key (or `ctid` if there's none). `total_count` is exact under
  100k estimated rows and the planner estimate above that.
- Scripts can hold several statements. You get the last result set, or the affected-row count if
  no statement returned rows.
- Cancellation: `Connection::cancel()` (Stop / ⌘. in the app) sends a Postgres cancel request.
  Dropping an `execute` future does the same.
- TLS follows libpq's `sslmode`: `prefer` (default) / `require` encrypt without verifying the
  certificate, `verify-full` checks it against Mozilla's roots. Uses rustls with `ring`, so no
  OpenSSL or cmake is needed.

## Run

```sh
cargo test -p dbcore                               # core tests (Postgres tests skip themselves)
./scripts/test-postgres.sh                         # + integration tests against the dev database
./scripts/build-core.sh                            # needed once before opening apps/macos in Xcode
(cd apps/macos && swift test)                      # Swift ⇄ Rust bridge tests (DBGUI_TEST_POSTGRES=1 for real-db ones)
./scripts/bundle-mac.sh && open build/DBGui.app    # build + run the macOS app
```

Rerun `scripts/build-core.sh` after changing anything in `crates/`. `bundle-mac.sh` does this for you.
Set `UNIVERSAL=1` to also build x86_64. The toolchain is pinned in `rust-toolchain.toml`.

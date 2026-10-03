# dbear

<img src="assets/logo.svg" width="96" alt="dbear">

Native database client. The macOS app is SwiftUI, a Linux app in GPUI is planned. Both sit on a
shared Rust core.

## Quick start

```sh
nix develop                                       # Rust toolchain (Xcode provides Swift on macOS)
./scripts/dev-db.sh up                            # dev Postgres at localhost:54329/app_dev
./scripts/bundle-mac.sh && open build/dbear.app   # build and run
```

In debug builds, **File ▸ Add Sample Connections** adds `app_dev` plus some mock connections.

### Releases

`./scripts/release-mac.sh 0.1.0` builds `build/dbear-0.1.0-macos-arm64.zip`. Add `--publish` to tag
`v0.1.0`, push the tag and create the GitHub release with the zip attached (needs a clean tree and `gh`).

### Without Nix

You don't need Nix; it just pins the exact toolchain. Install equivalents yourself:

- **Rust**: install via [rustup](https://rustup.rs). `rustup show` in the repo root picks up
  `rust-toolchain.toml` (stable channel + `rustfmt`, `clippy`, `rust-src`, `rust-analyzer`).
- **macOS**: Xcode (for Swift, `swift build`, `xcodebuild`, `lipo`, `codesign`) plus `pkg-config`
  (`brew install pkg-config`). Cargo/`cc` should use Xcode's clang, which is the default outside
  the Nix shell.
- **Linux**: a C toolchain (`clang` or `gcc`) and `pkg-config`, plus GPUI's native deps:
  `wayland`, `libxkbcommon`, a Vulkan loader, `libGL`/Mesa, `fontconfig`, `freetype`, `openssl`,
  `alsa-lib`, and X11 libs (`libX11`, `libxcb`, `libXcursor`, `libXi`, `libXrandr`) — e.g. on
  Debian/Ubuntu: `apt install clang pkg-config libwayland-dev libxkbcommon-dev libvulkan-dev
  libgl1-mesa-dev libfontconfig1-dev libfreetype6-dev libssl-dev libasound2-dev libx11-dev
  libxcb1-dev libxcursor-dev libxi-dev libxrandr-dev`.

Then skip `nix develop` and run the same commands as above (`./scripts/dev-db.sh up`,
`./scripts/bundle-mac.sh`, `cargo test -p dbcore`, ...) directly.

## Layout

```
crates/dbcore/      Rust core: models, drivers (Postgres + mock), connection store, SQL highlighting
crates/dbcore-ffi/  UniFFI bindings for Swift
apps/macos/         SwiftUI app
apps/linux/         GPUI app (todo)
scripts/            build, bundle, dev database, tests
```

## Tests

```sh
cargo test -p dbcore              # core (Postgres tests skip without a database)
./scripts/test-postgres.sh        # core against the dev database
(cd apps/macos && swift test)     # Swift ⇄ Rust bridge
```

## Notes

- Connections are saved in a SQLite database, `~/Library/Application Support/dbear/dbear.db` (or
  `$XDG_CONFIG_HOME/dbear/` on Linux), versioned with `PRAGMA user_version`. An old
  `connections.json` is imported once and renamed to `connections.json.migrated`. Override the
  path with `DBEAR_CONNECTIONS_FILE`.
- Passwords go in the system keychain, never in that file.
- SQL highlighting uses tree-sitter with [DerekStride/tree-sitter-sql](https://github.com/DerekStride/tree-sitter-sql)
  (crate `tree-sitter-sequel`) in `dbcore::highlight`. The core returns spans and each frontend picks the colors.
- Table tabs sort on the server (click a header: ascending → descending → off). In a table's
  Data view the toolbar search field (⌘L) is a raw `WHERE` filter, applied with Return and
  completed like the script editor (`complete::complete_filter`). Elsewhere (Structure, script
  results) it searches as you type. The core sorts by the primary key (or ctid/rowid) after the
  user's columns, so pages stay stable. It also rejects a filter with a `;` between statements:
  `dialect::normalize_filter` is the only guard for MySQL, whose text protocol runs multiple
  statements. Postgres and SQLite also prepare a single statement.
- Structure (⌥⌘2) comes from `Driver::describe_table`: columns with defaults and comments, the
  primary key in key order, indexes, foreign keys and DDL. Postgres DDL is rebuilt from the
  catalogs; MySQL uses `SHOW CREATE TABLE` and SQLite `sqlite_master`.
- Rows of tables with a primary key are editable: double-click or Return edits a cell, Tab moves to
  the next one, "+" or ⌥⌘N adds a row, and ⌫ deletes rows. Edits stay pending until ⌘S, which shows the
  exact SQL (`dbcore::edit::statements`) before running it. The driver runs it in one
  transaction on a session of its own. It rolls back if any statement fails, or if an
  UPDATE/DELETE doesn't match exactly one row (the row changed since it was loaded). Values
  are sent as string literals and cast by the database. Views, keyless tables and binary
  columns are read-only.
- `bundle-mac.sh` signs with your "Apple Development" identity when you have one, so the
  Keychain's "Always Allow" survives rebuilds.

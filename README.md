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

## Layout

```
crates/dbcore/      Rust core: models, drivers (Postgres + mock), connection store
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

- Connections are saved to `~/Library/Application Support/dbear/connections.json` (or
  `$XDG_CONFIG_HOME/dbear/` on Linux). Override the path with `DBEAR_CONNECTIONS_FILE`.
- Passwords go in the system keychain, never in that file.
- `bundle-mac.sh` signs with your "Apple Development" identity when you have one, so the
  Keychain's "Always Allow" survives rebuilds.

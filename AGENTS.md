# AGENTS.md

- Database logic (drivers, decoding, SQL, connection storage, URL parsing) belongs in `crates/dbcore`. It has no FFI or UI deps.
- `crates/dbcore-ffi` is the only FFI surface. Only `apps/macos/Sources/DBKit/RustDriver.swift` imports `DBCoreFFI`.
- Rows cross the FFI boundary in pages (`QueryResult`), never one cell at a time.
- After changing `crates/`, run `./scripts/build-core.sh` (`bundle-mac.sh` runs it for you). `Sources/DBCoreFFI` and `Frameworks/` are generated.
- Check work with `cargo test -p dbcore`, `./scripts/test-postgres.sh` and `(cd apps/macos && swift test)`.
- Dev DB: `./scripts/dev-db.sh up|down|reset|psql`. Seed is in `dev/postgres/init.sql`.
- Use `nix develop`. Commit messages use `feat:`, `fix:`, `chore:` prefixes.

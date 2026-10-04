#!/usr/bin/env bash
# Starts the dev libSQL server (container dbear-libsql) if needed and runs the core's integration tests against it.
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/dev-db.sh up libsql >/dev/null
DBEAR_TEST_LIBSQL=1 cargo test -p dbcore "$@"

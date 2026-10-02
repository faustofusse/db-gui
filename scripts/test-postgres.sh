#!/usr/bin/env bash
# Starts the dev Postgres if needed and runs the core's integration tests against it.
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/dev-db.sh up >/dev/null
DBGUI_TEST_POSTGRES=1 cargo test -p dbcore "$@"

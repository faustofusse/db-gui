#!/usr/bin/env bash
# Starts the dev MySQL if needed and runs the core's integration tests against it.
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/dev-db.sh up mysql >/dev/null
DBEAR_TEST_MYSQL=1 cargo test -p dbcore "$@"

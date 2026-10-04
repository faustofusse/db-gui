#!/usr/bin/env bash
# Starts the dev SQL Server if needed and runs the core's integration tests against it.
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/dev-db.sh up sqlserver >/dev/null
DBEAR_TEST_SQLSERVER=1 cargo test -p dbcore "$@"

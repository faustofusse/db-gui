#!/usr/bin/env bash
# Local databases for development and integration tests (Apple `container` CLI for servers).
#   scripts/dev-db.sh up    [postgres|mysql|sqlite]   start / create (seeds on first boot); default: all
#   scripts/dev-db.sh down  [postgres|mysql|sqlite]   stop and delete (data is discarded)
#   scripts/dev-db.sh reset [postgres|mysql|sqlite]   down + up
#   scripts/dev-db.sh shell  postgres|mysql|sqlite    open psql / mysql / sqlite3
#   scripts/dev-db.sh logs   postgres|mysql           container logs
#
# Connections (non-default ports so they don't clash with servers already running):
#   postgres://postgres:postgres@localhost:54329/app_dev
#   mysql://root:mysql@localhost:33069
#   sqlite://$PWD/dev/sqlite/app.db
set -euo pipefail
cd "$(dirname "$0")/.."

PG_NAME=dbear-postgres PG_IMAGE=postgres:17 PG_PORT=54329
MY_NAME=dbear-mysql MY_IMAGE=mysql:8.4 MY_PORT=33069
SQLITE_FILE=dev/sqlite/app.db

wait_for() { # name, ready-check command, seed-error pattern, url
  local name=$1 check=$2 error_pattern=$3 url=$4
  printf 'waiting for %s' "$name"
  for _ in $(seq 1 120); do
    if eval "$check" 2>/dev/null; then
      echo ' ready'
      echo "$url"
      return 0
    fi
    if container logs "$name" 2>/dev/null | grep -qE "$error_pattern"; then
      echo ' seed failed:' >&2
      container logs "$name" | grep -E -A2 "$error_pattern" >&2
      return 1
    fi
    printf '.'
    sleep 1
  done
  echo ' timed out' >&2
  container logs "$name" | tail -20 >&2
  return 1
}

start_container() { # name, run args...
  local name=$1; shift
  if container inspect "$name" >/dev/null 2>&1; then
    container start "$name" >/dev/null 2>&1 || true
  else
    container run -d --name "$name" "$@" >/dev/null
  fi
}

remove_container() {
  container stop "$1" >/dev/null 2>&1 || true
  container rm "$1" >/dev/null 2>&1 || true
}

up_postgres() {
  start_container "$PG_NAME" \
    -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=app_dev \
    -p "127.0.0.1:$PG_PORT:5432" \
    -v "$PWD/dev/postgres:/docker-entrypoint-initdb.d:ro" \
    "$PG_IMAGE"
  # The entrypoint runs init.sql on a temporary server first; wait for the real one.
  wait_for "$PG_NAME" \
    "container logs $PG_NAME | grep -q 'PostgreSQL init process complete' && container exec $PG_NAME pg_isready -q -U postgres -d app_dev" \
    'init.sql:[0-9]*: ERROR' \
    "postgres://postgres:postgres@localhost:$PG_PORT/app_dev"
}

up_mysql() {
  start_container "$MY_NAME" \
    -e MYSQL_ROOT_PASSWORD=mysql \
    -p "127.0.0.1:$MY_PORT:3306" \
    -v "$PWD/dev/mysql:/docker-entrypoint-initdb.d:ro" \
    "$MY_IMAGE"
  wait_for "$MY_NAME" \
    "container logs $MY_NAME | grep -q 'MySQL init process done' && container exec $MY_NAME mysqladmin ping -uroot -pmysql --silent >/dev/null" \
    'ERROR [0-9]+ \(' \
    "mysql://root:mysql@localhost:$MY_PORT"
}

up_sqlite() {
  if [ ! -f "$SQLITE_FILE" ]; then
    command -v sqlite3 >/dev/null || { echo 'sqlite3 not found (nix develop provides it)' >&2; return 1; }
    sqlite3 "$SQLITE_FILE" < dev/sqlite/init.sql
  fi
  echo "sqlite://$PWD/$SQLITE_FILE"
}

for_each() { # action, target
  local action=$1 target=${2:-all}
  case "$target" in
    postgres|mysql|sqlite) "${action}_$target" ;;
    all) "${action}_postgres"; "${action}_mysql"; "${action}_sqlite" ;;
    *) echo "unknown database: $target (postgres|mysql|sqlite)" >&2; exit 2 ;;
  esac
}

down_postgres() { remove_container "$PG_NAME"; }
down_mysql() { remove_container "$MY_NAME"; }
down_sqlite() { rm -f "$SQLITE_FILE"; }

case "${1:-up}" in
  up) for_each up "${2:-}" ;;
  down) for_each down "${2:-}" ;;
  reset) for_each down "${2:-}"; for_each up "${2:-}" ;;
  shell|psql)
    case "${2:-postgres}" in
      postgres) container exec -it "$PG_NAME" psql -U postgres -d app_dev ;;
      mysql) container exec -it "$MY_NAME" mysql -uroot -pmysql ;;
      sqlite) sqlite3 "$SQLITE_FILE" ;;
    esac ;;
  logs) if [ "${2:-postgres}" = mysql ]; then container logs "$MY_NAME"; else container logs "$PG_NAME"; fi ;;
  *) echo "usage: $0 up|down|reset|shell|logs [postgres|mysql|sqlite]" >&2; exit 2 ;;
esac

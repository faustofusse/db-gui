#!/usr/bin/env bash
# Local Postgres for development and integration tests (Apple `container` CLI).
#   scripts/dev-db.sh up      start (seeds dev/postgres/init.sql on first boot)
#   scripts/dev-db.sh down    stop and delete the container (data is discarded)
#   scripts/dev-db.sh reset   down + up
#   scripts/dev-db.sh psql    open psql inside the container
#   scripts/dev-db.sh logs    show container logs
#
# Connection: postgres://postgres:postgres@localhost:54329/app_dev
# (port 54329 so it doesn't clash with a Postgres already on 5432)
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=dbgui-postgres
IMAGE=postgres:17
PORT=54329

wait_ready() {
  printf 'waiting for postgres'
  for _ in $(seq 1 60); do
    # The entrypoint runs init.sql on a temporary server first; wait for the real one.
    if container logs "$NAME" 2>/dev/null | grep -q 'PostgreSQL init process complete' &&
       container exec "$NAME" pg_isready -q -U postgres -d app_dev 2>/dev/null; then
      echo ' ready'
      echo "postgres://postgres:postgres@localhost:$PORT/app_dev"
      return 0
    fi
    if container logs "$NAME" 2>/dev/null | grep -q 'init.sql:[0-9]*: ERROR'; then
      echo ' seed failed:' >&2
      container logs "$NAME" | grep -A2 'init.sql:[0-9]*: ERROR' >&2
      return 1
    fi
    printf '.'
    sleep 1
  done
  echo ' timed out' >&2
  container logs "$NAME" | tail -20 >&2
  return 1
}

up() {
  if container inspect "$NAME" >/dev/null 2>&1; then
    container start "$NAME" >/dev/null 2>&1 || true
  else
    container run -d --name "$NAME" \
      -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=app_dev \
      -p "127.0.0.1:$PORT:5432" \
      -v "$PWD/dev/postgres:/docker-entrypoint-initdb.d:ro" \
      "$IMAGE" >/dev/null
  fi
  wait_ready
}

down() {
  container stop "$NAME" >/dev/null 2>&1 || true
  container rm "$NAME" >/dev/null 2>&1 || true
}

case "${1:-up}" in
  up) up ;;
  down) down ;;
  reset) down; up ;;
  psql) container exec -it "$NAME" psql -U postgres -d app_dev ;;
  logs) container logs "$NAME" ;;
  *) echo "usage: $0 up|down|reset|psql|logs" >&2; exit 2 ;;
esac

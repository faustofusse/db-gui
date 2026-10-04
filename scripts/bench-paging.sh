#!/usr/bin/env bash
# Benchmarks deep table pages, OFFSET vs keyset, on big seeded tables in throwaway containers
# (never the dev ones). Removes the containers afterwards unless --keep.
#   scripts/bench-paging.sh [--keep] [postgres|mysql|sqlite]...   (default: all)
set -euo pipefail
cd "$(dirname "$0")/.."

PG_NAME=dbear-keyset-bench-pg PG_PORT=54349 PG_ROWS=${PG_ROWS:-1000000}
MY_NAME=dbear-keyset-bench-mysql MY_PORT=33089 MY_ROWS=${MY_ROWS:-1000000}
KEEP=0
engines=()
for arg in "$@"; do
  case $arg in
    --keep) KEEP=1 ;;
    *) engines+=("$arg") ;;
  esac
done
[ ${#engines[@]} -eq 0 ] && engines=(postgres mysql sqlite)

cargo build --release -q -p dbcore --example bench_paging
bench() { ./target/release/examples/bench_paging "$@"; echo; }

cleanup() {
  [ "$KEEP" = 1 ] && return
  for name in "$PG_NAME" "$MY_NAME"; do
    container stop "$name" >/dev/null 2>&1 || true
    container rm "$name" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

wait_until() { # description, command
  printf 'waiting for %s' "$1"
  for _ in $(seq 1 180); do
    if eval "$2" >/dev/null 2>&1; then echo ' ready'; return 0; fi
    printf '.'; sleep 1
  done
  echo ' timed out' >&2; return 1
}

for engine in "${engines[@]}"; do
  case $engine in
  postgres)
    container inspect "$PG_NAME" >/dev/null 2>&1 ||
      container run -d --name "$PG_NAME" -e POSTGRES_PASSWORD=postgres -p "127.0.0.1:$PG_PORT:5432" postgres:17 >/dev/null
    wait_until "$PG_NAME" "container exec $PG_NAME psql -U postgres -tAc 'select 1'"
    echo "seeding $PG_ROWS rows…"
    container exec -i "$PG_NAME" psql -q -U postgres -v ON_ERROR_STOP=1 <<SQL
drop table if exists bench;
create table bench (
  id bigint generated always as identity primary key,
  user_id int, kind text not null, amount numeric(10,2), created_at timestamptz not null
);
insert into bench (user_id, kind, amount, created_at)
select case when i % 10 = 0 then null else i % 50000 end,
       (array['view','click','signup','purchase'])[1 + i % 4],
       case when i % 7 = 0 then null else (i % 100000) / 100.0 end,
       timestamptz '2020-01-01' + i * interval '13 seconds'
from generate_series(1, $PG_ROWS) i;
create index on bench (created_at);
create index on bench (user_id, id);
vacuum analyze bench;
SQL
    url="postgres://postgres:postgres@127.0.0.1:$PG_PORT/postgres?sslmode=disable"
    bench "$url" public bench
    bench "$url" public bench created_at:desc
    bench "$url" public bench user_id
    bench "$url" public bench amount
    ;;
  mysql)
    container inspect "$MY_NAME" >/dev/null 2>&1 ||
      container run -d --name "$MY_NAME" -e MYSQL_ROOT_PASSWORD=mysql -e MYSQL_DATABASE=bench -p "127.0.0.1:$MY_PORT:3306" mysql:8.4 >/dev/null
    wait_until "$MY_NAME" "container logs $MY_NAME 2>&1 | grep -q 'ready for connections.*port: 3306' && container exec $MY_NAME mysql -uroot -pmysql -e 'select 1' bench"
    echo "seeding $MY_ROWS rows…"
    container exec -i "$MY_NAME" mysql -uroot -pmysql bench 2> >(grep -v "Using a password" >&2) <<SQL
drop table if exists bench;
create table bench (
  id bigint auto_increment primary key,
  user_id int, kind varchar(16) not null, amount decimal(10,2), created_at datetime not null,
  key (created_at), key (user_id, id)
);
set session cte_max_recursion_depth = 10000000;
insert into bench (user_id, kind, amount, created_at)
with recursive n (i) as (select 1 union all select i + 1 from n where i < $MY_ROWS)
select if(i % 10 = 0, null, i % 50000), elt(1 + i % 4, 'view', 'click', 'signup', 'purchase'),
       if(i % 7 = 0, null, (i % 100000) / 100), timestamp '2020-01-01 00:00:00' + interval (i * 13) second
from n;
analyze table bench;
SQL
    url="mysql://root:mysql@127.0.0.1:$MY_PORT/bench?sslmode=disable"
    bench "$url" bench bench
    bench "$url" bench bench created_at:desc
    bench "$url" bench bench user_id
    bench "$url" bench bench amount
    ;;
  sqlite)
    dir=$(mktemp -d)
    db="$dir/bench.db"
    echo "seeding $PG_ROWS rows…"
    sqlite3 "$db" <<SQL
create table bench (id integer primary key, user_id int, kind text not null, amount numeric, created_at text not null);
with recursive n (i) as (select 1 union all select i + 1 from n where i < $PG_ROWS)
insert into bench (user_id, kind, amount, created_at)
select case when i % 10 = 0 then null else i % 50000 end, 'k' || (i % 4),
       case when i % 7 = 0 then null else (i % 100000) / 100.0 end, datetime('2020-01-01', '+' || (i * 13) || ' seconds')
from n;
create index bench_created on bench (created_at);
create index bench_user on bench (user_id, id);
SQL
    bench "sqlite://$db" main bench
    bench "sqlite://$db" main bench created_at:desc
    bench "sqlite://$db" main bench user_id
    bench "sqlite://$db" main bench amount
    rm -rf "$dir"
    ;;
  *) echo "unknown engine: $engine" >&2; exit 2 ;;
  esac
done

-- Seed data for the local dev Postgres (scripts/dev-db.sh).
-- Covers the column types the driver has to decode: ints, numeric, bool, text, uuid, jsonb,
-- arrays, enums, timestamps/date/interval, inet, bytea, plus views, a materialized view,
-- a partitioned table, a table without a primary key and an empty schema.

create extension if not exists pgcrypto;

create schema billing;
create schema analytics;
create schema archive; -- intentionally empty

create type order_status as enum ('pending', 'paid', 'shipped', 'delivered', 'canceled', 'refunded');

-- public -------------------------------------------------------------------

create table users (
  id            bigint generated always as identity primary key,
  name          text not null,
  email         text not null unique,
  is_admin      boolean not null default false,
  settings      jsonb not null default '{}',
  tags          text[] not null default '{}',
  last_login_at timestamptz,
  created_at    timestamptz not null default now()
);

insert into users (name, email, is_admin, settings, tags, last_login_at, created_at)
select
  n.name,
  lower(replace(n.name, ' ', '.')) || i || '@example.com',
  i % 17 = 0,
  jsonb_build_object('theme', (array['light','dark','system'])[1 + i % 3], 'beta', i % 5 = 0),
  string_to_array((array['admin,staff', 'customer,vip', 'customer'])[1 + i % 3], ','),
  case when i % 4 = 1 then null else timestamptz '2025-06-01' + (i * interval '7 hours') end,
  timestamptz '2024-01-01' + (i * interval '1 day 3 hours')
from generate_series(1, 248) as i
cross join lateral (
  select (array['Ada Lovelace','Alan Turing','Grace Hopper','Linus Torvalds','Barbara Liskov',
                'Ken Thompson','Dennis Ritchie','Margaret Hamilton','Edsger Dijkstra','Donald Knuth'])[1 + i % 10] as name
) n;

create table products (
  id       bigint generated always as identity primary key,
  sku      text not null unique,
  name     text not null,
  price    numeric(10, 2) not null,
  weight_g real,
  in_stock boolean not null default true
);

insert into products (sku, name, price, weight_g, in_stock)
select
  format('SKU-%s', lpad((i * 17)::text, 5, '0')),
  initcap((array['alpha','bravo','charlie','delta','echo','foxtrot','golf','hotel'])[1 + i % 8]) || ' ' ||
    (array['Widget','Gadget','Gizmo','Doohickey'])[1 + i % 4],
  round((4.99 + (i * 1733 % 50000) / 100.0)::numeric, 2),
  case when i % 6 = 0 then null else (50 + i * 3.7)::real end,
  i % 3 <> 0
from generate_series(1, 86) as i;

create table orders (
  id         bigint generated always as identity primary key,
  user_id    bigint not null references users (id),
  status     order_status not null,
  total      numeric(12, 2) not null,
  notes      text,
  created_at timestamptz not null
);

insert into orders (user_id, status, total, notes, created_at)
select
  1 + (i * 37) % 248,
  (enum_range(null::order_status))[1 + i % 6],
  round((10 + (i * 7919 % 90000) / 100.0)::numeric, 2),
  case when i % 4 = 1 then null else 'Leave at the door #' || i end,
  timestamptz '2025-01-01' + (i * interval '5 hours 13 minutes')
from generate_series(1, 1204) as i;

create table sessions (
  id         uuid primary key default gen_random_uuid(),
  user_id    bigint not null references users (id),
  ip         inet not null,
  user_agent text,
  token      bytea not null,
  ttl        interval not null,
  expires_at timestamptz not null
);

insert into sessions (user_id, ip, user_agent, token, ttl, expires_at)
select
  1 + (i * 13) % 248,
  ('10.0.' || i % 255 || '.' || (i * 3) % 255)::inet,
  (array['Safari/18', 'Firefox/131', 'Chrome/130', null])[1 + i % 4],
  gen_random_bytes(16),
  (1 + i % 48) * interval '1 hour',
  timestamptz '2026-01-01' + i * interval '1 hour'
from generate_series(1, 512) as i;

create view active_users as
  select id, name, email, last_login_at
  from users
  where last_login_at > timestamptz '2025-06-10';

-- billing ------------------------------------------------------------------

create table billing.invoices (
  id       bigint generated always as identity primary key,
  order_id bigint not null references orders (id),
  amount   numeric(12, 2) not null,
  paid     boolean not null,
  due_on   date not null
);

insert into billing.invoices (order_id, amount, paid, due_on)
select id, total, status in ('paid', 'shipped', 'delivered'), (created_at + interval '30 days')::date
from orders where id <= 930;

create table billing.payments (
  id         bigint generated always as identity primary key,
  invoice_id bigint not null references billing.invoices (id),
  provider   text not null,
  amount     numeric(12, 2) not null,
  -- exact numeric beyond float precision: must survive the round trip
  fx_rate    numeric(30, 20) not null,
  created_at timestamptz not null
);

insert into billing.payments (invoice_id, provider, amount, fx_rate, created_at)
select
  inv.id,
  (array['stripe','paypal','mercadopago'])[1 + inv.id % 3],
  inv.amount,
  1234.56789012345678901234 + inv.id,
  inv.due_on - 3
from billing.invoices inv
where inv.paid;

create table billing.subscriptions (
  id          bigint generated always as identity primary key,
  user_id     bigint not null references users (id),
  plan        text not null,
  status      text not null,
  canceled_at timestamptz
);

insert into billing.subscriptions (user_id, plan, status, canceled_at)
select
  1 + i * 4,
  (array['free','pro','team'])[1 + i % 3],
  case when i % 5 = 0 then 'canceled' else 'active' end,
  case when i % 5 = 0 then timestamptz '2025-09-01' + i * interval '1 day' end
from generate_series(1, 61) as i;

-- analytics ----------------------------------------------------------------

-- partitioned table: only the parent should be listed
create table analytics.events (
  id         bigint generated always as identity,
  user_id    bigint,
  name       text not null,
  path       text not null,
  props      jsonb,
  created_at timestamptz not null,
  primary key (id, created_at)
) partition by range (created_at);

create table analytics.events_2025_h1 partition of analytics.events
  for values from ('2025-01-01') to ('2025-07-01');
create table analytics.events_2025_h2 partition of analytics.events
  for values from ('2025-07-01') to ('2026-01-01');

insert into analytics.events (user_id, name, path, props, created_at)
select
  case when i % 4 = 1 then null else 1 + i % 248 end,
  (array['page_view','click','signup','purchase'])[1 + i % 4],
  '/' || (array['home','pricing','docs','blog','account'])[1 + i % 5],
  case when i % 3 = 0 then jsonb_build_object('ref', 'campaign-' || i % 7) end,
  timestamptz '2025-01-01' + (i * interval '10 minutes 30 seconds')
from generate_series(1, 50000) as i;

-- no primary key: paging falls back to ctid ordering
create table analytics.raw_log (
  line       text,
  level      smallint,
  logged_at  timestamp
);

insert into analytics.raw_log
select 'log line ' || i, (i % 5)::smallint, timestamp '2025-03-01' + i * interval '1 second'
from generate_series(1, 300) as i;

create materialized view analytics.daily_signups as
  select created_at::date as day, count(*) as count
  from users
  group by 1
  order by 1;

analyze;

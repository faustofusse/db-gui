-- Seed for the dev SQLite file (scripts/dev-db.sh up sqlite → dev/sqlite/app.db).
-- Also loaded by the core's SQLite tests into a temporary file.

create table notes (
  id          integer primary key,
  title       text not null,
  body        text,
  pinned      boolean not null default 0,
  word_count  integer,
  score       real,
  price       decimal(10,2),
  attachment  blob,
  created_at  text not null
);

create table tags (
  id    integer primary key,
  name  text not null unique
);

create table note_tags (
  note_id  integer not null references notes (id) on delete cascade,
  tag_id   integer not null references tags (id) on delete cascade,
  primary key (note_id, tag_id)
) without rowid;

-- No declared primary key: paged by rowid.
create table settings (
  key    text not null,
  value  any
);

-- Larger table for paging.
create table events (
  id          integer primary key,
  kind        text not null,
  created_at  text not null
);

with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 120)
insert into notes (title, body, pinned, word_count, score, price, attachment, created_at)
select
  'Note #' || n,
  case when n % 6 = 0 then null else 'Body of note ' || n || ' — ¡hola! 🎉' end,
  n % 4 = 0,
  n * 13 % 900,
  case when n % 5 = 0 then null else (n % 50) / 10.0 end,
  case when n % 3 = 0 then '19.90' else null end,
  case when n % 10 = 0 then randomblob(8) else null end,
  datetime('2025-01-01', '+' || n || ' hours')
from seq;

insert into tags (name) values ('work'), ('home'), ('ideas'), ('todo'), ('archive');

insert into note_tags (note_id, tag_id)
select id, 1 + id % 5 from notes
union
select id, 1 + (id * 3) % 5 from notes where id % 2 = 0;

insert into settings (key, value) values
  ('theme', 'dark'), ('font_size', 13), ('zoom', 1.25), ('last_sync', null), ('token', x'deadbeef');

with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 20000)
insert into events (kind, created_at)
select
  case n % 4 when 0 then 'open' when 1 then 'edit' when 2 then 'share' else 'close' end,
  datetime('2025-01-01', '+' || n || ' minutes')
from seq;

create view pinned_notes as
select id, title, created_at from notes where pinned;

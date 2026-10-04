-- Round-trip fixture for SQLite dumps (crates/dbcore/tests/dump_sqlite.rs): awkward values,
-- generated columns, WITHOUT ROWID, AUTOINCREMENT, triggers with bodies, partial/expression indexes.

create table "we""ird table" (
  id     integer primary key autoincrement,
  [name] text not null,
  amount real,
  data   blob,
  any_v  any,
  slug   text generated always as (lower([name])) virtual,
  total  real generated always as (amount * 2) stored
);

create table kv (
  k text primary key,
  v text
) without rowid;

create table child (
  id      integer primary key,
  parent  integer references "we""ird table" (id) on delete cascade,
  note    text default 'n/a'
);

create table audit (id integer primary key, msg text);

create index child_parent on child (parent) where parent is not null;
create unique index kv_lower on kv (lower(k));

create view big_amounts as select id, [name] from "we""ird table" where amount > 10;
create view big_names as select upper([name]) as n from big_amounts;

create trigger child_audit after insert on child
begin
  insert into audit (msg) values (case when new.note is null then 'none' else 'note: ' || new.note end);
  insert into audit (msg) values ('two; statements');
end;

insert into "we""ird table" ([name], amount, data, any_v) values
  ('plain', 1.5, x'00ff10', 42),
  ('it''s; "quoted"', -0.0, x'', 'text'),
  ('üñíçødé 🐻', 1e300, null, 3.25),
  ('newline
and tab	here', 0.1, randomblob(64), x'0102'),
  ('nul', 12.0, null, cast(x'610062' as text)),
  ('inf', 9e999, null, -9e999),
  ('big int', 9223372036854775807, null, -9223372036854775808);
delete from "we""ird table" where [name] = 'nothing';

insert into kv values ('a', '1'), ('B', null), ('x;y', 'semi');
insert into child (parent, note) values (1, 'first'), (2, null), (null, 'orphan');

with recursive n(i) as (select 1 union all select i + 1 from n where i < 2000)
insert into kv select 'key' || i, hex(randomblob(8)) from n;

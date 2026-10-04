-- Round-trip fixture for MySQL dumps (crates/dbcore/tests/dump_mysql.rs), loaded into a scratch
-- database: every common type, generated columns, foreign keys, views on views, routines,
-- a trigger, a MyISAM table and awkward values.

create table customers (
  id       int unsigned auto_increment primary key,
  email    varchar(100) not null unique,
  name     varchar(100),
  mood     enum('sad', 'ok', 'happy') default 'ok',
  flags    set('a', 'b', 'c'),
  balance  decimal(10, 2),
  ratio    double,
  small    float,
  big      bigint unsigned,
  bits     bit(10),
  born     date,
  seen     datetime(6),
  stamp    timestamp null default null,
  t        time,
  y        year,
  meta     json,
  photo    blob,
  raw      varbinary(16),
  fixed    binary(4),
  note     text,
  slug     varchar(110) generated always as (lower(name)) virtual,
  shout    varchar(110) generated always as (upper(name)) stored,
  pt       point null,
  key name_prefix (name(10)),
  fulltext key note_text (note)
) engine = InnoDB auto_increment = 50 comment = 'People; who buy';

create table orders (
  id           bigint auto_increment primary key,
  customer_id  int unsigned not null,
  total        decimal(12, 2) not null,
  placed_on    date not null,
  note         varchar(200),
  constraint orders_customer foreign key (customer_id) references customers (id) on delete cascade,
  index placed (placed_on desc)
);

create table logs (id int, msg varchar(50)) engine = MyISAM;
create table `we``ird` (`semi;colon` int primary key, `back\slash` text);

create view order_totals as
  select c.email, sum(o.total) as total from customers c join orders o on o.customer_id = c.id group by c.email;
create view big_spenders as select * from order_totals where total > 100;

create procedure add_log(in m varchar(50))
begin
  insert into logs values (1, m);
  insert into logs values (2, concat(m, '; again'));
end;

create function double_it(x int) returns int deterministic no sql begin return x * 2; end;

create trigger orders_note before insert on orders for each row
begin
  if new.note is null then set new.note = 'auto; filled'; end if;
end;

insert into customers (email, name, mood, flags, balance, ratio, small, big, bits, born, seen, stamp, t, y, meta, photo, raw, fixed, note, pt) values
  ('a@x.io', 'Ann O''Neil', 'happy', 'a,c', 10.5, 0.1, 1.5, 18446744073709551615, b'1010101010', '2024-02-29', '2024-02-29 12:34:56.123456', '2024-03-01 00:00:01', '-838:59:59', 2024, '{"k": [1, 2, {"z": null}]}', 0x00FF0A5C, 0x00, 0xDEADBEEF, 'back\\slash "dq" \Z nul:\0 end', ST_GeomFromText('POINT(1 2)')),
  ('b@x.io', 'tab\there\nnewline', 'sad', '', -1, -1e300, null, 0, b'0', null, null, null, null, null, '"str"', '', '', null, '', null),
  ('c@x.io', 'üñí 🐻', null, null, null, null, null, null, null, null, null, null, null, null, null, null, null, null, null, null);
insert into orders (customer_id, total, placed_on, note)
  with recursive n(i) as (select 1 union all select i + 1 from n where i < 500)
  select 50 + i % 3, i * 1.25, date '2024-01-01' + interval i day, if(i % 2 = 0, concat('n', i), null) from n;
insert into logs values (7, 'myisam');
insert into `we``ird` values (1, 'a;b'), (2, null);

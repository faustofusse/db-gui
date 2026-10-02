-- Seed for the dev MySQL (scripts/dev-db.sh up mysql). Runs once, on first boot.
-- Databases are schemas in MySQL; dbear lists them all as sections of one connection.

-- Generated rows use recursive CTEs (default depth limit: 1000).
set session cte_max_recursion_depth = 100000;

create database shop;
create database blog;
create database archive; -- intentionally empty

use shop;

create table customers (
  id          int unsigned auto_increment primary key,
  email       varchar(255) not null unique,
  name        varchar(120) not null,
  is_active   boolean not null default true,
  balance     decimal(12,2) not null default 0,
  rating      double,
  country     char(2),
  tags        set('vip', 'beta', 'wholesale'),
  avatar      blob,
  created_at  datetime(3) not null default current_timestamp(3)
);

create table products (
  id          bigint unsigned auto_increment primary key,
  sku         varchar(32) not null unique,
  name        varchar(200) not null,
  price       decimal(10,2) not null,
  weight_kg   float,
  stock       mediumint not null default 0,
  attributes  json,
  released    year,
  flags       bit(8) not null default b'00000000'
);

create table orders (
  id           bigint unsigned auto_increment primary key,
  customer_id  int unsigned not null,
  status       enum('pending', 'paid', 'shipped', 'cancelled') not null default 'pending',
  total        decimal(12,2) not null,
  fx_rate      decimal(30,20),
  notes        text,
  placed_at    timestamp not null default current_timestamp,
  ship_by      date,
  foreign key (customer_id) references customers (id)
);

create table order_items (
  order_id    bigint unsigned not null,
  product_id  bigint unsigned not null,
  quantity    smallint not null,
  unit_price  decimal(10,2) not null,
  primary key (order_id, product_id)
);

-- No primary key: paged without an ORDER BY.
create table audit_log (
  happened_at datetime not null,
  actor       varchar(64),
  action      varchar(64) not null,
  payload     json
);

-- Big enough to exercise paging and estimated counts.
create table events (
  id          bigint unsigned auto_increment primary key,
  customer_id int unsigned,
  kind        varchar(32) not null,
  big_counter bigint unsigned not null,
  created_at  datetime not null
);

insert into customers (email, name, is_active, balance, rating, country, tags, avatar, created_at)
with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 250)
select
  concat('customer', n, '@example.com'),
  elt(1 + n % 8, 'Ada Lovelace', 'Alan Turing', 'Grace Hopper', 'Linus Torvalds',
      'Barbara Liskov', 'Ken Thompson', 'Margaret Hamilton', 'Donald Knuth'),
  n % 7 <> 0,
  round((n * 37.13) % 5000, 2),
  if(n % 5 = 0, null, (n % 50) / 10),
  elt(1 + n % 4, 'AR', 'US', 'DE', 'JP'),
  elt(1 + n % 4, 'vip', 'beta,wholesale', '', null),
  if(n % 3 = 0, unhex(md5(n)), null),
  timestamp('2025-01-01') + interval n hour
from seq;

insert into products (sku, name, price, weight_kg, stock, attributes, released, flags)
with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 80)
select
  concat('SKU-', lpad(n * 17, 5, '0')),
  concat(elt(1 + n % 5, 'Keyboard', 'Mouse', 'Monitor', 'Cable', 'Dock'), ' #', n),
  round(4.99 + (n * 13.7) % 900, 2),
  if(n % 6 = 0, null, (n % 30) / 3),
  (n * 7) % 500,
  json_object('color', elt(1 + n % 3, 'black', 'white', 'silver'), 'wireless', n % 2 = 0),
  2015 + n % 10,
  n % 256
from seq;

insert into orders (customer_id, status, total, fx_rate, notes, placed_at, ship_by)
with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 1200)
select
  1 + (n * 31) % 250,
  elt(1 + n % 4, 'pending', 'paid', 'shipped', 'cancelled'),
  round(10 + (n * 17.31) % 2000, 2),
  if(n % 4 = 0, null, 1.12345678901234567890 * (1 + n % 3)),
  if(n % 9 = 0, 'Leave at the door, it''s "fragile"', null),
  timestamp('2025-02-01') + interval n * 37 minute,
  date('2025-02-05') + interval n % 60 day
from seq;

insert into order_items (order_id, product_id, quantity, unit_price)
select o.id, 1 + (o.id + k.k * 7) % 80, 1 + (o.id + k.k) % 4, round(4.99 + (o.id * k.k) % 300, 2)
from orders o
cross join (select 1 as k union all select 2 union all select 3) k;

insert into audit_log (happened_at, actor, action, payload)
with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 40)
select
  timestamp('2025-03-01') + interval n minute,
  if(n % 5 = 0, null, elt(1 + n % 3, 'admin', 'system', 'support')),
  elt(1 + n % 4, 'login', 'refund', 'export', 'delete'),
  json_object('n', n, 'ok', n % 2 = 0)
from seq;

insert into events (customer_id, kind, big_counter, created_at)
with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 50000)
select
  if(n % 10 = 0, null, 1 + n % 250),
  elt(1 + n % 5, 'page_view', 'click', 'signup', 'purchase', 'logout'),
  18446744073709551615 - n,
  timestamp('2025-01-01') + interval n minute
from seq;

create view paid_orders as
select o.id, c.email, o.total, o.placed_at
from orders o join customers c on c.id = o.customer_id
where o.status = 'paid';

analyze table customers, products, orders, order_items, audit_log, events;

use blog;

create table posts (
  id         int auto_increment primary key,
  title      varchar(200) not null,
  slug       varchar(200) not null unique,
  body       mediumtext,
  published  tinyint(1) not null default 0,
  views      int not null default 0,
  created_at datetime not null
);

create table comments (
  id         int auto_increment primary key,
  post_id    int not null,
  author     varchar(80),
  body       text not null,
  created_at datetime not null,
  foreign key (post_id) references posts (id) on delete cascade
);

insert into posts (title, slug, body, published, views, created_at)
with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 30)
select
  concat('Post number ', n),
  concat('post-', n),
  repeat(concat('Paragraph ', n, '. '), 1 + n % 5),
  n % 3 <> 0,
  n * 113 % 5000,
  timestamp('2024-06-01') + interval n day
from seq;

insert into comments (post_id, author, body, created_at)
with recursive seq (n) as (select 1 union all select n + 1 from seq where n < 90)
select
  1 + n % 30,
  if(n % 4 = 0, null, concat('reader', n % 12)),
  concat('Comment ', n, ' — ¡qué bueno! 🎉'),
  timestamp('2024-06-02') + interval n hour
from seq;

create view popular_posts as
select id, title, views from posts where views > 2000;

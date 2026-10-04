-- Seed for the dev SQL Server (scripts/dev-db.sh up sqlserver). Run once by sqlcmd after the
-- first boot (the image has no init directory). Databases: app_dev (schemas dbo, sales) and archive.

create database app_dev;
go
create database archive; -- intentionally empty
go
use app_dev;
go
create schema sales;
go
create schema empty_schema;
go

-- Every type the driver decodes, with edge values.
create table dbo.types (
  id              int identity(1,1) not null constraint pk_types primary key,
  tiny            tinyint,
  small           smallint,
  big             bigint,
  flag            bit,
  real_num        real,
  float_num       float,
  exact           decimal(38,10),
  price           money,
  small_price     smallmoney,
  guid            uniqueidentifier,
  name            nvarchar(100),
  ascii_name      varchar(50),
  fixed           char(3),
  memo            nvarchar(max),
  payload         varbinary(max),
  bin             binary(4),
  born            date,
  alarm           time(7),
  legacy          datetime,
  small_legacy    smalldatetime,
  happened        datetime2(7),
  happened_tz     datetimeoffset(7),
  doc             xml,
  variant         sql_variant,
  version         rowversion
);

insert into dbo.types (tiny, small, big, flag, real_num, float_num, exact, price, small_price, guid, name, ascii_name, fixed,
                       memo, payload, bin, born, alarm, legacy, small_legacy, happened, happened_tz, doc, variant)
values (255, -32768, 9223372036854775807, 1, 0.1, 3.141592653589793, 1234567890123456789012345678.9012345678,
        922337203685477.5807, -214748.3648, '6F9619FF-8B86-D011-B42D-00C04FC964FF', N'Zoë 日本語 🐻', 'plain', 'abc',
        replicate(cast(N'x' as nvarchar(max)), 5000), 0xDEADBEEF, 0x00010203, '2024-01-02', '03:04:05.1234567',
        '2024-01-02T03:04:05.003', '2024-01-02T03:04:00', '2024-01-02T03:04:05.1234567',
        '2024-01-02T03:04:05.1234567+02:00', N'<a b="1">x</a>', cast(42 as int)),
       (null, null, null, 0, null, null, -0.5, 0, 0, null, null, null, null, null, null, null, null, null, null, null,
        null, null, null, null);

create table dbo.customers (
  id          int identity(1,1) not null primary key,
  email       nvarchar(255) not null unique,
  name        nvarchar(120) not null,
  is_active   bit not null default 1,
  balance     decimal(12,2) not null default 0,
  created_at  datetime2(3) not null default sysdatetime()
);
exec sys.sp_addextendedproperty @name = N'MS_Description', @value = N'Login e-mail',
  @level0type = N'SCHEMA', @level0name = N'dbo', @level1type = N'TABLE', @level1name = N'customers',
  @level2type = N'COLUMN', @level2name = N'email';

with n as (select top (250) row_number() over (order by (select null)) as i from sys.all_objects a cross join sys.all_objects b)
insert into dbo.customers (email, name, is_active, balance, created_at)
select concat(N'user', i, N'@example.com'), concat(N'Customer ', i), case when i % 7 = 0 then 0 else 1 end,
       cast(i * 37.13 % 1000 as decimal(12,2)), dateadd(hour, cast(i as int), cast('2025-01-01T00:00:00' as datetime2(3)))
from n;

create table sales.orders (
  id           bigint identity(1,1) not null constraint pk_orders primary key,
  customer_id  int not null constraint fk_orders_customers references dbo.customers (id) on delete cascade,
  status       varchar(20) not null constraint df_orders_status default 'pending',
  total        decimal(12,2) not null constraint ck_orders_total check (total >= 0),
  tax          as (total * 0.2) persisted,
  placed_at    datetime2(0) not null default sysdatetime()
);
create index ix_orders_status on sales.orders (status desc) include (total) where status <> 'cancelled';

insert into sales.orders (customer_id, status, total)
select top (300) c.id, case c.id % 3 when 0 then 'paid' when 1 then 'pending' else 'shipped' end, c.id * 1.5
from dbo.customers c cross join (values (1), (2)) v(x);

-- Composite key.
create table sales.order_items (
  order_id    bigint not null,
  line        int not null,
  quantity    smallint not null,
  constraint pk_order_items primary key (order_id, line)
);
insert into sales.order_items values (1, 1, 2), (1, 2, 1), (2, 1, 5);

-- No primary key, but a unique index on NOT NULL columns: pages are ordered by it.
create table dbo.audit_log (
  seq     int not null,
  message nvarchar(200) not null
);
create unique index ux_audit_log_seq on dbo.audit_log (seq);
insert into dbo.audit_log (seq, message)
select top (40) row_number() over (order by (select null)), N'event' from sys.all_objects;

-- Heap: no key at all.
create table dbo.heap_notes (note nvarchar(50));
insert into dbo.heap_notes values (N'one'), (N'two'), (N'three');

go
create view sales.paid_orders as
  select o.id, o.total, c.email from sales.orders o join dbo.customers c on c.id = o.customer_id where o.status = 'paid';
go

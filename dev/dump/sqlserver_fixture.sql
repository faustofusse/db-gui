-- Round-trip fixture for SQL Server dumps (crates/dbcore/tests/dump_sqlserver.rs), loaded into a
-- scratch database: identity/computed/default columns, keys, checks, foreign keys, clustered and
-- filtered indexes, views on views, a procedure, a function, a trigger and awkward values.

create schema sales;
GO
create function dbo.double_it(@x int) returns int with schemabinding as begin return @x * 2 end;
GO
create table sales.customers (
  id        int identity(10, 5) constraint pk_customers primary key,
  email     nvarchar(100) not null constraint uq_customers_email unique,
  name      nvarchar(100) collate Latin1_General_CS_AS null,
  balance   decimal(10, 2) not null constraint df_balance default 0 constraint ck_balance check (balance >= 0),
  ratio     float null,
  small     real null,
  cash      money null,
  flag      bit not null constraint df_flag default 1,
  born      date null,
  seen      datetime2(3) null,
  old       datetime null,
  small_dt  smalldatetime null,
  t         time(4) null,
  at        datetimeoffset null,
  guid      uniqueidentifier not null constraint df_guid default newid(),
  photo     varbinary(max) null,
  fixed     binary(4) null,
  ascii     varchar(20) null,
  doc       xml null,
  doubled   as dbo.double_it(id),
  rv        rowversion,
  notes     nvarchar(max) null
);
create table sales.orders (
  id           bigint identity constraint pk_orders primary key nonclustered,
  customer_id  int not null constraint fk_orders_customer references sales.customers (id) on delete cascade,
  total        decimal(12, 2) not null,
  placed       date not null,
  note         nvarchar(200) null
);
create clustered index orders_placed on sales.orders (placed desc);
create index orders_customer on sales.orders (customer_id) include (total) where note is not null;
create table dbo.[we]]ird] ([semi;colon] int not null constraint [pk we]]ird] primary key, [back\slash] nvarchar(10) null);
GO
create view sales.order_totals as
  select c.email, sum(o.total) as total from sales.customers c join sales.orders o on o.customer_id = c.id group by c.email;
GO
create view sales.big_spenders as select * from sales.order_totals where total > 100;
GO
create procedure sales.add_order @customer int, @total decimal(12, 2) as
begin
  set nocount on;
  insert into sales.orders (customer_id, total, placed) values (@customer, @total, getdate());
end;
GO
create trigger sales.orders_note on sales.orders after insert as
begin
  set nocount on;
  update o set note = N'auto; filled' from sales.orders o join inserted i on i.id = o.id where o.note is null and i.total < 0;
end;
GO
insert into sales.customers (email, name, balance, ratio, small, cash, flag, born, seen, old, small_dt, t, at, photo, fixed, ascii, doc, notes) values
  (N'a@x.io', N'Ann O''Neil', 10.5, 0.1, 0.1, 12.3456, 1, '2024-02-29', '2024-02-29 12:34:56.789', '2024-01-02 03:04:05.997',
   '2024-01-02 03:04:00', '23:59:59.1234', '2024-01-02 03:04:05.1234567 +02:00', 0x00FF0A5C, 0xDEADBEEF, 'plain', N'<a b="1">x &amp; y</a>', N'üñí 🐻'),
  (N'b@x.io', N'tab	here
newline', 0, -1e300, null, -0.0001, 0, null, null, null, null, null, null, 0x, null, '', null, N'GO'),
  (N'c@x.io', null, 1, null, null, null, 1, null, null, null, null, null, null, null, null, null, null, null);
insert into sales.orders (customer_id, total, placed, note)
  select 10 + 5 * (n % 3), n * 1.25, dateadd(day, n, '2024-01-01'), case when n % 2 = 0 then concat(N'n', n) end
  from (select top 500 row_number() over (order by (select null)) as n from sys.all_objects a cross join sys.all_objects b) numbers;
delete from sales.orders where id > 495;
insert into dbo.[we]]ird] values (1, N'a;b'), (2, null);

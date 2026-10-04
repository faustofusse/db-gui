//! SQL Server dumps: a T-SQL script of `GO`-separated batches, as sqlcmd and SSMS run them
//! (and [`crate::restore`]).
//!
//! Order: schemas, functions that read no tables, tables (columns with types, identity, computed
//! columns, defaults, collations, primary keys, unique and check constraints), rows as batched
//! INSERTs (with `SET IDENTITY_INSERT`), then indexes, foreign keys, and views, procedures,
//! functions and triggers in dependency order. Rows are read in a `SNAPSHOT` transaction when the
//! database allows it.
//!
//! Not included (noted in the file): permissions, users and roles, extended properties,
//! sequences, synonyms, user-defined table types, CLR objects, partition schemes, and full-text,
//! XML, spatial and columnstore indexes, and CLR-typed values (`hierarchyid`, `geometry`…).
//! Alias types are written as their base type; `sql_variant` values keep their base type.

use std::collections::{HashMap, HashSet};

use futures_util::TryStreamExt;
use tiberius::{ColumnData, QueryItem};

use super::order::topo_sort;
use super::{Ctx, DumpPhase, DumpScope};
use crate::dialect::Dialect;
use crate::driver::Result;
use crate::model::{ConnectionConfig, DatabaseKind, Value};
use crate::sqlserver::decode::{column_type, numeric, value};
use crate::sqlserver::{connect, query_error, Client};

const MSSQL: Dialect = Dialect(DatabaseKind::SqlServer);
/// T-SQL allows at most 1000 rows per `VALUES` list.
const ROWS_PER_INSERT: usize = 100;
/// A `GO` after this many INSERT statements keeps batches a reasonable size.
const INSERTS_PER_BATCH: usize = 10;
const USER_SCHEMAS: &str = "s.schema_id < 16384 and s.name not in (N'sys', N'INFORMATION_SCHEMA', N'guest')";
const NOT_INCLUDED: &str = "permissions, users and roles, extended properties, sequences, synonyms, user-defined table types, \
CLR objects, partition schemes, full-text/XML/spatial/columnstore indexes";

fn ident(name: &str) -> String {
    MSSQL.quote_ident(name)
}

fn qualified(schema: &str, name: &str) -> String {
    MSSQL.quote_relation(schema, name)
}

/// `N'it''s'`.
fn literal(text: &str) -> String {
    MSSQL.quote_literal(text)
}

async fn rows(client: &mut Client, sql: &str) -> Result<Vec<Vec<Value>>> {
    let result = async { client.simple_query(sql).await?.into_first_result().await }.await;
    let rows = result.map_err(|e| query_error(&e, 1))?;
    Ok(rows.iter().map(|r| r.cells().map(|(_, d)| value(d)).collect()).collect())
}

fn s(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        other => other.display(),
    }
}

fn opt(v: &Value) -> Option<String> {
    (!v.is_null()).then(|| v.display())
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        Value::Bool(b) => i64::from(*b),
        Value::Decimal(d) | Value::Text(d) => d.parse().unwrap_or(0),
        _ => 0,
    }
}

fn flag(v: &Value) -> bool {
    int(v) != 0
}

struct Table {
    id: i64,
    schema: String,
    name: String,
    rows: Option<u64>,
}

impl Table {
    fn qualified(&self) -> String {
        qualified(&self.schema, &self.name)
    }
}

struct Column {
    name: String,
    definition: String,
    /// Written in INSERTs: not computed, not `rowversion`, not CLR or `sql_variant`.
    insertable: bool,
    /// `sql_variant`: read with its base type and written as `CAST(… AS type)`.
    variant: bool,
    identity: bool,
    /// The identity's current value (`sys.identity_columns.last_value`), restored with `DBCC CHECKIDENT`.
    last_identity: Option<String>,
}

struct Index {
    table: i64,
    id: i64,
    name: String,
    is_primary: bool,
    is_unique: bool,
    /// A unique constraint: written with its table, like the primary key.
    is_constraint: bool,
    clustered: bool,
    filter: Option<String>,
}

struct Module {
    id: i64,
    schema: String,
    name: String,
    kind: String,
    definition: Option<String>,
    ansi_nulls: bool,
    quoted_identifier: bool,
    parent: i64,
}

struct ForeignKey {
    table: i64,
    name: String,
    columns: Vec<String>,
    referenced: String,
    referenced_columns: Vec<String>,
    on_delete: String,
    on_update: String,
    disabled: bool,
    not_trusted: bool,
}

pub(super) async fn dump(config: &ConnectionConfig, ctx: &mut Ctx) -> Result<()> {
    let mut client = connect(config, "set nocount on; set textsize -1").await?;
    let client = &mut client;
    let version = rows(client, "select @@version").await?;
    let server = version.first().and_then(|r| r.first()).map(s).unwrap_or_default();
    let server = server.lines().next().unwrap_or_default().trim().to_string();

    // One snapshot for every table, when the database allows it.
    let snapshot = rows(client, "select snapshot_isolation_state from sys.databases where database_id = db_id()").await?;
    if snapshot.first().and_then(|r| r.first()).is_some_and(|v| int(v) == 1) {
        exec(client, "set transaction isolation level snapshot; begin transaction").await?;
    } else if ctx.options.content.data() {
        ctx.warn("Rows were read without a consistent snapshot (the database doesn't allow SNAPSHOT isolation)");
    }

    super::header(ctx, config, &server);
    ctx.line(&format!("-- Not included: {NOT_INCLUDED}."));
    ctx.line("SET NOCOUNT ON;");
    ctx.line("SET DATEFORMAT ymd;");
    ctx.line("SET ANSI_NULLS ON;");
    ctx.line("SET QUOTED_IDENTIFIER ON;");
    ctx.line("GO");
    ctx.line("");

    ctx.phase(DumpPhase::Schema);
    let options = ctx.options.clone();
    let scope = &options.scope;
    let catalog = Catalog::load(client, scope).await?;
    for warning in &catalog.warnings {
        ctx.warn(warning.clone());
    }
    let content = options.content;

    if content.schema() {
        if options.drop_objects {
            write_drops(ctx, &catalog);
        }
        for schema in &catalog.schemas {
            ctx.line(&format!(
                "IF SCHEMA_ID({}) IS NULL EXEC(N'CREATE SCHEMA {}');",
                literal(schema),
                ident(schema).replace('\'', "''")
            ));
        }
        if !catalog.schemas.is_empty() {
            ctx.line("GO");
        }
        for module in catalog.modules.iter().filter(|m| catalog.early.contains(&m.id)) {
            write_module(ctx, module);
        }
        for table in &catalog.tables {
            write_table(ctx, &catalog, table);
        }
    }

    if content.data() {
        ctx.phase(DumpPhase::Data);
        ctx.set_tables_total(catalog.tables.len() as u32);
        for table in &catalog.tables {
            ctx.begin_table(format!("{}.{}", table.schema, table.name), table.rows);
            insert_rows(client, ctx, &catalog, table).await?;
            ctx.end_table();
        }
    }

    if content.schema() {
        ctx.phase(DumpPhase::PostData);
        let table_ids: HashSet<i64> = catalog.tables.iter().map(|t| t.id).collect();
        for index in catalog.indexes.iter().filter(|i| table_ids.contains(&i.table) && !i.is_primary && !i.is_constraint) {
            write_index(ctx, &catalog, index);
        }
        for fk in &catalog.foreign_keys {
            write_foreign_key(ctx, &catalog, fk);
        }
        for (table, name) in &catalog.disabled_checks {
            ctx.line(&format!("ALTER TABLE {table} NOCHECK CONSTRAINT {};", ident(name)));
        }
        if !catalog.foreign_keys.is_empty() || !catalog.disabled_checks.is_empty() {
            ctx.line("GO");
        }
        for module in catalog.modules.iter().filter(|m| !catalog.early.contains(&m.id)) {
            write_module(ctx, module);
            // Indexed views: their indexes come right after them.
            for index in catalog.indexes.iter().filter(|i| i.table == module.id) {
                write_index(ctx, &catalog, index);
            }
        }
    }

    if catalog.in_transaction {
        let _ = exec(client, "commit").await;
    }
    Ok(())
}

async fn exec(client: &mut Client, sql: &str) -> Result<()> {
    let result = async { client.simple_query(sql).await?.into_results().await }.await;
    result.map(drop).map_err(|e| query_error(&e, 1))
}

struct Catalog {
    schemas: Vec<String>,
    tables: Vec<Table>,
    columns: HashMap<i64, Vec<Column>>,
    indexes: Vec<Index>,
    /// (table, index) → key columns (with ASC/DESC) and included columns.
    index_columns: HashMap<(i64, i64), (Vec<String>, Vec<String>)>,
    checks: HashMap<i64, Vec<String>>,
    disabled_checks: Vec<(String, String)>,
    foreign_keys: Vec<ForeignKey>,
    /// Views, procedures, functions and triggers in dependency order (triggers last).
    modules: Vec<Module>,
    /// Functions that read no tables or views: created before the tables (checks and defaults may use them).
    early: HashSet<i64>,
    warnings: Vec<String>,
    in_transaction: bool,
}

impl Catalog {
    async fn load(client: &mut Client, scope: &DumpScope) -> Result<Self> {
        let mut warnings = Vec::new();
        let in_transaction = !rows(client, "select 1 where @@trancount > 0").await?.is_empty();

        let tables: Vec<Table> = rows(
            client,
            &format!(
                "select t.object_id, s.name, t.name, t.temporal_type,
                        (select sum(p.rows) from sys.partitions p where p.object_id = t.object_id and p.index_id in (0, 1))
                 from sys.tables t join sys.schemas s on s.schema_id = t.schema_id
                 where t.is_ms_shipped = 0 and {USER_SCHEMAS}
                 order by s.name, t.name"
            ),
        )
        .await?
        .into_iter()
        .filter(|r| scope.includes_table(&s(&r[1]), &s(&r[2])))
        .map(|r| {
            if int(&r[3]) == 2 {
                warnings.push(format!("{}.{} is dumped without system versioning", s(&r[1]), s(&r[2])));
            }
            Table { id: int(&r[0]), schema: s(&r[1]), name: s(&r[2]), rows: opt(&r[4]).and_then(|n| n.parse().ok()) }
        })
        .collect();
        let table_ids: HashSet<i64> = tables.iter().map(|t| t.id).collect();

        let mut columns: HashMap<i64, Vec<Column>> = HashMap::new();
        let mut skipped_data: Vec<String> = Vec::new();
        for r in rows(
            client,
            "select c.object_id, c.name, ty.name, c.max_length, c.precision, c.scale, c.is_nullable, c.is_identity,
                    cast(ic.seed_value as nvarchar(50)), cast(ic.increment_value as nvarchar(50)),
                    cc.definition, cc.is_persisted, dc.name, dc.definition,
                    case when c.collation_name <> cast(databasepropertyex(db_name(), 'Collation') as sysname) then c.collation_name end,
                    c.is_rowguidcol, ty.is_user_defined, bt.name, ty.is_assembly_type, cast(ic.last_value as nvarchar(50))
             from sys.columns c
             join sys.tables t on t.object_id = c.object_id
             join sys.types ty on ty.user_type_id = c.user_type_id
             left join sys.types bt on bt.user_type_id = ty.system_type_id
             left join sys.identity_columns ic on ic.object_id = c.object_id and ic.column_id = c.column_id
             left join sys.computed_columns cc on cc.object_id = c.object_id and cc.column_id = c.column_id
             left join sys.default_constraints dc on dc.object_id = c.default_object_id
             where t.is_ms_shipped = 0
             order by c.object_id, c.column_id",
        )
        .await?
        {
            let table = int(&r[0]);
            if !table_ids.contains(&table) {
                continue;
            }
            let name = s(&r[1]);
            let assembly = flag(&r[18]);
            // Alias types (`create type x from int`) as their base type.
            let type_name = if flag(&r[16]) && !assembly { s(&r[17]) } else { s(&r[2]) };
            let computed = opt(&r[10]);
            let mut definition = format!("    {}", ident(&name));
            if let Some(expression) = &computed {
                definition.push_str(&format!(" AS {expression}"));
                if flag(&r[11]) {
                    definition.push_str(" PERSISTED");
                }
            } else {
                definition.push(' ');
                definition.push_str(&column_type(&type_name, int(&r[3]), int(&r[4]), int(&r[5])));
                if let Some(collation) = opt(&r[14]) {
                    definition.push_str(&format!(" COLLATE {collation}"));
                }
                if flag(&r[7]) {
                    definition.push_str(&format!(" IDENTITY({},{})", s(&r[8]), s(&r[9])));
                }
                if flag(&r[15]) {
                    definition.push_str(" ROWGUIDCOL");
                }
                definition.push_str(if flag(&r[6]) { " NULL" } else { " NOT NULL" });
                if let (Some(name), Some(default)) = (opt(&r[12]), opt(&r[13])) {
                    definition.push_str(&format!(" CONSTRAINT {} DEFAULT {default}", ident(&name)));
                }
            }
            let unreadable = assembly;
            if unreadable {
                skipped_data.push(format!("{name} ({type_name})"));
            }
            let insertable = computed.is_none() && type_name != "timestamp" && !unreadable;
            let last_identity = opt(&r[19]);
            let variant = type_name == "sql_variant";
            columns.entry(table).or_default().push(Column { name, definition, insertable, variant, identity: flag(&r[7]), last_identity });
        }
        if !skipped_data.is_empty() {
            warnings.push(format!("Values of these columns are not dumped: {}", skipped_data.join(", ")));
        }

        let indexes: Vec<Index> = rows(
            client,
            "select i.object_id, i.index_id, i.name, i.is_primary_key, i.is_unique, i.type_desc, i.filter_definition,
                    i.is_unique_constraint, i.type
             from sys.indexes i join sys.objects o on o.object_id = i.object_id
             where o.is_ms_shipped = 0 and o.type in ('U', 'V') and i.type > 0 and i.is_hypothetical = 0
             order by i.object_id, i.index_id",
        )
        .await?
        .into_iter()
        .filter_map(|r| {
            let kind = int(&r[8]);
            if kind > 2 {
                warnings.push(format!("Skipped {} index {}", s(&r[5]).to_lowercase(), s(&r[2])));
                return None;
            }
            Some(Index {
                table: int(&r[0]),
                id: int(&r[1]),
                name: s(&r[2]),
                is_primary: flag(&r[3]),
                is_unique: flag(&r[4]),
                is_constraint: flag(&r[7]),
                clustered: kind == 1,
                filter: opt(&r[6]),
            })
        })
        .collect();

        let mut index_columns: HashMap<(i64, i64), (Vec<String>, Vec<String>)> = HashMap::new();
        for r in rows(
            client,
            "select ic.object_id, ic.index_id, c.name, ic.is_descending_key, ic.is_included_column
             from sys.index_columns ic
             join sys.columns c on c.object_id = ic.object_id and c.column_id = ic.column_id
             join sys.objects o on o.object_id = ic.object_id
             where o.is_ms_shipped = 0
             order by ic.object_id, ic.index_id, ic.is_included_column, ic.key_ordinal, ic.index_column_id",
        )
        .await?
        {
            let entry = index_columns.entry((int(&r[0]), int(&r[1]))).or_default();
            if flag(&r[4]) {
                entry.1.push(ident(&s(&r[2])));
            } else {
                entry.0.push(format!("{} {}", ident(&s(&r[2])), if flag(&r[3]) { "DESC" } else { "ASC" }));
            }
        }

        let mut checks: HashMap<i64, Vec<String>> = HashMap::new();
        let mut disabled_checks = Vec::new();
        for r in rows(client, "select parent_object_id, name, definition, is_disabled from sys.check_constraints order by parent_object_id, name").await? {
            let table = int(&r[0]);
            let Some(t) = tables.iter().find(|t| t.id == table) else { continue };
            checks.entry(table).or_default().push(format!("    CONSTRAINT {} CHECK {}", ident(&s(&r[1])), s(&r[2])));
            if flag(&r[3]) {
                disabled_checks.push((t.qualified(), s(&r[1])));
            }
        }

        let mut fk_columns: HashMap<i64, (Vec<String>, Vec<String>)> = HashMap::new();
        for r in rows(
            client,
            "select fkc.constraint_object_id, pc.name, rc.name
             from sys.foreign_key_columns fkc
             join sys.columns pc on pc.object_id = fkc.parent_object_id and pc.column_id = fkc.parent_column_id
             join sys.columns rc on rc.object_id = fkc.referenced_object_id and rc.column_id = fkc.referenced_column_id
             order by fkc.constraint_object_id, fkc.constraint_column_id",
        )
        .await?
        {
            let entry = fk_columns.entry(int(&r[0])).or_default();
            entry.0.push(ident(&s(&r[1])));
            entry.1.push(ident(&s(&r[2])));
        }
        let foreign_keys: Vec<ForeignKey> = rows(
            client,
            "select fk.object_id, fk.parent_object_id, fk.name, schema_name(rt.schema_id), rt.name,
                    fk.delete_referential_action_desc, fk.update_referential_action_desc, fk.is_disabled, fk.is_not_trusted
             from sys.foreign_keys fk join sys.objects rt on rt.object_id = fk.referenced_object_id
             order by fk.parent_object_id, fk.name",
        )
        .await?
        .into_iter()
        .filter(|r| table_ids.contains(&int(&r[1])))
        .map(|r| {
            let (columns, referenced_columns) = fk_columns.remove(&int(&r[0])).unwrap_or_default();
            ForeignKey {
                table: int(&r[1]),
                name: s(&r[2]),
                columns,
                referenced: qualified(&s(&r[3]), &s(&r[4])),
                referenced_columns,
                on_delete: s(&r[5]).replace('_', " "),
                on_update: s(&r[6]).replace('_', " "),
                disabled: flag(&r[7]),
                not_trusted: flag(&r[8]),
            }
        })
        .collect();

        // Views, procedures, functions and triggers.
        let all_modules: Vec<Module> = rows(
            client,
            "select o.object_id, schema_name(o.schema_id), o.name, rtrim(o.type), m.definition,
                    m.uses_ansi_nulls, m.uses_quoted_identifier, o.parent_object_id
             from sys.sql_modules m join sys.objects o on o.object_id = m.object_id
             where o.is_ms_shipped = 0 and o.type in ('V', 'P', 'FN', 'IF', 'TF', 'TR')
             order by o.object_id",
        )
        .await?
        .into_iter()
        .map(|r| Module {
            id: int(&r[0]),
            schema: s(&r[1]),
            name: s(&r[2]),
            kind: s(&r[3]),
            definition: opt(&r[4]),
            ansi_nulls: flag(&r[5]),
            quoted_identifier: flag(&r[6]),
            parent: int(&r[7]),
        })
        .collect();
        let mut modules: Vec<Module> = Vec::new();
        for module in all_modules {
            let wanted = match module.kind.as_str() {
                "TR" => table_ids.contains(&module.parent),
                "V" => scope.includes_table(&module.schema, &module.name),
                _ => scope.whole_schemas() && scope.includes_schema(&module.schema),
            };
            if !wanted {
                continue;
            }
            if module.definition.is_none() {
                warnings.push(format!("Skipped {}.{}: its definition is encrypted", module.schema, module.name));
                continue;
            }
            modules.push(module);
        }
        let deps: Vec<(i64, i64)> = rows(
            client,
            "select distinct d.referencing_id, d.referenced_id from sys.sql_expression_dependencies d where d.referenced_id is not null",
        )
        .await?
        .iter()
        .map(|r| (int(&r[0]), int(&r[1])))
        .collect();
        let table_like: HashSet<i64> = rows(client, "select object_id from sys.objects where type in ('U', 'V')")
            .await?
            .iter()
            .map(|r| int(&r[0]))
            .collect();
        let early: HashSet<i64> = modules
            .iter()
            .filter(|m| matches!(m.kind.as_str(), "FN" | "IF" | "TF"))
            .filter(|m| !deps.iter().any(|(from, to)| *from == m.id && table_like.contains(to)))
            .map(|m| m.id)
            .collect();
        let ids: Vec<i64> = modules.iter().map(|m| m.id).collect();
        let order: HashMap<i64, usize> = topo_sort(&ids, &deps).into_iter().enumerate().map(|(i, id)| (id, i)).collect();
        modules.sort_by_key(|m| (m.kind == "TR", order[&m.id]));

        let mut schemas: Vec<String> = Vec::new();
        if scope.whole_schemas() {
            for r in rows(client, &format!("select s.name from sys.schemas s where {USER_SCHEMAS} and s.name <> N'dbo' order by s.name")).await? {
                if scope.includes_schema(&s(&r[0])) {
                    schemas.push(s(&r[0]));
                }
            }
        } else {
            for table in &tables {
                if table.schema != "dbo" && !schemas.contains(&table.schema) {
                    schemas.push(table.schema.clone());
                }
            }
        }

        if scope.whole_schemas() {
            let counts = rows(
                client,
                "select (select count(*) from sys.sequences), (select count(*) from sys.synonyms),
                        (select count(*) from sys.table_types where is_user_defined = 1),
                        (select count(*) from sys.objects where type in ('PC', 'FS', 'FT', 'TA', 'AF'))",
            )
            .await?;
            if let Some(r) = counts.first() {
                for (n, what) in r.iter().map(int).zip(["sequence", "synonym", "table type", "CLR object"]) {
                    if n > 0 {
                        warnings.push(format!("Skipped {n} {what}{}", if n == 1 { "" } else { "s" }));
                    }
                }
            }
        }

        Ok(Self {
            schemas,
            tables,
            columns,
            indexes,
            index_columns,
            checks,
            disabled_checks,
            foreign_keys,
            modules,
            early,
            warnings,
            in_transaction,
        })
    }

    fn table(&self, id: i64) -> Option<&Table> {
        self.tables.iter().find(|t| t.id == id)
    }

    fn relation(&self, id: i64) -> Option<String> {
        self.table(id).map(Table::qualified).or_else(|| {
            self.modules.iter().find(|m| m.id == id).map(|m| qualified(&m.schema, &m.name))
        })
    }
}

fn write_drops(ctx: &mut Ctx, catalog: &Catalog) {
    for fk in &catalog.foreign_keys {
        if let Some(table) = catalog.table(fk.table) {
            let qualified_fk = format!("{}.{}", ident(&table.schema), ident(&fk.name));
            ctx.line(&format!(
                "IF OBJECT_ID({}, N'F') IS NOT NULL ALTER TABLE {} DROP CONSTRAINT {};",
                literal(&qualified_fk),
                table.qualified(),
                ident(&fk.name)
            ));
        }
    }
    let drop_module = |ctx: &mut Ctx, module: &Module| {
        let kind = match module.kind.as_str() {
            "V" => "VIEW",
            "P" => "PROCEDURE",
            _ => "FUNCTION",
        };
        ctx.line(&format!("DROP {kind} IF EXISTS {};", qualified(&module.schema, &module.name)));
    };
    // Views and procedures first, then the tables, then the functions the tables may use.
    for module in catalog.modules.iter().rev().filter(|m| m.kind != "TR" && !catalog.early.contains(&m.id)) {
        drop_module(ctx, module);
    }
    for table in catalog.tables.iter().rev() {
        ctx.line(&format!("DROP TABLE IF EXISTS {};", table.qualified()));
    }
    for module in catalog.modules.iter().rev().filter(|m| catalog.early.contains(&m.id)) {
        drop_module(ctx, module);
    }
    ctx.line("GO");
}

fn write_table(ctx: &mut Ctx, catalog: &Catalog, table: &Table) {
    let mut lines: Vec<String> = catalog.columns.get(&table.id).into_iter().flatten().map(|c| c.definition.clone()).collect();
    for index in catalog.indexes.iter().filter(|i| i.table == table.id && (i.is_primary || i.is_constraint)) {
        let (keys, _) = catalog.index_columns.get(&(table.id, index.id)).cloned().unwrap_or_default();
        let kind = if index.is_primary { "PRIMARY KEY" } else { "UNIQUE" };
        let clustered = if index.clustered { "CLUSTERED" } else { "NONCLUSTERED" };
        lines.push(format!("    CONSTRAINT {} {kind} {clustered} ({})", ident(&index.name), keys.join(", ")));
    }
    lines.extend(catalog.checks.get(&table.id).into_iter().flatten().cloned());
    ctx.line(&format!("CREATE TABLE {} (\n{}\n);", table.qualified(), lines.join(",\n")));
    ctx.line("GO");
}

fn write_index(ctx: &mut Ctx, catalog: &Catalog, index: &Index) {
    if index.is_primary || index.is_constraint {
        return;
    }
    let Some(relation) = catalog.relation(index.table) else { return };
    let (keys, included) = catalog.index_columns.get(&(index.table, index.id)).cloned().unwrap_or_default();
    let unique = if index.is_unique { "UNIQUE " } else { "" };
    let clustered = if index.clustered { "CLUSTERED" } else { "NONCLUSTERED" };
    let mut sql = format!("CREATE {unique}{clustered} INDEX {} ON {relation} ({})", ident(&index.name), keys.join(", "));
    if !included.is_empty() {
        sql.push_str(&format!(" INCLUDE ({})", included.join(", ")));
    }
    if let Some(filter) = &index.filter {
        sql.push_str(&format!(" WHERE {filter}"));
    }
    ctx.line(&format!("{sql};"));
    ctx.line("GO");
}

fn write_foreign_key(ctx: &mut Ctx, catalog: &Catalog, fk: &ForeignKey) {
    let Some(table) = catalog.table(fk.table) else { return };
    let check = if fk.not_trusted { "WITH NOCHECK" } else { "WITH CHECK" };
    ctx.line(&format!(
        "ALTER TABLE {} {check} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({}) ON DELETE {} ON UPDATE {};",
        table.qualified(),
        ident(&fk.name),
        fk.columns.join(", "),
        fk.referenced,
        fk.referenced_columns.join(", "),
        fk.on_delete,
        fk.on_update
    ));
    if fk.disabled {
        ctx.line(&format!("ALTER TABLE {} NOCHECK CONSTRAINT {};", table.qualified(), ident(&fk.name)));
    }
}

/// A module must be alone in its batch, with the settings it was created with.
fn write_module(ctx: &mut Ctx, module: &Module) {
    let Some(definition) = &module.definition else { return };
    ctx.line(&format!(
        "SET ANSI_NULLS {};\nSET QUOTED_IDENTIFIER {};\nGO",
        if module.ansi_nulls { "ON" } else { "OFF" },
        if module.quoted_identifier { "ON" } else { "OFF" }
    ));
    // Verbatim (the batch is the definition, exactly as stored), on lines of its own.
    ctx.push(definition);
    if !definition.ends_with('\n') {
        ctx.line("");
    }
    ctx.line("GO");
    if !(module.ansi_nulls && module.quoted_identifier) {
        ctx.line("SET ANSI_NULLS ON;\nSET QUOTED_IDENTIFIER ON;\nGO");
    }
}

async fn insert_rows(client: &mut Client, ctx: &mut Ctx, catalog: &Catalog, table: &Table) -> Result<()> {
    let columns: Vec<&Column> = catalog.columns.get(&table.id).into_iter().flatten().filter(|c| c.insertable).collect();
    if columns.is_empty() {
        return Ok(());
    }
    let list = columns.iter().map(|c| ident(&c.name)).collect::<Vec<_>>().join(", ");
    let identity = columns.iter().any(|c| c.identity);
    let prefix = format!("INSERT INTO {} ({list}) VALUES\n", table.qualified());

    let select: Vec<String> = columns.iter().map(|c| if c.variant { variant_expression(&c.name) } else { ident(&c.name) }).collect();
    let sql = format!("select {} from {}", select.join(", "), table.qualified());
    let mut stream = client.simple_query(&sql).await.map_err(|e| query_error(&e, 1))?;
    let mut statement = String::new();
    let (mut pending, mut statements, mut any) = (0usize, 0usize, false);
    while let Some(item) = stream.try_next().await.map_err(|e| query_error(&e, 1))? {
        let QueryItem::Row(row) = item else { continue };
        if !any {
            any = true;
            if identity {
                ctx.line(&format!("SET IDENTITY_INSERT {} ON;", table.qualified()));
            }
        }
        statement.push_str(if pending == 0 { &prefix } else { ",\n" });
        statement.push('(');
        for (i, ((_, data), column)) in row.cells().zip(&columns).enumerate() {
            if i > 0 {
                statement.push_str(", ");
            }
            match data {
                ColumnData::String(Some(text)) if column.variant => write_variant(&mut statement, text),
                data => write_value(&mut statement, data),
            }
        }
        statement.push(')');
        pending += 1;
        if pending == ROWS_PER_INSERT {
            statement.push_str(";\n");
            statements += 1;
            if statements % INSERTS_PER_BATCH == 0 {
                statement.push_str("GO\n");
            }
            ctx.push(&statement);
            statement.clear();
            ctx.rows(pending as u64);
            pending = 0;
            ctx.tick().await?;
        }
    }
    if pending > 0 {
        statement.push_str(";\n");
        ctx.push(&statement);
        ctx.rows(pending as u64);
    }
    if any && identity {
        ctx.line(&format!("SET IDENTITY_INSERT {} OFF;", table.qualified()));
    }
    // The next identity value continues where the source's did (even past deleted rows).
    if let Some(last) = columns.iter().find_map(|c| c.last_identity.as_ref()) {
        ctx.line(&format!("DBCC CHECKIDENT ({}, RESEED, {last}) WITH NO_INFOMSGS;", literal(&table.qualified())));
    }
    if any || columns.iter().any(|c| c.last_identity.is_some()) {
        ctx.line("GO");
    }
    Ok(())
}

/// Reads a `sql_variant` as `basetype|precision|scale|maxlength|value`, the value as exact text
/// (floats with 17 digits, binary as `0x…`, dates in ISO 8601).
fn variant_expression(column: &str) -> String {
    let c = ident(column);
    let property = |name: &str| format!("cast(sql_variant_property({c}, '{name}') as nvarchar(128))");
    format!(
        "case when {c} is null then null else concat({}, N'|', {}, N'|', {}, N'|', {}, N'|', case {}
           when N'binary' then convert(nvarchar(max), cast({c} as varbinary(8000)), 1)
           when N'varbinary' then convert(nvarchar(max), cast({c} as varbinary(8000)), 1)
           when N'float' then convert(nvarchar(max), cast({c} as float), 3)
           when N'real' then convert(nvarchar(max), cast({c} as float), 3)
           when N'money' then convert(nvarchar(max), cast({c} as money), 2)
           when N'smallmoney' then convert(nvarchar(max), cast({c} as money), 2)
           when N'datetime' then convert(nvarchar(max), cast({c} as datetime), 126)
           when N'smalldatetime' then convert(nvarchar(max), cast({c} as smalldatetime), 126)
           when N'datetime2' then convert(nvarchar(max), cast({c} as datetime2), 126)
           when N'datetimeoffset' then convert(nvarchar(max), cast({c} as datetimeoffset), 126)
           else convert(nvarchar(max), {c}) end) end as {c}",
        property("BaseType"),
        property("Precision"),
        property("Scale"),
        property("MaxLength"),
        property("BaseType"),
    )
}

/// `CAST(N'42' AS int)` from [`variant_expression`]'s text.
fn write_variant(out: &mut String, text: &str) {
    let mut parts = text.splitn(5, '|');
    let (Some(base), Some(precision), Some(scale), Some(length), Some(value)) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return out.push_str("NULL");
    };
    let number = |s: &str| s.parse::<i64>().unwrap_or(0);
    let ty = column_type(base, number(length), number(precision), number(scale));
    let value = if matches!(base, "binary" | "varbinary") && value.starts_with("0x") { value.to_string() } else { literal(value) };
    out.push_str(&format!("CAST({value} AS {ty})"));
}

/// A TDS value as a T-SQL literal: numbers bare, strings `N'…'`, binary `0x…`, dates quoted
/// (read as y-m-d thanks to `SET DATEFORMAT ymd`).
fn write_value(out: &mut String, data: &ColumnData<'_>) {
    use std::fmt::Write;
    let _ = match data {
        ColumnData::U8(Some(v)) => write!(out, "{v}"),
        ColumnData::I16(Some(v)) => write!(out, "{v}"),
        ColumnData::I32(Some(v)) => write!(out, "{v}"),
        ColumnData::I64(Some(v)) => write!(out, "{v}"),
        // `real` via its shortest text, so 0.1 stays 0.1.
        ColumnData::F32(Some(v)) => write!(out, "{v:?}"),
        ColumnData::F64(Some(v)) => write!(out, "{v:?}"),
        ColumnData::Bit(Some(v)) => write!(out, "{}", u8::from(*v)),
        ColumnData::String(Some(v)) => write!(out, "{}", literal(v)),
        ColumnData::Xml(Some(v)) => write!(out, "{}", literal(&v.to_string())),
        ColumnData::Guid(Some(v)) => write!(out, "'{}'", v.hyphenated().to_string().to_uppercase()),
        ColumnData::Numeric(Some(v)) => write!(out, "{}", numeric(*v)),
        ColumnData::Binary(Some(v)) => {
            out.push_str("0x");
            for b in v.iter() {
                let _ = write!(out, "{b:02X}");
            }
            Ok(())
        }
        other => match value(other) {
            Value::Null => write!(out, "NULL"),
            v => write!(out, "'{}'", v.display().replace('\'', "''")),
        },
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    #[test]
    fn writes_literals() {
        let mut out = String::new();
        for data in [
            ColumnData::I32(Some(-5)),
            ColumnData::I32(None),
            ColumnData::F64(Some(0.1)),
            ColumnData::F32(Some(0.1)),
            ColumnData::Bit(Some(true)),
            ColumnData::String(Some(Cow::Borrowed("it's ü"))),
            ColumnData::Binary(Some(Cow::Borrowed(&[0, 0xab][..]))),
            ColumnData::Binary(Some(Cow::Borrowed(&[][..]))),
        ] {
            write_value(&mut out, &data);
            out.push('|');
        }
        assert_eq!(out, "-5|NULL|0.1|0.1|1|N'it''s ü'|0x00AB|0x|");
    }

    #[test]
    fn writes_variants() {
        let mut out = String::new();
        for text in ["int|10|0|4|42", "nvarchar|0|0|20|it's", "varbinary|0|0|8000|0x00FF", "decimal|10|2|9|1.50", "junk"] {
            write_variant(&mut out, text);
            out.push('|');
        }
        assert_eq!(out, "CAST(N'42' AS int)|CAST(N'it''s' AS nvarchar(10))|CAST(0x00FF AS varbinary(8000))|CAST(N'1.50' AS decimal(10,2))|NULL|");
    }
}

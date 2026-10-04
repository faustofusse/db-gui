//! Postgres dumps, close to `pg_dump --no-owner --no-privileges` in plain format.
//!
//! Everything is read on a dedicated connection inside one `REPEATABLE READ` transaction, with an
//! empty `search_path` so the catalog functions (`format_type`, `pg_get_*def`…) qualify every name.
//!
//! Order: schemas, extensions, types, sequences, functions, tables (columns, defaults, checks,
//! partitions), functions that need tables, rows (`COPY` or `INSERT`), sequence values, then
//! primary/unique keys, indexes, foreign keys, views, triggers, row security and comments.
//! Owners, grants, extension members, foreign tables and other rarer objects are left out.

use std::collections::{HashMap, HashSet};

use futures_util::{pin_mut, TryStreamExt};
use tokio_postgres::{Client, SimpleQueryMessage};

use super::literal::sql_string;
use super::order::topo_sort;
use super::{Ctx, DataStyle, DumpPhase, DumpScope};
use crate::dialect::Dialect;
use crate::driver::{Error, Result};
use crate::model::{ConnectionConfig, DatabaseKind};
use crate::postgres::query_error;

const PG: Dialect = Dialect(DatabaseKind::Postgres);
/// Rows per `INSERT` in insert mode.
const ROWS_PER_INSERT: usize = 100;

fn ident(name: &str) -> String {
    PG.quote_ident(name)
}

fn qualified(schema: &str, name: &str) -> String {
    PG.quote_relation(schema, name)
}

fn literal(text: &str) -> String {
    let mut s = String::new();
    sql_string(&mut s, text);
    s
}

fn err(e: tokio_postgres::Error) -> Error {
    query_error(&e, None)
}

/// Not a system schema (`pg_catalog`, `pg_toast`, `information_schema`…).
const USER_SCHEMA: &str = "n.nspname <> 'information_schema' and n.nspname !~ '^pg_'";

/// `alias.oid` isn't owned by an extension (those objects come back with `CREATE EXTENSION`).
fn not_extension_member(catalog: &str, alias: &str) -> String {
    format!(
        "not exists (select 1 from pg_depend e where e.classid = '{catalog}'::regclass and e.objid = {alias}.oid and e.deptype = 'e')"
    )
}

// MARK: Catalog

struct Relation {
    oid: u32,
    schema: String,
    name: String,
    /// `r` table, `p` partitioned table, `v` view, `m` materialized view, `f` foreign table.
    kind: String,
    unlogged: bool,
    estimate: Option<u64>,
    partition_bound: Option<String>,
    partition_key: Option<String>,
    row_security: bool,
    force_row_security: bool,
    comment: Option<String>,
    options: Option<String>,
    view_definition: Option<String>,
}

impl Relation {
    fn qualified(&self) -> String {
        qualified(&self.schema, &self.name)
    }

    fn label(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    fn is_table(&self) -> bool {
        matches!(self.kind.as_str(), "r" | "p")
    }

    fn is_view(&self) -> bool {
        matches!(self.kind.as_str(), "v" | "m")
    }

    /// `ALTER TABLE ONLY x`, except on partitioned tables, where keys and indexes must cascade
    /// to the partitions.
    fn alter(&self) -> String {
        if self.kind == "p" {
            format!("ALTER TABLE {}", self.qualified())
        } else {
            format!("ALTER TABLE ONLY {}", self.qualified())
        }
    }
}

struct Column {
    name: String,
    type_name: String,
    not_null: bool,
    default: Option<String>,
    /// `a` always, `d` by default, empty otherwise.
    identity: String,
    /// `s` stored, `v` virtual, empty otherwise.
    generated: String,
    is_local: bool,
    collation: Option<String>,
    comment: Option<String>,
    type_oid: u32,
    element_type_oid: u32,
}

struct Constraint {
    relation: u32,
    name: String,
    kind: String,
    definition: String,
    is_local: bool,
    /// Cloned from a partitioned parent's constraint: recreated with the parent's.
    inherited_from_parent: bool,
    /// For foreign keys: the referenced table.
    references: u32,
}

struct Index {
    relation: u32,
    definition: String,
    backs_constraint: bool,
    /// Attached to a partitioned parent's index: recreated with the parent's.
    attached: bool,
}

struct Trigger {
    relation: u32,
    name: String,
    definition: String,
    disabled: bool,
}

struct Sequence {
    schema: String,
    name: String,
    definition: String,
    /// Owning table and column (`serial` columns: `a`; identity columns: `i`).
    owner: Option<(u32, String, String)>,
}

struct Type {
    kind: String,
    name: String,
    definition: String,
    comment: Option<String>,
}

struct Function {
    signature: String,
    is_procedure: bool,
    definition: String,
    /// Uses a table's row type or reads tables: created after the tables.
    late: bool,
    comment: Option<String>,
}

struct Policy {
    sql: String,
}

pub(super) async fn dump(config: &ConnectionConfig, ctx: &mut Ctx) -> Result<()> {
    let client = crate::postgres::connect(config).await?;
    let version_row = client
        .query_one("select current_setting('server_version_num')::int4, current_setting('server_version')", &[])
        .await
        .map_err(err)?;
    let version: i32 = version_row.get(0);
    let server: String = version_row.get(1);
    if version < 110000 {
        return Err(Error::Unsupported("dumps need PostgreSQL 11 or later".into()));
    }
    client
        .batch_execute(
            "begin isolation level repeatable read, read only;
             set local statement_timeout = 0;
             set local lock_timeout = 0;
             set local idle_in_transaction_session_timeout = 0;
             set local extra_float_digits = 3;
             set local search_path = '';
             set local datestyle = 'ISO';
             set local intervalstyle = 'postgres';
             set local bytea_output = 'hex'",
        )
        .await
        .map_err(err)?;

    super::header(ctx, config, &server);
    ctx.line("SET statement_timeout = 0;");
    ctx.line("SET lock_timeout = 0;");
    ctx.line("SET idle_in_transaction_session_timeout = 0;");
    ctx.line("SET client_encoding = 'UTF8';");
    ctx.line("SET standard_conforming_strings = on;");
    ctx.line("SELECT pg_catalog.set_config('search_path', '', false);");
    ctx.line("SET check_function_bodies = false;");
    ctx.line("SET xmloption = content;");
    ctx.line("SET client_min_messages = warning;");
    ctx.line("SET row_security = off;");
    ctx.line("");

    ctx.phase(DumpPhase::Schema);
    let catalog = Catalog::load(&client, version, &ctx.options.scope).await?;
    for warning in &catalog.warnings {
        ctx.warn(warning.clone());
    }
    let content = ctx.options.content;

    if content.schema() {
        if ctx.options.drop_objects {
            write_drops(ctx, &catalog);
        }
        write_pre_data(ctx, &catalog);
    }

    if content.data() {
        ctx.phase(DumpPhase::Data);
        let tables = catalog.data_tables();
        ctx.set_tables_total(tables.len() as u32);
        for relation in tables {
            let columns: Vec<&Column> = catalog.columns(relation.oid).iter().filter(|c| c.generated.is_empty()).collect();
            if columns.is_empty() {
                ctx.end_table();
                continue;
            }
            ctx.begin_table(relation.label(), relation.estimate);
            match ctx.options.data_style {
                DataStyle::Copy => copy_rows(&client, ctx, relation, &columns).await?,
                DataStyle::Insert => insert_rows(&client, ctx, relation, &columns).await?,
            }
            ctx.end_table();
        }
        write_sequence_values(&client, ctx, &catalog).await?;
    }

    if content.schema() {
        ctx.phase(DumpPhase::PostData);
        write_post_data(ctx, &catalog);
    }
    client.batch_execute("commit").await.map_err(err)?;
    Ok(())
}

struct Catalog {
    schemas: Vec<(String, Option<String>)>,
    extensions: Vec<(String, String)>,
    types: Vec<Type>,
    sequences: Vec<Sequence>,
    functions: Vec<Function>,
    /// Tables, views… in scope, parents before partitions/children.
    relations: Vec<Relation>,
    columns: HashMap<u32, Vec<Column>>,
    parents: HashMap<u32, Vec<String>>,
    constraints: Vec<Constraint>,
    indexes: Vec<Index>,
    triggers: Vec<Trigger>,
    policies: Vec<Policy>,
    view_deps: Vec<(u32, u32)>,
    warnings: Vec<String>,
}

impl Catalog {
    async fn load(client: &Client, version: i32, scope: &DumpScope) -> Result<Self> {
        let mut warnings = Vec::new();

        // Relations, then narrowed to the scope (partitions follow their parent).
        let rows = client
            .query(
                &format!(
                    "select c.oid, n.nspname, c.relname, c.relkind::text, c.relpersistence = 'u', c.reltuples::float8,
                            case when c.relispartition then pg_get_expr(c.relpartbound, c.oid) end,
                            case when c.relkind = 'p' then pg_get_partkeydef(c.oid) end,
                            c.relrowsecurity, c.relforcerowsecurity, obj_description(c.oid, 'pg_class'),
                            array_to_string(c.reloptions, ', '),
                            case when c.relkind in ('v', 'm') then pg_get_viewdef(c.oid) end
                     from pg_class c join pg_namespace n on n.oid = c.relnamespace
                     where c.relkind in ('r', 'p', 'v', 'm', 'f') and {USER_SCHEMA} and {}
                     order by n.nspname, c.relname",
                    not_extension_member("pg_class", "c")
                ),
                &[],
            )
            .await
            .map_err(err)?;
        let all: Vec<Relation> = rows
            .iter()
            .map(|r| {
                let reltuples: f64 = r.get(5);
                Relation {
                    oid: r.get(0),
                    schema: r.get(1),
                    name: r.get(2),
                    kind: r.get(3),
                    unlogged: r.get(4),
                    estimate: (reltuples >= 0.0).then_some(reltuples as u64),
                    partition_bound: r.get(6),
                    partition_key: r.get(7),
                    row_security: r.get(8),
                    force_row_security: r.get(9),
                    comment: r.get(10),
                    options: r.get::<_, Option<String>>(11).filter(|o| !o.is_empty()),
                    view_definition: r.get(12),
                }
            })
            .collect();

        let inherits: Vec<(u32, u32, String)> = client
            .query(
                "select i.inhrelid, i.inhparent, i.inhparent::regclass::text
                 from pg_inherits i join pg_class c on c.oid = i.inhrelid
                 where c.relkind in ('r', 'p', 'f')
                 order by i.inhrelid, i.inhseqno",
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect();

        let mut included: HashSet<u32> =
            all.iter().filter(|r| scope.includes_table(&r.schema, &r.name)).map(|r| r.oid).collect();
        // Partitions of an included partitioned table hold its rows: include them too.
        loop {
            let before = included.len();
            for (child, parent, _) in &inherits {
                let partition = all.iter().any(|r| r.oid == *child && r.partition_bound.is_some());
                if partition && included.contains(parent) {
                    included.insert(*child);
                }
            }
            if included.len() == before {
                break;
            }
        }
        let mut relations: Vec<Relation> = Vec::new();
        for relation in all.into_iter().filter(|r| included.contains(&r.oid)) {
            if relation.kind == "f" {
                warnings.push(format!("Skipped foreign table {}", relation.label()));
                continue;
            }
            relations.push(relation);
        }
        // Parents (inheritance or partitioning) before their children.
        let oids: Vec<u32> = relations.iter().map(|r| r.oid).collect();
        let edges: Vec<(u32, u32)> = inherits.iter().map(|(c, p, _)| (*c, *p)).collect();
        let order = topo_sort(&oids, &edges);
        let position: HashMap<u32, usize> = order.iter().enumerate().map(|(i, oid)| (*oid, i)).collect();
        relations.sort_by_key(|r| position[&r.oid]);
        let relation_oids: HashSet<u32> = relations.iter().map(|r| r.oid).collect();

        let mut parents: HashMap<u32, Vec<String>> = HashMap::new();
        for (child, _, parent_name) in &inherits {
            parents.entry(*child).or_default().push(parent_name.clone());
        }

        // Columns of relations and composite types.
        let generated = if version >= 120000 { "a.attgenerated::text" } else { "''::text" };
        let mut columns: HashMap<u32, Vec<Column>> = HashMap::new();
        for r in client
            .query(
                &format!(
                    "select a.attrelid, a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull,
                            pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, {generated}, a.attislocal,
                            case when a.attcollation <> 0 and a.attcollation <> t.typcollation
                                 then quote_ident(cn.nspname) || '.' || quote_ident(co.collname) end,
                            col_description(a.attrelid, a.attnum), a.atttypid, t.typelem
                     from pg_attribute a
                     join pg_class c on c.oid = a.attrelid
                     join pg_namespace n on n.oid = c.relnamespace
                     join pg_type t on t.oid = a.atttypid
                     left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
                     left join pg_collation co on co.oid = a.attcollation
                     left join pg_namespace cn on cn.oid = co.collnamespace
                     where c.relkind in ('r', 'p', 'v', 'm', 'c') and {USER_SCHEMA}
                       and a.attnum > 0 and not a.attisdropped
                     order by a.attrelid, a.attnum"
                ),
                &[],
            )
            .await
            .map_err(err)?
        {
            columns.entry(r.get(0)).or_default().push(Column {
                name: r.get(1),
                type_name: r.get(2),
                not_null: r.get(3),
                default: r.get(4),
                identity: r.get(5),
                generated: r.get(6),
                is_local: r.get(7),
                collation: r.get(8),
                comment: r.get(9),
                type_oid: r.get(10),
                element_type_oid: r.get(11),
            });
        }

        let parent_constraint = "con.conparentid <> 0";
        let constraints: Vec<Constraint> = client
            .query(
                &format!(
                    "select con.conrelid, con.conname, con.contype::text, pg_get_constraintdef(con.oid),
                            con.conislocal, {parent_constraint}, con.confrelid
                     from pg_constraint con
                     where con.conrelid <> 0 and con.contype in ('c', 'p', 'u', 'f', 'x')
                     order by con.conrelid, con.conname"
                ),
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .map(|r| Constraint {
                relation: r.get(0),
                name: r.get(1),
                kind: r.get(2),
                definition: r.get(3),
                is_local: r.get(4),
                inherited_from_parent: r.get(5),
                references: r.get(6),
            })
            .filter(|c| relation_oids.contains(&c.relation))
            .collect();

        let indexes: Vec<Index> = client
            .query(
                "select i.indrelid, pg_get_indexdef(i.indexrelid),
                        exists(select 1 from pg_constraint c
                               where c.conindid = i.indexrelid and c.conrelid = i.indrelid and c.contype in ('p', 'u', 'x')),
                        exists(select 1 from pg_inherits h where h.inhrelid = i.indexrelid)
                 from pg_index i join pg_class ic on ic.oid = i.indexrelid
                 order by i.indrelid, ic.relname",
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .map(|r| Index { relation: r.get(0), definition: r.get(1), backs_constraint: r.get(2), attached: r.get(3) })
            .filter(|i| relation_oids.contains(&i.relation))
            .collect();

        let clone = if version >= 130000 { "and t.tgparentid = 0" } else { "" };
        let triggers: Vec<Trigger> = client
            .query(
                &format!(
                    "select t.tgrelid, t.tgname, pg_get_triggerdef(t.oid), t.tgenabled = 'D'
                     from pg_trigger t where not t.tgisinternal {clone}
                     order by t.tgrelid, t.tgname"
                ),
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .map(|r| Trigger { relation: r.get(0), name: r.get(1), definition: r.get(2), disabled: r.get(3) })
            .filter(|t| relation_oids.contains(&t.relation))
            .collect();

        let policies: Vec<Policy> = client
            .query(
                "select pol.polrelid, pol.polname, pol.polpermissive, pol.polcmd::text,
                        case when pol.polroles = '{0}' then null
                             else (select string_agg(quote_ident(rolname), ', ' order by rolname) from pg_roles where oid = any(pol.polroles)) end,
                        pg_get_expr(pol.polqual, pol.polrelid), pg_get_expr(pol.polwithcheck, pol.polrelid)
                 from pg_policy pol order by pol.polrelid, pol.polname",
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .filter(|r| relation_oids.contains(&r.get::<_, u32>(0)))
            .map(|r| {
                let relation: u32 = r.get(0);
                let table = relations.iter().find(|t| t.oid == relation).map(Relation::qualified).unwrap_or_default();
                let mut sql = format!("CREATE POLICY {} ON {table}", ident(&r.get::<_, String>(1)));
                if !r.get::<_, bool>(2) {
                    sql.push_str(" AS RESTRICTIVE");
                }
                let command = match r.get::<_, String>(3).as_str() {
                    "r" => "SELECT",
                    "a" => "INSERT",
                    "w" => "UPDATE",
                    "d" => "DELETE",
                    _ => "ALL",
                };
                sql.push_str(&format!(" FOR {command}"));
                if let Some(roles) = r.get::<_, Option<String>>(4) {
                    sql.push_str(&format!(" TO {roles}"));
                }
                if let Some(using) = r.get::<_, Option<String>>(5) {
                    sql.push_str(&format!(" USING ({using})"));
                }
                if let Some(check) = r.get::<_, Option<String>>(6) {
                    sql.push_str(&format!(" WITH CHECK ({check})"));
                }
                sql.push(';');
                Policy { sql }
            })
            .collect();

        let view_deps: Vec<(u32, u32)> = client
            .query(
                "select distinct r.ev_class, d.refobjid
                 from pg_rewrite r
                 join pg_depend d on d.classid = 'pg_rewrite'::regclass and d.objid = r.oid
                                 and d.refclassid = 'pg_class'::regclass and d.refobjid <> r.ev_class",
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();

        // Sequences: identity ones come with their column; the others are created up front.
        let sequences: Vec<Sequence> = client
            .query(
                &format!(
                    "select c.oid, n.nspname, c.relname, format_type(s.seqtypid, null), s.seqstart, s.seqincrement,
                            s.seqmin, s.seqmax, s.seqcache, s.seqcycle, dep.refobjid, dep.deptype::text, a.attname
                     from pg_sequence s
                     join pg_class c on c.oid = s.seqrelid
                     join pg_namespace n on n.oid = c.relnamespace
                     left join pg_depend dep on dep.classid = 'pg_class'::regclass and dep.objid = c.oid
                                            and dep.refclassid = 'pg_class'::regclass and dep.deptype in ('a', 'i')
                     left join pg_attribute a on a.attrelid = dep.refobjid and a.attnum = dep.refobjsubid
                     where {USER_SCHEMA} and {}
                     order by n.nspname, c.relname",
                    not_extension_member("pg_class", "c")
                ),
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .filter_map(|r| {
                let schema: String = r.get(1);
                let owner_oid: Option<u32> = r.get(10);
                let deptype: Option<String> = r.get(11);
                let owner = match owner_oid {
                    Some(oid) => {
                        let table = relations.iter().find(|t| t.oid == oid)?;
                        Some((oid, table.qualified(), r.get::<_, Option<String>>(12).unwrap_or_default()))
                    }
                    None if scope.whole_schemas() && scope.includes_schema(&schema) => None,
                    None => return None,
                };
                let name: String = r.get(2);
                let identity = deptype.as_deref() == Some("i");
                let definition = if identity {
                    String::new()
                } else {
                    format!(
                        "CREATE SEQUENCE {}\n    AS {}\n    START WITH {}\n    INCREMENT BY {}\n    MINVALUE {}\n    MAXVALUE {}\n    CACHE {}{};",
                        qualified(&schema, &name),
                        r.get::<_, String>(3),
                        r.get::<_, i64>(4),
                        r.get::<_, i64>(5),
                        r.get::<_, i64>(6),
                        r.get::<_, i64>(7),
                        r.get::<_, i64>(8),
                        if r.get::<_, bool>(9) { "\n    CYCLE" } else { "" },
                    )
                };
                Some(Sequence { schema, name, definition, owner })
            })
            .collect();

        // Types: whole schemas get all of theirs; a table selection gets the ones its columns use.
        let used_types: HashSet<u32> = relations
            .iter()
            .flat_map(|r| columns.get(&r.oid).into_iter().flatten())
            .flat_map(|c| [c.type_oid, c.element_type_oid])
            .collect();
        let domain_checks: HashMap<u32, Vec<String>> = client
            .query(
                "select contypid, 'CONSTRAINT ' || quote_ident(conname) || ' ' || pg_get_constraintdef(oid)
                 from pg_constraint where contypid <> 0 and contype = 'c' order by contypid, conname",
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .fold(HashMap::new(), |mut map, r| {
                map.entry(r.get(0)).or_insert_with(Vec::new).push(r.get(1));
                map
            });
        let mut types: Vec<Type> = Vec::new();
        for r in client
            .query(
                &format!(
                    "select t.oid, n.nspname, t.typname, t.typtype::text,
                            case when t.typtype = 'd' then format_type(t.typbasetype, t.typtypmod) end,
                            t.typnotnull, t.typdefault,
                            array(select quote_literal(enumlabel) from pg_enum where enumtypid = t.oid order by enumsortorder),
                            (select format_type(rngsubtype, null) from pg_range where rngtypid = t.oid),
                            obj_description(t.oid, 'pg_type'), t.typrelid
                     from pg_type t join pg_namespace n on n.oid = t.typnamespace
                     where t.typtype in ('e', 'd', 'c', 'r') and {USER_SCHEMA} and {}
                       and (t.typtype <> 'c' or (select relkind from pg_class where oid = t.typrelid) = 'c')
                     order by t.oid",
                    not_extension_member("pg_type", "t")
                ),
                &[],
            )
            .await
            .map_err(err)?
        {
            let oid: u32 = r.get(0);
            let schema: String = r.get(1);
            let wanted = if scope.whole_schemas() { scope.includes_schema(&schema) } else { used_types.contains(&oid) };
            if !wanted {
                continue;
            }
            let name = qualified(&schema, &r.get::<_, String>(2));
            let kind: String = r.get(3);
            let definition = match kind.as_str() {
                "e" => format!("CREATE TYPE {name} AS ENUM (\n    {}\n);", r.get::<_, Vec<String>>(7).join(",\n    ")),
                "r" => format!("CREATE TYPE {name} AS RANGE (\n    subtype = {}\n);", r.get::<_, Option<String>>(8).unwrap_or_default()),
                "d" => {
                    let mut sql = format!("CREATE DOMAIN {name} AS {}", r.get::<_, Option<String>>(4).unwrap_or_default());
                    if let Some(default) = r.get::<_, Option<String>>(6) {
                        sql.push_str(&format!("\n    DEFAULT {default}"));
                    }
                    if r.get::<_, bool>(5) {
                        sql.push_str("\n    NOT NULL");
                    }
                    for check in domain_checks.get(&oid).into_iter().flatten() {
                        sql.push_str(&format!("\n    {check}"));
                    }
                    sql.push(';');
                    sql
                }
                _ => {
                    let attributes: Vec<String> = columns
                        .get(&r.get::<_, u32>(10))
                        .into_iter()
                        .flatten()
                        .map(|c| format!("    {} {}", ident(&c.name), c.type_name))
                        .collect();
                    format!("CREATE TYPE {name} AS (\n{}\n);", attributes.join(",\n"))
                }
            };
            types.push(Type { kind, name, definition, comment: r.get(9) });
        }
        // Enums and ranges first, then domains (over them), then composite types (over any).
        types.sort_by_key(|t| match t.kind.as_str() {
            "e" => 0,
            "r" => 1,
            "d" => 2,
            _ => 3,
        });

        let mut functions = Vec::new();
        if scope.whole_schemas() {
            for r in client
                .query(
                    &format!(
                        "select n.nspname, p.prokind::text, p.oid::regprocedure::text,
                                case when p.prokind <> 'a' then pg_get_functiondef(p.oid) end,
                                exists (select 1 from pg_depend d
                                        left join pg_type t on d.refclassid = 'pg_type'::regclass and t.oid = d.refobjid
                                        left join pg_class r on r.oid = t.typrelid
                                        where d.classid = 'pg_proc'::regclass and d.objid = p.oid
                                          and (d.refclassid = 'pg_class'::regclass or r.relkind in ('r', 'p', 'v', 'm', 'f'))),
                                obj_description(p.oid, 'pg_proc')
                         from pg_proc p join pg_namespace n on n.oid = p.pronamespace
                         where {USER_SCHEMA} and {}
                         order by n.nspname, p.proname, p.oid",
                        not_extension_member("pg_proc", "p")
                    ),
                    &[],
                )
                .await
                .map_err(err)?
            {
                let schema: String = r.get(0);
                if !scope.includes_schema(&schema) {
                    continue;
                }
                let signature: String = r.get(2);
                let Some(definition) = r.get::<_, Option<String>>(3) else {
                    warnings.push(format!("Skipped aggregate {signature}"));
                    continue;
                };
                functions.push(Function {
                    is_procedure: r.get::<_, String>(1) == "p",
                    signature,
                    definition,
                    late: r.get(4),
                    comment: r.get(5),
                });
            }
        }

        let schema_rows = client
            .query(
                &format!(
                    "select n.nspname, obj_description(n.oid, 'pg_namespace') from pg_namespace n
                     where {USER_SCHEMA} and {} order by n.nspname",
                    not_extension_member("pg_namespace", "n")
                ),
                &[],
            )
            .await
            .map_err(err)?;
        let schemas: Vec<(String, Option<String>)> = schema_rows
            .iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, Option<String>>(1)))
            .filter(|(name, _)| {
                if scope.whole_schemas() {
                    scope.includes_schema(name)
                } else {
                    relations.iter().any(|r| &r.schema == name)
                }
            })
            .collect();

        let extensions = client
            .query(
                "select e.extname, n.nspname from pg_extension e join pg_namespace n on n.oid = e.extnamespace
                 where e.extname <> 'plpgsql' order by e.extname",
                &[],
            )
            .await
            .map_err(err)?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();

        Ok(Self {
            schemas,
            extensions,
            types,
            sequences,
            functions,
            relations,
            columns,
            parents,
            constraints,
            indexes,
            triggers,
            policies,
            view_deps,
            warnings,
        })
    }

    fn columns(&self, relation: u32) -> &[Column] {
        self.columns.get(&relation).map_or(&[], Vec::as_slice)
    }

    fn relation(&self, oid: u32) -> Option<&Relation> {
        self.relations.iter().find(|r| r.oid == oid)
    }

    /// Plain tables (and leaf partitions) with rows, referenced tables first so a data-only
    /// dump restores without tripping foreign keys.
    fn data_tables(&self) -> Vec<&Relation> {
        let oids: Vec<u32> = self.relations.iter().filter(|r| r.kind == "r").map(|r| r.oid).collect();
        let deps: Vec<(u32, u32)> =
            self.constraints.iter().filter(|c| c.kind == "f").map(|c| (c.relation, c.references)).collect();
        topo_sort(&oids, &deps).into_iter().filter_map(|oid| self.relation(oid)).collect()
    }

    /// Views and materialized views, each after the views it reads.
    fn views(&self) -> Vec<&Relation> {
        let oids: Vec<u32> = self.relations.iter().filter(|r| r.is_view()).map(|r| r.oid).collect();
        topo_sort(&oids, &self.view_deps).into_iter().filter_map(|oid| self.relation(oid)).collect()
    }
}

// MARK: Writing

fn write_drops(ctx: &mut Ctx, catalog: &Catalog) {
    for view in catalog.views().iter().rev() {
        let kind = if view.kind == "m" { "MATERIALIZED VIEW" } else { "VIEW" };
        ctx.line(&format!("DROP {kind} IF EXISTS {} CASCADE;", view.qualified()));
    }
    for table in catalog.relations.iter().rev().filter(|r| r.is_table() && r.partition_bound.is_none()) {
        ctx.line(&format!("DROP TABLE IF EXISTS {} CASCADE;", table.qualified()));
    }
    for sequence in catalog.sequences.iter().filter(|s| !s.definition.is_empty()) {
        ctx.line(&format!("DROP SEQUENCE IF EXISTS {} CASCADE;", qualified(&sequence.schema, &sequence.name)));
    }
    for function in &catalog.functions {
        let kind = if function.is_procedure { "PROCEDURE" } else { "FUNCTION" };
        ctx.line(&format!("DROP {kind} IF EXISTS {} CASCADE;", function.signature));
    }
    for t in catalog.types.iter().rev() {
        let kind = if t.kind == "d" { "DOMAIN" } else { "TYPE" };
        ctx.line(&format!("DROP {kind} IF EXISTS {} CASCADE;", t.name));
    }
    ctx.line("");
}

fn write_pre_data(ctx: &mut Ctx, catalog: &Catalog) {
    for (schema, _) in &catalog.schemas {
        ctx.line(&format!("CREATE SCHEMA IF NOT EXISTS {};", ident(schema)));
    }
    for (extension, schema) in &catalog.extensions {
        ctx.line(&format!("CREATE EXTENSION IF NOT EXISTS {} WITH SCHEMA {};", ident(extension), ident(schema)));
    }
    ctx.line("");
    for t in &catalog.types {
        ctx.line(&t.definition);
        ctx.line("");
    }
    for sequence in catalog.sequences.iter().filter(|s| !s.definition.is_empty()) {
        ctx.line(&sequence.definition);
        ctx.line("");
    }
    for function in catalog.functions.iter().filter(|f| !f.late) {
        write_function(ctx, function);
    }
    for relation in catalog.relations.iter().filter(|r| r.is_table()) {
        write_table(ctx, catalog, relation);
    }
    for sequence in &catalog.sequences {
        if let (false, Some((_, table, column))) = (sequence.definition.is_empty(), &sequence.owner) {
            ctx.line(&format!(
                "ALTER SEQUENCE {} OWNED BY {table}.{};",
                qualified(&sequence.schema, &sequence.name),
                ident(column)
            ));
        }
    }
    ctx.line("");
    for function in catalog.functions.iter().filter(|f| f.late) {
        write_function(ctx, function);
    }
}

fn write_function(ctx: &mut Ctx, function: &Function) {
    let definition = function.definition.trim_end();
    ctx.line(&format!("{definition};"));
    ctx.line("");
}

fn write_table(ctx: &mut Ctx, catalog: &Catalog, table: &Relation) {
    let name = table.qualified();
    let checks: Vec<&Constraint> =
        catalog.constraints.iter().filter(|c| c.relation == table.oid && c.kind == "c" && c.is_local).collect();
    if let Some(bound) = &table.partition_bound {
        let parent = catalog.parents.get(&table.oid).and_then(|p| p.first()).cloned().unwrap_or_default();
        let mut sql = format!("CREATE TABLE {name} PARTITION OF {parent} {bound}");
        if let Some(key) = &table.partition_key {
            sql.push_str(&format!(" PARTITION BY {key}"));
        }
        ctx.line(&format!("{sql};"));
        for check in checks {
            ctx.line(&format!("{} ADD CONSTRAINT {} {};", table.alter(), ident(&check.name), check.definition));
        }
        ctx.line("");
        return;
    }

    let inherits = catalog.parents.get(&table.oid).filter(|p| !p.is_empty());
    let mut lines: Vec<String> = catalog
        .columns(table.oid)
        .iter()
        .filter(|c| c.is_local || inherits.is_none())
        .map(|c| {
            let mut line = format!("    {} {}", ident(&c.name), c.type_name);
            if let Some(collation) = &c.collation {
                line.push_str(&format!(" COLLATE {collation}"));
            }
            match (c.identity.as_str(), c.generated.as_str(), &c.default) {
                ("a", _, _) => line.push_str(" GENERATED ALWAYS AS IDENTITY"),
                ("d", _, _) => line.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
                ("", "s", Some(expression)) => line.push_str(&format!(" GENERATED ALWAYS AS ({expression}) STORED")),
                ("", "v", Some(expression)) => line.push_str(&format!(" GENERATED ALWAYS AS ({expression}) VIRTUAL")),
                (_, _, Some(default)) => line.push_str(&format!(" DEFAULT {default}")),
                _ => {}
            }
            if c.not_null {
                line.push_str(" NOT NULL");
            }
            line
        })
        .collect();
    lines.extend(checks.iter().map(|c| format!("    CONSTRAINT {} {}", ident(&c.name), c.definition)));

    let unlogged = if table.unlogged { "UNLOGGED " } else { "" };
    let mut sql = format!("CREATE {unlogged}TABLE {name} (\n{}\n)", lines.join(",\n"));
    if let Some(parents) = inherits {
        sql.push_str(&format!("\nINHERITS ({})", parents.join(", ")));
    }
    if let Some(key) = &table.partition_key {
        sql.push_str(&format!("\nPARTITION BY {key}"));
    }
    if let Some(options) = &table.options {
        sql.push_str(&format!("\nWITH ({options})"));
    }
    ctx.line(&format!("{sql};"));
    ctx.line("");
}

fn write_post_data(ctx: &mut Ctx, catalog: &Catalog) {
    let tables = || catalog.relations.iter().filter(|r| r.is_table());
    // Keys and unique constraints, indexes, then foreign keys (which need those keys).
    for table in tables() {
        for c in catalog.constraints.iter().filter(|c| c.relation == table.oid && matches!(c.kind.as_str(), "p" | "u" | "x")) {
            if !c.inherited_from_parent {
                ctx.line(&format!("{} ADD CONSTRAINT {} {};", table.alter(), ident(&c.name), c.definition));
            }
        }
    }
    ctx.line("");
    for table in tables() {
        write_indexes(ctx, catalog, table);
    }
    ctx.line("");
    for table in tables() {
        for c in catalog.constraints.iter().filter(|c| c.relation == table.oid && c.kind == "f" && !c.inherited_from_parent) {
            ctx.line(&format!("{} ADD CONSTRAINT {} {};", table.alter(), ident(&c.name), c.definition));
        }
    }
    ctx.line("");

    let views = catalog.views();
    for view in &views {
        let body = view.view_definition.as_deref().unwrap_or_default().trim().trim_end_matches(';');
        let options = view.options.as_ref().map(|o| format!(" WITH ({o})")).unwrap_or_default();
        if view.kind == "m" {
            ctx.line(&format!("CREATE MATERIALIZED VIEW {}{options} AS\n{body}\nWITH NO DATA;", view.qualified()));
            write_indexes(ctx, catalog, view);
        } else {
            ctx.line(&format!("CREATE VIEW {}{options} AS\n{body};", view.qualified()));
        }
        ctx.line("");
    }
    if ctx.options.content.data() {
        for view in views.iter().filter(|v| v.kind == "m") {
            ctx.line(&format!("REFRESH MATERIALIZED VIEW {};", view.qualified()));
        }
    }

    for trigger in &catalog.triggers {
        ctx.line(&format!("{};", trigger.definition));
        if trigger.disabled {
            if let Some(table) = catalog.relation(trigger.relation) {
                ctx.line(&format!("ALTER TABLE {} DISABLE TRIGGER {};", table.qualified(), ident(&trigger.name)));
            }
        }
    }
    ctx.line("");

    for table in &catalog.relations {
        if table.row_security {
            ctx.line(&format!("ALTER TABLE {} ENABLE ROW LEVEL SECURITY;", table.qualified()));
        }
        if table.force_row_security {
            ctx.line(&format!("ALTER TABLE {} FORCE ROW LEVEL SECURITY;", table.qualified()));
        }
    }
    for policy in &catalog.policies {
        ctx.line(&policy.sql);
    }
    ctx.line("");

    write_comments(ctx, catalog);
}

fn write_indexes(ctx: &mut Ctx, catalog: &Catalog, relation: &Relation) {
    for index in catalog.indexes.iter().filter(|i| i.relation == relation.oid && !i.backs_constraint && !i.attached) {
        // A partitioned table's index is shown `ON ONLY`; without it, it's built on every partition too.
        let definition = if relation.kind == "p" { index.definition.replacen(" ON ONLY ", " ON ", 1) } else { index.definition.clone() };
        ctx.line(&format!("{definition};"));
    }
}

fn write_comments(ctx: &mut Ctx, catalog: &Catalog) {
    // `public` has a default comment that only its owner may change.
    for (schema, comment) in &catalog.schemas {
        if let (true, Some(comment)) = (schema != "public", comment) {
            ctx.line(&format!("COMMENT ON SCHEMA {} IS {};", ident(schema), literal(comment)));
        }
    }
    for t in &catalog.types {
        if let Some(comment) = &t.comment {
            let kind = if t.kind == "d" { "DOMAIN" } else { "TYPE" };
            ctx.line(&format!("COMMENT ON {kind} {} IS {};", t.name, literal(comment)));
        }
    }
    for function in &catalog.functions {
        if let Some(comment) = &function.comment {
            let kind = if function.is_procedure { "PROCEDURE" } else { "FUNCTION" };
            ctx.line(&format!("COMMENT ON {kind} {} IS {};", function.signature, literal(comment)));
        }
    }
    for relation in &catalog.relations {
        let kind = match relation.kind.as_str() {
            "v" => "VIEW",
            "m" => "MATERIALIZED VIEW",
            _ => "TABLE",
        };
        if let Some(comment) = &relation.comment {
            ctx.line(&format!("COMMENT ON {kind} {} IS {};", relation.qualified(), literal(comment)));
        }
        for column in catalog.columns(relation.oid) {
            if let Some(comment) = &column.comment {
                ctx.line(&format!(
                    "COMMENT ON COLUMN {}.{} IS {};",
                    relation.qualified(),
                    ident(&column.name),
                    literal(comment)
                ));
            }
        }
    }
}

// MARK: Data

fn column_list(columns: &[&Column]) -> String {
    columns.iter().map(|c| ident(&c.name)).collect::<Vec<_>>().join(", ")
}

async fn copy_rows(client: &Client, ctx: &mut Ctx, table: &Relation, columns: &[&Column]) -> Result<()> {
    let target = format!("{} ({})", table.qualified(), column_list(columns));
    let stream = client.copy_out(&format!("COPY {target} TO STDOUT")).await.map_err(err)?;
    pin_mut!(stream);
    ctx.line(&format!("COPY {target} FROM stdin;"));
    while let Some(chunk) = stream.try_next().await.map_err(err)? {
        // Rows end with a newline; newlines inside values are escaped as `\n`.
        let rows = chunk.iter().filter(|&&b| b == b'\n').count() as u64;
        ctx.out.push(&chunk);
        ctx.rows(rows);
        ctx.tick().await?;
    }
    ctx.line("\\.");
    ctx.line("");
    Ok(())
}

async fn insert_rows(client: &Client, ctx: &mut Ctx, table: &Relation, columns: &[&Column]) -> Result<()> {
    let overriding = if columns.iter().any(|c| c.identity == "a") { " OVERRIDING SYSTEM VALUE" } else { "" };
    let prefix = format!("INSERT INTO {} ({}){overriding} VALUES\n", table.qualified(), column_list(columns));
    let select = format!("SELECT {} FROM ONLY {}", column_list(columns), table.qualified());
    let stream = client.simple_query_raw(&select).await.map_err(err)?;
    pin_mut!(stream);
    let mut statement = String::new();
    let mut pending = 0usize;
    while let Some(message) = stream.try_next().await.map_err(err)? {
        let SimpleQueryMessage::Row(row) = message else { continue };
        statement.push_str(if pending == 0 { &prefix } else { ",\n" });
        statement.push('(');
        for i in 0..row.len() {
            if i > 0 {
                statement.push_str(", ");
            }
            match row.get(i) {
                None => statement.push_str("NULL"),
                // A quoted literal is cast to the column's type, whatever it is.
                Some(text) => sql_string(&mut statement, text),
            }
        }
        statement.push(')');
        pending += 1;
        if pending == ROWS_PER_INSERT {
            statement.push_str(";\n");
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
    ctx.line("");
    Ok(())
}

async fn write_sequence_values(client: &Client, ctx: &mut Ctx, catalog: &Catalog) -> Result<()> {
    for sequence in &catalog.sequences {
        let name = qualified(&sequence.schema, &sequence.name);
        let row = client
            .query_one(&format!("select last_value, is_called from {name}"), &[])
            .await
            .map_err(err)?;
        let (last, called): (i64, bool) = (row.get(0), row.get(1));
        let target = match (&sequence.owner, sequence.definition.is_empty()) {
            // Identity sequences get their name on restore; look it up from the column.
            (Some((_, table, column)), true) => {
                format!("pg_catalog.pg_get_serial_sequence({}, {})", literal(table), literal(column))
            }
            _ => literal(&name),
        };
        ctx.line(&format!("SELECT pg_catalog.setval({target}, {last}, {called});"));
    }
    ctx.line("");
    Ok(())
}

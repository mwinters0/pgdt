//! What the preamble declares, per database: the tables, types, collations
//! and extensions its DDL wrote, kept as the words written — a declared type's
//! text, never a parsed `(base, typmod)` pair, the codec drawing every
//! conclusion from them (`docs/design/decisions.md`, "D74"). The shapes are the
//! cache's (`crate::persisted::catalog`); here are the walks over what they
//! declare. The grammar that reads them is `pgdt_dump::ddl`'s.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CheckConstraint, ColumnDef, DatabaseMetadata, DumpMetadata, NotNull, PartitionOf, TableDef,
    TableReference, TypeKind,
};
use pgdt_core::internal::canonical_type_name;

impl DumpMetadata {
    /// The database named `database` — an exact match on
    /// [`DatabaseMetadata::name`], never a guess; `None` matches the unnamed
    /// database a plain (non-`\connect`) dump produces. A block's span records
    /// its database's name, which is what makes the exact match possible.
    pub fn database(&self, database: Option<&str>) -> Option<&DatabaseMetadata> {
        // deficiency: KD133 — the first segment of a name answers for every
        // segment of it: two concatenated dumps of one database are two
        // entries, so a block of the second resolves against the first's DDL —
        // a column the second retyped typed as the first declared it, a table
        // only the second declares `NotDeclared` — with no error, every reader
        // of a block's DDL reaching this lookup. The fix is metadata keyed by
        // the segment a block belongs to rather than by its name. **(c)
        // unowned**; promoted by a same-name concatenation reaching a user.
        self.databases.iter().find(|db| db.name.as_deref() == database)
    }
}

impl DatabaseMetadata {
    /// The declaration of column `column` of table `table`: the table's own,
    /// else, for a typed table, its `OF` type's field alone, else the first
    /// parent's, in `INHERITS` order, that declares it — each parent asked the same way, so a
    /// grandparent's column is found. `None` where the table is not declared
    /// here or nothing it reaches declares the column.
    ///
    /// Read against the preamble's final state, never folded into the
    /// table's own list (`docs/design/decisions.md`, "D36"). A reference that
    /// names a table or type the preamble does not hold, or a composite whose
    /// field list did not parse, reaches nothing; a cycle, which no server
    /// can hold and a hand-written file can, ends at the table it returns to.
    ///
    /// The server merges a column declared on two of these into one of a
    /// single type and collation, refusing the table otherwise, so which one
    /// is found first decides nothing `pg_dump` can write.
    pub fn declared_column(&self, table: &str, column: &str) -> Option<&ColumnDef> {
        self.declared_column_from(table, column, &mut BTreeSet::new())
    }

    fn declared_column_from<'a>(
        &'a self,
        table: &str,
        column: &str,
        visited: &mut BTreeSet<&'a str>,
    ) -> Option<&'a ColumnDef> {
        let (name, def) = self.tables.get_key_value(table)?;
        if !visited.insert(name) {
            return None;
        }
        if let Some(found) = def.columns.iter().find(|c| c.name == column) {
            return Some(found);
        }
        if let Some(of_type) = &def.of_type {
            return self.composite_fields(of_type)?.iter().find(|f| f.name == column);
        }
        def.parents.iter().find_map(|parent| self.declared_column_from(parent, column, visited))
    }

    /// Every column of `table`, in the order the server gives them — each
    /// parent's, in `INHERITS` order, then the `OF` type's fields, then the
    /// table's own — a column two of them declare appearing once, at its
    /// first place, as the declaration [`declared_column`](Self::declared_column)
    /// returns — bar a table both typed and inheriting, which no server holds,
    /// whose parents that lookup never asks. Empty where the table is not
    /// declared here.
    pub fn declared_columns(&self, table: &str) -> Vec<&ColumnDef> {
        let mut columns = Vec::new();
        self.declared_columns_from(table, &mut BTreeSet::new(), &mut columns);
        columns
    }

    fn declared_columns_from<'a>(
        &'a self,
        table: &str,
        visited: &mut BTreeSet<&'a str>,
        columns: &mut Vec<&'a ColumnDef>,
    ) {
        let Some((name, def)) = self.tables.get_key_value(table) else { return };
        if !visited.insert(name) {
            return;
        }
        for parent in &def.parents {
            let mut inherited = Vec::new();
            self.declared_columns_from(parent, visited, &mut inherited);
            for column in inherited {
                if !columns.iter().any(|c| c.name == column.name) {
                    columns.push(column);
                }
            }
        }
        let typed = def.of_type.as_deref().and_then(|t| self.composite_fields(t));
        for column in typed.into_iter().flatten().chain(&def.columns) {
            match columns.iter_mut().find(|c| c.name == column.name) {
                Some(merged) => *merged = column,
                None => columns.push(column),
            }
        }
    }

    /// Whether table `table` declares column `column` `NOT NULL` — on the
    /// column, at the table, or by an `ALTER TABLE … SET NOT NULL` — or takes
    /// it from a parent, in `INHERITS` order and each parent asked the same
    /// way, which passes on every `NOT NULL` but v18's `NO INHERIT` (I76). A
    /// domain's is its type's, `pgdt_codec`'s `domain_not_null`'s to say.
    ///
    /// Read against the preamble's final state, as
    /// [`declared_column`](Self::declared_column) is; a `NOT NULL` added after
    /// the data (v18's `NOT VALID`, a post-data primary key) is never in it.
    pub fn column_not_null(&self, table: &str, column: &str) -> bool {
        self.not_null_from(table, column, true, &mut BTreeSet::new())
    }

    /// [`column_not_null`](Self::column_not_null) over `table`, which is the
    /// table asked about where `own` and an ancestor of it elsewhere.
    fn not_null_from<'a>(
        &'a self,
        table: &str,
        column: &str,
        own: bool,
        visited: &mut BTreeSet<&'a str>,
    ) -> bool {
        let Some((name, def)) = self.tables.get_key_value(table) else { return false };
        if !visited.insert(name) {
            return false;
        }
        let binds = |not_null: NotNull| own || not_null == NotNull::Inherited;
        def.columns.iter().filter(|c| c.name == column).filter_map(|c| c.not_null).any(binds)
            || def.not_null.iter().any(|(c, not_null)| c == column && binds(*not_null))
            || def.parents.iter().any(|parent| self.not_null_from(parent, column, false, visited))
    }

    /// The `CHECK` constraints a restore evaluates against each row of table
    /// `table`, in declaration order: its own, then each parent's it takes,
    /// in `INHERITS` order and each parent asked the same way, a parent
    /// passing on every one but a `NO INHERIT` one (I77). Each is paired with
    /// the ancestor declaring it, `None` for the table's own. A name met
    /// twice is one constraint, the server merging an inherited `CHECK` into
    /// a child's own of that name, as `--binary-upgrade`'s child re-declares
    /// it; an unnamed one is never merged.
    ///
    /// Read against the preamble's final state, as
    /// [`declared_column`](Self::declared_column) is; a `CHECK` added after
    /// the data is never in it.
    pub fn table_checks(&self, table: &str) -> Vec<(&CheckConstraint, Option<&str>)> {
        let mut checks = Vec::new();
        self.checks_from(table, true, &mut BTreeSet::new(), &mut checks);
        checks
    }

    /// [`table_checks`](Self::table_checks) over `table`, which is the table
    /// asked about where `own` and an ancestor of it elsewhere.
    fn checks_from<'a>(
        &'a self,
        table: &str,
        own: bool,
        visited: &mut BTreeSet<&'a str>,
        checks: &mut Vec<(&'a CheckConstraint, Option<&'a str>)>,
    ) {
        let Some((name, def)) = self.tables.get_key_value(table) else { return };
        if !visited.insert(name) {
            return;
        }
        let declared_by = (!own).then_some(name.as_str());
        for check in def.checks.iter().filter(|c| own || !c.no_inherit) {
            let merged =
                check.name.is_some() && checks.iter().any(|(held, _)| held.name == check.name);
            if !merged {
                checks.push((check, declared_by));
            }
        }
        for parent in &def.parents {
            self.checks_from(parent, false, visited, checks);
        }
    }

    /// The partition bounds a row loaded into table `table` must fall within:
    /// its own, then its parent's where that is a partition too, and so up,
    /// each paired with the partition it bounds — a partition's constraint
    /// holding every ancestor's (I86). Empty for a table that is no
    /// partition; a cycle, which no server holds, ends where it returns.
    pub fn partition_bounds(&self, table: &str) -> Vec<(&str, &PartitionOf)> {
        let mut bounds: Vec<(&str, &PartitionOf)> = Vec::new();
        let mut next = self.tables.get_key_value(table);
        while let Some((name, def)) = next
            && let Some(partition_of) = &def.partition_of
            && !bounds.iter().any(|(held, _)| held == name)
        {
            bounds.push((name.as_str(), partition_of));
            next = self.tables.get_key_value(&partition_of.parent);
        }
        bounds
    }

    /// Whether some table this preamble declares is a partition of table
    /// `table`, so a row loaded into `table` is routed to one by their bounds.
    pub fn has_partitions(&self, table: &str) -> bool {
        self.tables.values().any(|def| def.partition_of.as_ref().is_some_and(|p| p.parent == table))
    }

    /// **Every table whose rows a read of table `table` reads beside its
    /// own**, as `SELECT … FROM table` does on the server: each table naming
    /// it among its `INHERITS` parents or as the table it is a partition of,
    /// and each of theirs, at any depth, each once — a child of two parents
    /// both reach being one descendant. Breadth first, the first level in
    /// key order and each later one in its parents' order, a parent's
    /// children in key order; `table` itself never, a cycle, which no server holds, ending
    /// where it returns. Empty for a table nothing declares a child of.
    pub fn descendants(&self, table: &str) -> Vec<&str> {
        let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (name, def) in &self.tables {
            for parent in def.parents.iter().chain(def.partition_of.iter().map(|p| &p.parent)) {
                children.entry(parent.as_str()).or_default().push(name.as_str());
            }
        }
        let mut found: Vec<&str> = Vec::new();
        let mut next = 0;
        let mut level: Vec<&str> = children.get(table).cloned().unwrap_or_default();
        loop {
            for child in level {
                if child != table && !found.contains(&child) {
                    found.push(child);
                }
            }
            if next == found.len() {
                return found;
            }
            level = found[next..]
                .iter()
                .flat_map(|t| children.get(t).into_iter().flatten().copied())
                .collect();
            next = found.len();
        }
    }

    /// **Every table a read of which reads table `table`'s rows**, the
    /// inverse of [`descendants`](Self::descendants): each `INHERITS` parent
    /// and the table it is a partition of, where the preamble declares them,
    /// and each of theirs, at any depth, each once, the nearest first.
    /// `table` itself never; a cycle ends where it returns.
    pub fn ancestors(&self, table: &str) -> Vec<&str> {
        let mut found: Vec<&str> = Vec::new();
        let mut next = 0;
        let mut level: Vec<&str> = vec![table];
        loop {
            for parent in level.iter().filter_map(|t| self.tables.get(*t)).flat_map(|def| {
                def.parents.iter().chain(def.partition_of.iter().map(|p| &p.parent))
            }) {
                let Some((parent, _)) = self.tables.get_key_value(parent.as_str()) else {
                    continue;
                };
                if parent != table && !found.contains(&parent.as_str()) {
                    found.push(parent);
                }
            }
            if next == found.len() {
                return found;
            }
            level = found[next..].to_vec();
            next = found.len();
        }
    }

    /// The fields of the composite `type_name` names, where it parsed, the
    /// two names compared in their canonical spelling (I29).
    fn composite_fields(&self, type_name: &str) -> Option<&[ColumnDef]> {
        let key = canonical_type_name(type_name)?;
        self.types.iter().find_map(|t| match &t.kind {
            TypeKind::Composite { fields: Some(fields) }
                if canonical_type_name(&t.name).is_some_and(|name| name == key) =>
            {
                Some(fields.as_slice())
            }
            _ => None,
        })
    }
}

/// What a crate above calls on [`DatabaseMetadata`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait DatabaseMetadataExt {
    fn empty(name: Option<String>) -> Self;
}

impl DatabaseMetadataExt for DatabaseMetadata {
    fn empty(name: Option<String>) -> Self {
        Self {
            name,
            preamble_complete: false,
            server_version: None,
            pg_dump_version: None,
            extensions: Vec::new(),
            types: Vec::new(),
            collations: Vec::new(),
            tables: BTreeMap::new(),
        }
    }
}

impl ColumnDef {
    /// A column carrying no `COLLATE` clause and no `NOT NULL` — the ordinary
    /// case, and the one a test or an embedder building metadata by hand
    /// wants.
    pub fn new(name: impl Into<String>, declared_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            declared_type: declared_type.into(),
            collation: None,
            not_null: None,
        }
    }
}

impl TableDef {
    /// A table declaring `columns` and referring to nothing — the ordinary
    /// shape, and the one a test or an embedder building metadata by hand
    /// wants.
    pub fn with_columns(columns: Vec<ColumnDef>) -> Self {
        Self { columns, ..Self::default() }
    }
}

/// What a crate above calls on [`TableDef`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait TableDefExt {
    fn add(&mut self, reference: &TableReference);
}

impl TableDefExt for TableDef {
    fn add(&mut self, reference: &TableReference) {
        match reference {
            TableReference::Parent(parent) if !self.parents.contains(parent) => {
                self.parents.push(parent.clone());
            }
            TableReference::Parent(_) => {}
            TableReference::OfType(of_type) => self.of_type = Some(of_type.clone()),
            TableReference::NotNull(column) => {
                self.not_null.push((column.clone(), NotNull::Inherited));
            }
            TableReference::Check(check) => self.checks.push(check.clone()),
            TableReference::PartitionOf(partition_of) => {
                self.partition_of = Some(partition_of.clone());
            }
        }
    }
}

impl TypeKind {
    /// A domain carrying no `COLLATE` clause, `NOT NULL` or `CHECK` of its
    /// own — the ordinary shape, and the one a test or an embedder building a
    /// type list by hand wants.
    pub fn domain(base_type: impl Into<String>) -> Self {
        Self::Domain { base_type: base_type.into(), collation: None, not_null: false, check: false }
    }

    /// A base type declaring no `DELIMITER`, so `,` — the ordinary shape, and
    /// the one a test or an embedder building a type list by hand wants.
    pub fn base() -> Self {
        Self::Base { delimiter: Some(b',') }
    }

    /// An enum whose labels are exactly these, in declaration order — what
    /// `CREATE TYPE … AS ENUM (…)` declares, and the shape a test or an
    /// embedder building a type list by hand wants.
    pub fn exact_enum<S: Into<String>>(labels: impl IntoIterator<Item = S>) -> Self {
        Self::Enum { labels: labels.into_iter().map(Into::into).collect(), exact: true }
    }
}

#[cfg(test)]
mod tests;

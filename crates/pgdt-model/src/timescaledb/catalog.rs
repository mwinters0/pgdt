//! One database's catalog blocks, read by column name: the view every
//! conclusion is drawn over.

use std::collections::BTreeMap;

use pgdt_core::internal::Cursor;

use super::recorded::CATALOG_SCHEMA;
use crate::{CopyBlock, DumpIndex};

/// The extension's name, as `CREATE EXTENSION` writes it.
pub const EXTENSION: &str = "timescaledb";

/// One database's catalog blocks, by table: every block's header, and the
/// rows of those a pass recorded.
pub(super) struct Catalog<'a> {
    pub(super) extension: bool,
    pub(super) tables: BTreeMap<&'a str, Vec<&'a CopyBlock>>,
}

impl<'a> Catalog<'a> {
    // timescaledb: TS2 — detected by the extension or by the catalog's own blocks.
    pub(super) fn of(index: &'a DumpIndex, database: Option<&str>) -> Option<Self> {
        let extension = index
            .metadata
            .as_ref()
            .and_then(|metadata| {
                metadata.databases.iter().find(|db| db.name.as_deref() == database)
            })
            .is_some_and(|db| db.extensions.iter().any(|e| e.name == EXTENSION));
        let mut tables: BTreeMap<&str, Vec<&CopyBlock>> = BTreeMap::new();
        for (db, block) in index.blocks_with_database() {
            if db == database && block.header.schema.as_deref() == Some(CATALOG_SCHEMA) {
                tables.entry(block.header.table.as_str()).or_default().push(block);
            }
        }
        (extension || !tables.is_empty()).then_some(Catalog { extension, tables })
    }

    /// Whether the catalog holds a block for `table`.
    pub(super) fn has_table(&self, table: &str) -> bool {
        self.tables.contains_key(table)
    }

    /// Whether every block of `table` names `column`; `None` where it holds
    /// no block for it.
    pub(super) fn has_column(&self, table: &str, column: &str) -> Option<bool> {
        let blocks = self.tables.get(table)?;
        Some(blocks.iter().all(|block| block.header.columns.iter().any(|c| c == column)))
    }

    /// The rows of `table`, every recorded block's in file order, each read
    /// by column name.
    pub(super) fn rows(&self, table: &str) -> impl Iterator<Item = Row<'a>> + '_ {
        self.tables.get(table).into_iter().flatten().flat_map(|block| {
            let columns = block.header.columns.as_slice();
            block.recorded_rows.iter().flat_map(|recorded| &recorded.rows).map(move |fields| Row {
                table: block.header.table.as_str(),
                columns,
                fields,
            })
        })
    }
}

/// One recorded row, read by column name.
#[derive(Clone, Copy)]
pub(super) struct Row<'a> {
    pub(super) table: &'a str,
    columns: &'a [String],
    fields: &'a [Option<String>],
}

impl<'a> Row<'a> {
    /// The text of `column`, `None` for a NULL; `Err` where the row's block
    /// does not name the column or the row holds no field for it.
    pub(super) fn text(&self, column: &str) -> Result<Option<&'a str>, String> {
        let position = self.columns.iter().position(|c| c == column);
        match position.and_then(|p| self.fields.get(p)) {
            Some(field) => Ok(field.as_deref()),
            None => Err(format!("{}.{column}", self.table)),
        }
    }
}

/// A `regclass` value's text, `pg_dump` having written it schema-qualified
/// (TS2), as `schema.table` the way `CopyHeader::qualified_name` spells
/// it; `None` where it does not read as a qualified name.
pub(super) fn regclass_name(text: &str) -> Option<String> {
    let mut cursor = Cursor::new(text.as_bytes());
    let schema = cursor.parse_ident()?;
    if !cursor.eat_byte(b'.') {
        return None;
    }
    let table = cursor.parse_ident()?;
    (cursor.pos() == text.len()).then(|| format!("{schema}.{table}"))
}

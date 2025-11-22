use crate::db::DbResult;
use crate::schema::initialize_schema;
use rusqlite::{Connection, Transaction};

use super::DbHandle;

#[derive(Debug)]
pub struct CatalogDb {
    conn: Connection,
}

impl CatalogDb {
    pub fn open(path: &str) -> DbResult<Self> {
        let conn = Connection::open(path)?;
        initialize_schema(&conn)?;
        Ok(Self { conn })
    }

    pub fn in_memory() -> DbResult<Self> {
        let conn = Connection::open_in_memory()?;
        initialize_schema(&conn)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn transaction(&mut self) -> rusqlite::Result<Transaction<'_>> {
        self.conn.transaction()
    }
}

impl DbHandle for CatalogDb {
    fn execute(&self, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<usize> {
        self.conn.execute(sql, params)
    }

    fn prepare<'a>(&'a self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>> {
        self.conn.prepare(sql)
    }

    fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }
}

use crate::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// One SQLite connection shared by the services. Never hold the lock across an await.
pub type Db = Arc<Mutex<Connection>>;

pub fn shared(conn: Connection) -> Db {
    Arc::new(Mutex::new(conn))
}

const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    init(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    init(Connection::open_in_memory()?)
}

fn init(conn: Connection) -> Result<Connection> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let current: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        conn.execute_batch(sql)?;
        conn.pragma_update(None, "user_version", (i + 1) as i64)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        drop(open(&path).unwrap());
        let conn = open(&path).unwrap();
        let v: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, MIGRATIONS.len());
    }
}

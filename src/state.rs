use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::TransactionBehavior;
use rusqlite::params;

use crate::config::Baseline;
use crate::config::State;

const APPLICATION_ID: i64 = 0x4654_524c;
const SCHEMA_VERSION: i64 = 1;
const DATABASE: &str = "state.db";

pub fn load(root: &Path) -> Result<State> {
    let path = root.join(DATABASE);
    if !exists(&path)? {
        // Read-only operations and dry runs must not create the database.
        return Ok(State::default());
    }
    let mut connection = open(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let transaction = connection.transaction()?;
    let state = read(&transaction)
        .context("cannot read state.db; refusing to discard synchronization history")?;
    transaction.commit()?;
    Ok(state)
}

pub fn save(root: &Path, state: &State) -> Result<()> {
    let path = root.join(DATABASE);
    if exists(&path)? {
        let mut connection = open(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        return write(&mut connection, state);
    }
    // Publish a new database only after its schema and initial state are complete.
    let temporary = tempfile::NamedTempFile::new_in(root)?;
    let mut connection = open(temporary.path(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    initialize(&mut connection)?;
    write(&mut connection, state)?;
    connection.close().map_err(|(_, error)| error)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(&path)
        .map_err(|error| error.error)?;
    fs::File::open(root)?.sync_all()?;
    Ok(())
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn open(path: &Path, flags: OpenFlags) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, flags)
        .with_context(|| format!("cannot open {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}

fn initialize(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE ownership (
            path TEXT PRIMARY KEY NOT NULL,
            entry_id INTEGER NOT NULL CHECK (entry_id >= 0)
        ) STRICT;
        CREATE TABLE baselines (
            path TEXT PRIMARY KEY NOT NULL,
            entry_id INTEGER NOT NULL CHECK (entry_id >= 0),
            fingerprint TEXT NOT NULL
        ) STRICT;
        CREATE TABLE conflicts (
            path TEXT PRIMARY KEY NOT NULL,
            reason TEXT NOT NULL
        ) STRICT;
        CREATE TABLE sync_metadata (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            last_sync INTEGER CHECK (last_sync >= 0)
        ) STRICT;
        INSERT INTO sync_metadata (id, last_sync) VALUES (1, NULL);",
    )?;
    transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn read(connection: &Connection) -> Result<State> {
    let application: i64 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if application != APPLICATION_ID || version != SCHEMA_VERSION {
        bail!("unrecognized state database or unsupported schema version {version}");
    }
    let mut state = State::default();
    let mut statement = connection.prepare("SELECT path, entry_id FROM ownership")?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (path, id) = row?;
        state.owned.insert(path, u64::try_from(id)?);
    }
    let mut statement = connection.prepare("SELECT path, entry_id, fingerprint FROM baselines")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (path, entry, fingerprint) = row?;
        state.files.insert(
            path,
            Baseline {
                entry: u64::try_from(entry)?,
                fingerprint,
            },
        );
    }
    let mut statement = connection.prepare("SELECT path, reason FROM conflicts")?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (path, reason) = row?;
        state.conflicts.insert(path, reason);
    }
    let last_sync: Option<i64> = connection.query_row(
        "SELECT last_sync FROM sync_metadata WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    state.last_sync = last_sync.map(u64::try_from).transpose()?;
    Ok(state)
}

fn write(connection: &mut Connection, state: &State) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous = read(&transaction)?;
    {
        let mut remove = transaction.prepare("DELETE FROM ownership WHERE path = ?1")?;
        let mut upsert = transaction.prepare("INSERT INTO ownership (path, entry_id) VALUES (?1, ?2) ON CONFLICT(path) DO UPDATE SET entry_id = excluded.entry_id")?;
        for path in previous
            .owned
            .keys()
            .filter(|path| !state.owned.contains_key(*path))
        {
            remove.execute([path])?;
        }
        for (path, id) in &state.owned {
            if previous.owned.get(path) != Some(id) {
                upsert.execute(params![path, i64::try_from(*id)?])?;
            }
        }
    }
    {
        let mut remove = transaction.prepare("DELETE FROM baselines WHERE path = ?1")?;
        let mut upsert = transaction.prepare("INSERT INTO baselines (path, entry_id, fingerprint) VALUES (?1, ?2, ?3) ON CONFLICT(path) DO UPDATE SET entry_id = excluded.entry_id, fingerprint = excluded.fingerprint")?;
        for path in previous
            .files
            .keys()
            .filter(|path| !state.files.contains_key(*path))
        {
            remove.execute([path])?;
        }
        for (path, baseline) in &state.files {
            if previous.files.get(path) != Some(baseline) {
                upsert.execute(params![
                    path,
                    i64::try_from(baseline.entry)?,
                    baseline.fingerprint
                ])?;
            }
        }
    }
    {
        let mut remove = transaction.prepare("DELETE FROM conflicts WHERE path = ?1")?;
        let mut upsert = transaction.prepare("INSERT INTO conflicts (path, reason) VALUES (?1, ?2) ON CONFLICT(path) DO UPDATE SET reason = excluded.reason")?;
        for path in previous
            .conflicts
            .keys()
            .filter(|path| !state.conflicts.contains_key(*path))
        {
            remove.execute([path])?;
        }
        for (path, reason) in &state.conflicts {
            if previous.conflicts.get(path) != Some(reason) {
                upsert.execute(params![path, reason])?;
            }
        }
    }
    if previous.last_sync != state.last_sync {
        transaction.execute(
            "UPDATE sync_metadata SET last_sync = ?1 WHERE id = 1",
            [state.last_sync.map(i64::try_from).transpose()?],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

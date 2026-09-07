use std::fs;

use filetrail::config::Baseline;
use filetrail::config::State;
use filetrail::config::Store;
use rusqlite::Connection;

fn sample() -> State {
    let mut state = State::default();
    state.owned.insert("config/a'\"\nfile".into(), 7);
    state.owned.insert("config/deleted".into(), 8);
    state.files.insert(
        "config/a'\"\nfile".into(),
        Baseline {
            entry: 7,
            fingerprint: "content fingerprint".into(),
        },
    );
    state
        .conflicts
        .insert("unowned".into(), "target already exists".into());
    state.last_sync = Some(123456789);
    state
}

#[test]
fn database_roundtrip_reopen_and_removal_preserve_ownership() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::new(temporary.path().join("data")).unwrap();
    assert_eq!(store.state().unwrap(), State::default());
    assert!(!store.root.join("state.db").exists());
    let state = sample();
    store.save_state(&state).unwrap();
    assert!(
        fs::read(store.root.join("state.db"))
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    let reopened = Store::new(store.root.clone()).unwrap();
    assert_eq!(reopened.state().unwrap(), state);
    let mut updated = state.clone();
    updated.files.clear();
    updated.conflicts.clear();
    updated.owned.remove("config/deleted");
    updated.last_sync = None;
    reopened.save_state(&updated).unwrap();
    assert_eq!(store.state().unwrap(), updated);
    assert!(
        store
            .state()
            .unwrap()
            .owned
            .contains_key("config/a'\"\nfile")
    );
}

#[test]
fn failed_update_rolls_back_all_tables() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::new(temporary.path().join("data")).unwrap();
    let original = sample();
    store.save_state(&original).unwrap();
    let mut invalid = State::default();
    invalid.owned.insert("new".into(), 10);
    invalid
        .conflicts
        .insert("different".into(), "changed".into());
    // SQLite cannot represent this value. Earlier table updates must also roll back.
    invalid.last_sync = Some(u64::MAX);
    assert!(store.save_state(&invalid).is_err());
    assert_eq!(store.state().unwrap(), original);
}

#[test]
fn failed_initial_write_does_not_publish_partial_database() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::new(temporary.path().join("data")).unwrap();
    let mut state = sample();
    state.last_sync = Some(u64::MAX);
    assert!(store.save_state(&state).is_err());
    assert!(!store.root.join("state.db").exists());
    state.last_sync = Some(10);
    store.save_state(&state).unwrap();
    assert_eq!(store.state().unwrap(), state);
}

#[test]
fn unsupported_schema_is_not_overwritten() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::new(temporary.path().join("data")).unwrap();
    store.save_state(&sample()).unwrap();
    let path = store.root.join("state.db");
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 999).unwrap();
    drop(connection);
    let before = fs::read(&path).unwrap();
    assert!(store.state().is_err());
    assert!(store.save_state(&State::default()).is_err());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn unchanged_baselines_are_not_rewritten() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::new(temporary.path().join("data")).unwrap();
    let mut state = sample();
    store.save_state(&state).unwrap();
    let connection = Connection::open(store.root.join("state.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_update BEFORE UPDATE ON baselines BEGIN SELECT RAISE(ABORT, 'unchanged baseline was rewritten'); END;").unwrap();
    drop(connection);
    state.last_sync = Some(123456790);
    store.save_state(&state).unwrap();
    assert_eq!(store.state().unwrap(), state);
}

#[test]
fn database_corruption_blocks_reads_and_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::new(temporary.path().join("data")).unwrap();
    let path = store.root.join("state.db");
    fs::write(&path, "corrupted database").unwrap();
    assert!(store.state().is_err());
    assert!(store.save_state(&sample()).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "corrupted database");
}

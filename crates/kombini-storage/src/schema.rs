//! Schema migrations keyed on `PRAGMA user_version`.

use rusqlite::Connection;

/// Each entry migrates from version `index` to `index + 1`. Append only.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE games (
  id TEXT PRIMARY KEY,
  path TEXT NOT NULL,
  last_build TEXT,
  active_profile TEXT NOT NULL DEFAULT 'default',
  committed_txn TEXT,
  created_at INTEGER NOT NULL
);
CREATE TABLE mods (
  game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  doc TEXT NOT NULL,
  PRIMARY KEY (game_id, id)
);
CREATE TABLE profiles (
  game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  name TEXT NOT NULL,
  position INTEGER NOT NULL,
  autoexec TEXT,
  launch_options TEXT,
  ignored_conflicts TEXT NOT NULL DEFAULT '[]',
  PRIMARY KEY (game_id, id)
);
CREATE TABLE profile_mods (
  game_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  mod_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  enabled INTEGER NOT NULL,
  pinned_position INTEGER,
  PRIMARY KEY (game_id, profile_id, mod_id),
  FOREIGN KEY (game_id, profile_id) REFERENCES profiles(game_id, id) ON DELETE CASCADE
);
CREATE TABLE groups (
  game_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  id TEXT NOT NULL,
  name TEXT NOT NULL,
  position INTEGER NOT NULL,
  collapsed INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (game_id, id),
  FOREIGN KEY (game_id, profile_id) REFERENCES profiles(game_id, id) ON DELETE CASCADE
);
CREATE TABLE group_mods (
  game_id TEXT NOT NULL,
  group_id TEXT NOT NULL,
  mod_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (game_id, group_id, mod_id),
  FOREIGN KEY (game_id, group_id) REFERENCES groups(game_id, id) ON DELETE CASCADE
);
CREATE TABLE downloads (
  id TEXT PRIMARY KEY,
  game_id TEXT,
  doc TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE actions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  game_id TEXT NOT NULL,
  ts INTEGER NOT NULL,
  action TEXT NOT NULL,
  summary TEXT NOT NULL,
  details TEXT,
  snapshot_id TEXT
);
CREATE INDEX actions_game_ts ON actions(game_id, ts DESC);
CREATE TABLE snapshots (
  id TEXT PRIMARY KEY,
  game_id TEXT NOT NULL,
  label TEXT NOT NULL,
  kind TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  manifest TEXT NOT NULL,
  size INTEGER NOT NULL
);
CREATE INDEX snapshots_game ON snapshots(game_id, created_at DESC);
CREATE TABLE launches (
  id TEXT PRIMARY KEY,
  game_id TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  doc TEXT NOT NULL
);
CREATE TABLE blobs (hash TEXT PRIMARY KEY, size INTEGER NOT NULL, created_at INTEGER NOT NULL);
"#];

pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let target = index as i64 + 1;
        if current >= target {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", target)?;
        tx.commit()?;
        tracing::info!(version = target, "applied database migration");
    }
    Ok(())
}

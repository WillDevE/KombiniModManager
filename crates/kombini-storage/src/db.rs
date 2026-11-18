//! SQLite-backed persistence for library state, history, snapshots and downloads.

use crate::{Result, StorageError, io_err, now, schema};
use kombini_core::{Group, GroupId, Mod, ModId, Profile, ProfileId};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRow {
    pub id: String,
    pub path: String,
    pub last_build: Option<String>,
    pub active_profile: String,
    pub committed_txn: Option<String>,
}

/// Everything the engine needs in memory for one game install.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryState {
    pub mods: BTreeMap<ModId, Mod>,
    /// In display order; never empty after `load_library`.
    pub profiles: Vec<Profile>,
    pub active_profile: ProfileId,
}

impl LibraryState {
    pub fn empty() -> Self {
        Self {
            mods: BTreeMap::new(),
            profiles: vec![Profile::default_profile()],
            active_profile: ProfileId::default_profile(),
        }
    }

    pub fn profile(&self) -> &Profile {
        self.profiles
            .iter()
            .find(|p| p.id == self.active_profile)
            .unwrap_or(&self.profiles[0])
    }

    pub fn profile_mut(&mut self) -> &mut Profile {
        let index = self
            .profiles
            .iter()
            .position(|p| p.id == self.active_profile)
            .unwrap_or(0);
        &mut self.profiles[index]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct ActionRecord {
    pub id: i64,
    pub ts: i64,
    pub action: String,
    pub summary: String,
    pub details: Option<serde_json::Value>,
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct SnapshotRow {
    pub id: String,
    pub game_id: String,
    pub label: String,
    /// "auto" or "manual".
    pub kind: String,
    pub created_at: i64,
    /// JSON manifest (see engine::snapshots).
    pub manifest: String,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct LaunchRow {
    pub id: String,
    pub started_at: i64,
    pub doc: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct DownloadRow {
    pub id: String,
    pub game_id: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub doc: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenReport {
    /// Set when the previous database was unreadable and was moved aside.
    pub corrupt_backup: Option<PathBuf>,
}

pub struct Storage {
    conn: Connection,
}

fn to_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|source| StorageError::Json {
        what: "serialize".into(),
        source,
    })
}

fn from_json<T: DeserializeOwned>(what: &str, text: &str) -> Result<T> {
    serde_json::from_str(text).map_err(|source| StorageError::Json {
        what: what.to_owned(),
        source,
    })
}

impl Storage {
    /// Opens (creating if needed) the database. A database that fails to open
    /// or fails `PRAGMA quick_check` is renamed to `<name>.corrupt-<ts>` and a
    /// fresh one is created; the report says so, and nothing is deleted.
    pub fn open(path: &Path) -> Result<(Self, OpenReport)> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        let mut report = OpenReport::default();
        match Self::try_open(path) {
            Ok(storage) => Ok((storage, report)),
            Err(err) => {
                tracing::error!(error = %err, path = %path.display(), "database unusable; moving it aside");
                let backup = path.with_extension(format!("db.corrupt-{}", now()));
                std::fs::rename(path, &backup).map_err(|e| io_err(path, e))?;
                for suffix in ["-wal", "-shm"] {
                    let side = PathBuf::from(format!("{}{suffix}", path.display()));
                    if side.exists() {
                        let target = PathBuf::from(format!("{}{suffix}", backup.display()));
                        let _ = std::fs::rename(&side, target);
                    }
                }
                report.corrupt_backup = Some(backup);
                Ok((Self::try_open(path)?, report))
            }
        }
    }

    fn try_open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        Self::configure(&mut conn)?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(StorageError::Sql(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some(check),
            )));
        }
        schema::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        Self::configure(&mut conn)?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    fn configure(conn: &mut Connection) -> Result<()> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    // ------------------------------------------------------------ settings

    pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw: Option<String> = self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        match raw {
            Some(text) => match serde_json::from_str(&text) {
                Ok(v) => Ok(Some(v)),
                Err(err) => {
                    tracing::warn!(key, error = %err, "ignoring unreadable setting");
                    Ok(None)
                }
            },
            None => Ok(None),
        }
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, to_json(value)?],
        )?;
        Ok(())
    }

    pub fn all_settings(&self) -> Result<BTreeMap<String, serde_json::Value>> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (k, v) = row?;
            if let Ok(value) = serde_json::from_str(&v) {
                out.insert(k, value);
            }
        }
        Ok(out)
    }

    // --------------------------------------------------------------- games

    pub fn upsert_game(&self, id: &str, path: &str) -> Result<GameRow> {
        self.conn.execute(
            "INSERT INTO games(id, path, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET path = excluded.path",
            params![id, path, now()],
        )?;
        self.game(id)?
            .ok_or_else(|| StorageError::Sql(rusqlite::Error::QueryReturnedNoRows))
    }

    pub fn game(&self, id: &str) -> Result<Option<GameRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, path, last_build, active_profile, committed_txn FROM games WHERE id = ?1",
                [id],
                |r| {
                    Ok(GameRow {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        last_build: r.get(2)?,
                        active_profile: r.get(3)?,
                        committed_txn: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_last_build(&self, game_id: &str, build: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET last_build = ?2 WHERE id = ?1",
            params![game_id, build],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------- library

    pub fn load_library(&self, game_id: &str) -> Result<LibraryState> {
        let game = self.game(game_id)?;
        let mut state = LibraryState::empty();
        let mut stmt = self
            .conn
            .prepare("SELECT id, doc FROM mods WHERE game_id = ?1")?;
        let rows = stmt.query_map([game_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, doc) = row?;
            match from_json::<Mod>(&format!("mod {id}"), &doc) {
                Ok(m) => {
                    state.mods.insert(m.id.clone(), m);
                }
                // Keep going: one bad document must not hide the whole library.
                Err(err) => tracing::error!(error = %err, "skipping unreadable mod record"),
            }
        }

        let mut profiles = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT id, name, autoexec, launch_options, ignored_conflicts
             FROM profiles WHERE game_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map([game_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (id, name, autoexec, launch_options, ignored) = row?;
            let mut profile = Profile::new(ProfileId::new(id), name);
            profile.autoexec = autoexec;
            profile.launch_options = launch_options;
            profile.ignored_conflicts =
                from_json::<BTreeSet<String>>("ignored_conflicts", &ignored).unwrap_or_default();
            profiles.push(profile);
        }
        for profile in &mut profiles {
            self.load_profile_members(game_id, profile)?;
        }
        if !profiles.is_empty() {
            state.profiles = profiles;
        }
        if let Some(game) = game {
            let active = ProfileId::new(game.active_profile);
            if state.profiles.iter().any(|p| p.id == active) {
                state.active_profile = active;
            } else {
                state.active_profile = state.profiles[0].id.clone();
            }
        }
        Ok(state)
    }

    fn load_profile_members(&self, game_id: &str, profile: &mut Profile) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT mod_id, enabled, pinned_position FROM profile_mods
             WHERE game_id = ?1 AND profile_id = ?2 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![game_id, profile.id.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        let mut pinned: Vec<(i64, ModId)> = Vec::new();
        for row in rows {
            let (mod_id, enabled, pin) = row?;
            let id = ModId::new(mod_id);
            if enabled {
                profile.enabled.insert(id.clone());
            }
            if let Some(pos) = pin {
                pinned.push((pos, id.clone()));
            }
            profile.order.push(id);
        }
        pinned.sort_by_key(|(pos, _)| *pos);
        profile.pinned = pinned.into_iter().map(|(_, id)| id).collect();

        let mut stmt = self.conn.prepare(
            "SELECT id, name, collapsed FROM groups
             WHERE game_id = ?1 AND profile_id = ?2 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![game_id, profile.id.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?;
        let mut groups = Vec::new();
        for row in rows {
            let (id, name, collapsed) = row?;
            groups.push(Group {
                id: GroupId::new(id),
                name,
                mod_ids: Vec::new(),
                collapsed,
            });
        }
        for group in &mut groups {
            let mut stmt = self.conn.prepare(
                "SELECT mod_id FROM group_mods WHERE game_id = ?1 AND group_id = ?2 ORDER BY position",
            )?;
            let rows = stmt.query_map(params![game_id, group.id.as_str()], |r| {
                r.get::<_, String>(0)
            })?;
            for row in rows {
                group.mod_ids.push(ModId::new(row?));
            }
        }
        profile.groups = groups;
        Ok(())
    }

    /// Replaces the whole library for a game in one transaction. When
    /// `committed_txn` is given it is recorded atomically with the state (used
    /// by layout transaction recovery).
    pub fn save_library(
        &mut self,
        game_id: &str,
        state: &LibraryState,
        committed_txn: Option<&str>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM mods WHERE game_id = ?1", [game_id])?;
        {
            let mut insert =
                tx.prepare("INSERT INTO mods(game_id, id, doc) VALUES (?1, ?2, ?3)")?;
            for m in state.mods.values() {
                insert.execute(params![game_id, m.id.as_str(), to_json(m)?])?;
            }
        }
        tx.execute("DELETE FROM profiles WHERE game_id = ?1", [game_id])?;
        for (position, profile) in state.profiles.iter().enumerate() {
            write_profile(&tx, game_id, profile, position)?;
        }
        tx.execute(
            "UPDATE games SET active_profile = ?2 WHERE id = ?1",
            params![game_id, state.active_profile.as_str()],
        )?;
        if let Some(txn) = committed_txn {
            tx.execute(
                "UPDATE games SET committed_txn = ?2 WHERE id = ?1",
                params![game_id, txn],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Upserts one mod document (metadata-only changes such as favorite).
    pub fn save_mod(&self, game_id: &str, m: &Mod) -> Result<()> {
        self.conn.execute(
            "INSERT INTO mods(game_id, id, doc) VALUES (?1, ?2, ?3)
             ON CONFLICT(game_id, id) DO UPDATE SET doc = excluded.doc",
            params![game_id, m.id.as_str(), to_json(m)?],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------- actions

    pub fn log_action(
        &self,
        game_id: &str,
        action: &str,
        summary: &str,
        details: Option<&serde_json::Value>,
        snapshot_id: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO actions(game_id, ts, action, summary, details, snapshot_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                game_id,
                now(),
                action,
                summary,
                details.map(|d| d.to_string()),
                snapshot_id
            ],
        )?;
        // Keep the newest 500 records per game.
        self.conn.execute(
            "DELETE FROM actions WHERE game_id = ?1 AND id NOT IN
             (SELECT id FROM actions WHERE game_id = ?1 ORDER BY id DESC LIMIT 500)",
            [game_id],
        )?;
        Ok(())
    }

    pub fn recent_actions(&self, game_id: &str, limit: usize) -> Result<Vec<ActionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, action, summary, details, snapshot_id FROM actions
             WHERE game_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![game_id, limit as i64], |r| {
            Ok(ActionRecord {
                id: r.get(0)?,
                ts: r.get(1)?,
                action: r.get(2)?,
                summary: r.get(3)?,
                details: r
                    .get::<_, Option<String>>(4)?
                    .and_then(|d| serde_json::from_str(&d).ok()),
                snapshot_id: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ----------------------------------------------------------- snapshots

    pub fn insert_snapshot(&self, row: &SnapshotRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO snapshots(id, game_id, label, kind, created_at, manifest, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.id,
                row.game_id,
                row.label,
                row.kind,
                row.created_at,
                row.manifest,
                row.size
            ],
        )?;
        Ok(())
    }

    /// Newest first. `with_manifest = false` returns an empty manifest string
    /// (cheap listing).
    pub fn list_snapshots(&self, game_id: &str, with_manifest: bool) -> Result<Vec<SnapshotRow>> {
        let sql = if with_manifest {
            "SELECT id, game_id, label, kind, created_at, manifest, size FROM snapshots
             WHERE game_id = ?1 ORDER BY created_at DESC, rowid DESC"
        } else {
            "SELECT id, game_id, label, kind, created_at, '', size FROM snapshots
             WHERE game_id = ?1 ORDER BY created_at DESC, rowid DESC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([game_id], snapshot_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn snapshot(&self, id: &str) -> Result<Option<SnapshotRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, game_id, label, kind, created_at, manifest, size FROM snapshots WHERE id = ?1",
                [id],
                snapshot_from_row,
            )
            .optional()?)
    }

    pub fn delete_snapshot(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM snapshots WHERE id = ?1", [id])?
            > 0)
    }

    /// Every snapshot manifest (all games), for blob garbage collection.
    pub fn all_snapshot_manifests(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT manifest FROM snapshots")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every mod document (all games), for blob garbage collection.
    pub fn all_mod_docs(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT doc FROM mods")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ------------------------------------------------------------ launches

    pub fn save_launch(&self, game_id: &str, row: &LaunchRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO launches(id, game_id, started_at, doc) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET doc = excluded.doc",
            params![row.id, game_id, row.started_at, row.doc.to_string()],
        )?;
        self.conn.execute(
            "DELETE FROM launches WHERE game_id = ?1 AND id NOT IN
             (SELECT id FROM launches WHERE game_id = ?1 ORDER BY started_at DESC LIMIT 50)",
            [game_id],
        )?;
        Ok(())
    }

    pub fn recent_launches(&self, game_id: &str, limit: usize) -> Result<Vec<LaunchRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, started_at, doc FROM launches WHERE game_id = ?1
             ORDER BY started_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![game_id, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, started_at, doc) = row?;
            out.push(LaunchRow {
                id,
                started_at,
                doc: serde_json::from_str(&doc).unwrap_or(serde_json::Value::Null),
            });
        }
        Ok(out)
    }

    // ----------------------------------------------------------- downloads

    pub fn upsert_download(&self, row: &DownloadRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO downloads(id, game_id, doc, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET doc = excluded.doc, status = excluded.status",
            params![
                row.id,
                row.game_id,
                row.doc.to_string(),
                row.status,
                row.created_at
            ],
        )?;
        Ok(())
    }

    pub fn list_downloads(&self) -> Result<Vec<DownloadRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, game_id, doc, status, created_at FROM downloads ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, game_id, doc, status, created_at) = row?;
            out.push(DownloadRow {
                id,
                game_id,
                status,
                created_at,
                doc: serde_json::from_str(&doc).unwrap_or(serde_json::Value::Null),
            });
        }
        Ok(out)
    }

    pub fn delete_download(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM downloads WHERE id = ?1", [id])?;
        Ok(())
    }

    // --------------------------------------------------------------- blobs

    pub fn record_blob(&self, hash: &str, size: u64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO blobs(hash, size, created_at) VALUES (?1, ?2, ?3)",
            params![hash, size as i64, now()],
        )?;
        Ok(())
    }

    pub fn forget_blob(&self, hash: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM blobs WHERE hash = ?1", [hash])?;
        Ok(())
    }
}

fn snapshot_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotRow> {
    Ok(SnapshotRow {
        id: r.get(0)?,
        game_id: r.get(1)?,
        label: r.get(2)?,
        kind: r.get(3)?,
        created_at: r.get(4)?,
        manifest: r.get(5)?,
        size: r.get(6)?,
    })
}

fn write_profile(tx: &Transaction<'_>, game_id: &str, p: &Profile, position: usize) -> Result<()> {
    tx.execute(
        "INSERT INTO profiles(game_id, id, name, position, autoexec, launch_options, ignored_conflicts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            game_id,
            p.id.as_str(),
            p.name,
            position as i64,
            p.autoexec,
            p.launch_options,
            to_json(&p.ignored_conflicts)?
        ],
    )?;
    let mut members: Vec<&ModId> = Vec::new();
    let mut seen = BTreeSet::new();
    for id in p
        .order
        .iter()
        .chain(p.enabled.iter())
        .chain(p.pinned.iter())
    {
        if seen.insert(id) {
            members.push(id);
        }
    }
    let mut insert = tx.prepare(
        "INSERT INTO profile_mods(game_id, profile_id, mod_id, position, enabled, pinned_position)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (pos, id) in members.iter().enumerate() {
        let pin = p.pinned.iter().position(|m| m == *id).map(|i| i as i64);
        insert.execute(params![
            game_id,
            p.id.as_str(),
            id.as_str(),
            pos as i64,
            p.enabled.contains(*id),
            pin
        ])?;
    }
    for (gpos, group) in p.groups.iter().enumerate() {
        tx.execute(
            "INSERT INTO groups(game_id, profile_id, id, name, position, collapsed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                game_id,
                p.id.as_str(),
                group.id.as_str(),
                group.name,
                gpos as i64,
                group.collapsed
            ],
        )?;
        let mut seen_members = BTreeSet::new();
        for (mpos, mod_id) in group.mod_ids.iter().enumerate() {
            if !seen_members.insert(mod_id) {
                continue;
            }
            tx.execute(
                "INSERT INTO group_mods(game_id, group_id, mod_id, position) VALUES (?1, ?2, ?3, ?4)",
                params![game_id, group.id.as_str(), mod_id.as_str(), mpos as i64],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kombini_core::{BlobHash, ModFile, ModSource, Placement};

    fn sample_mod(id: &str) -> Mod {
        let mut m = Mod::new(ModId::from(id), format!("Mod {id}"), ModSource::Local, 1);
        m.files.push(ModFile {
            rel_path: "pak01_dir.vpk".into(),
            original_name: "pak01_dir.vpk".into(),
            blob: BlobHash::from("ab"),
            size: 5,
            is_chunk: false,
            parent: None,
            chunk_index: None,
            selected: true,
            placement: Placement::Disabled {
                file_name: format!("{id}_pak01_dir.vpk"),
            },
        });
        m
    }

    #[test]
    fn library_round_trip() {
        let mut db = Storage::open_in_memory().unwrap();
        db.upsert_game("g1", "D:/Deadlock").unwrap();
        let mut state = LibraryState::empty();
        for id in ["gb1", "gb2", "gb3"] {
            state.mods.insert(ModId::from(id), sample_mod(id));
        }
        let p = state.profile_mut();
        p.order = vec!["gb2".into(), "gb1".into(), "gb3".into()];
        p.enabled = ["gb1".into(), "gb2".into()].into_iter().collect();
        p.pinned = vec!["gb1".into()];
        p.autoexec = Some("fps_max 240\n".into());
        p.ignored_conflicts.insert("a/b.vmdl_c".into());
        p.groups.push(Group {
            id: GroupId::from("g_1"),
            name: "HUD".into(),
            mod_ids: vec!["gb3".into(), "gb1".into()],
            collapsed: true,
        });
        let mut second = Profile::new(ProfileId::from("p_2"), "Second");
        second.enabled.insert("gb3".into());
        state.profiles.push(second);
        state.active_profile = ProfileId::from("p_2");

        db.save_library("g1", &state, Some("txn1")).unwrap();
        let loaded = db.load_library("g1").unwrap();
        assert_eq!(loaded.mods, state.mods);
        assert_eq!(loaded.active_profile, ProfileId::from("p_2"));
        let p = &loaded.profiles[0];
        assert_eq!(p.order, state.profiles[0].order);
        assert_eq!(p.enabled, state.profiles[0].enabled);
        assert_eq!(p.pinned, state.profiles[0].pinned);
        assert_eq!(p.groups, state.profiles[0].groups);
        assert_eq!(p.autoexec.as_deref(), Some("fps_max 240\n"));
        assert_eq!(p.ignored_conflicts, state.profiles[0].ignored_conflicts);
        // enabled-but-unordered mods become ordered members on load
        assert_eq!(loaded.profiles[1].order, vec![ModId::from("gb3")]);
        assert_eq!(
            db.game("g1").unwrap().unwrap().committed_txn.as_deref(),
            Some("txn1")
        );
    }

    #[test]
    fn empty_game_has_default_profile() {
        let db = Storage::open_in_memory().unwrap();
        db.upsert_game("g", "x").unwrap();
        let state = db.load_library("g").unwrap();
        assert_eq!(state.profiles.len(), 1);
        assert_eq!(state.active_profile, ProfileId::default_profile());
    }

    #[test]
    fn settings_actions_snapshots_downloads() {
        let db = Storage::open_in_memory().unwrap();
        db.upsert_game("g", "x").unwrap();
        db.set_setting("auto_snapshot", &true).unwrap();
        assert_eq!(db.get_setting::<bool>("auto_snapshot").unwrap(), Some(true));
        assert_eq!(db.get_setting::<bool>("nope").unwrap(), None);
        for i in 0..510 {
            db.log_action("g", "install", &format!("n{i}"), None, None)
                .unwrap();
        }
        let recent = db.recent_actions("g", 1000).unwrap();
        assert_eq!(recent.len(), 500);
        assert_eq!(recent[0].summary, "n509");

        let snap = SnapshotRow {
            id: "s1".into(),
            game_id: "g".into(),
            label: "before install".into(),
            kind: "auto".into(),
            created_at: 5,
            manifest: "{}".into(),
            size: 10,
        };
        db.insert_snapshot(&snap).unwrap();
        assert_eq!(db.list_snapshots("g", true).unwrap(), vec![snap.clone()]);
        assert_eq!(db.list_snapshots("g", false).unwrap()[0].manifest, "");
        assert!(db.delete_snapshot("s1").unwrap());

        let dl = DownloadRow {
            id: "d1".into(),
            game_id: Some("g".into()),
            status: "pending".into(),
            created_at: 1,
            doc: serde_json::json!({"url": "x"}),
        };
        db.upsert_download(&dl).unwrap();
        assert_eq!(db.list_downloads().unwrap(), vec![dl]);
        db.delete_download("d1").unwrap();
        assert!(db.list_downloads().unwrap().is_empty());
    }

    #[test]
    fn corrupt_database_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kombini.db");
        std::fs::write(
            &path,
            b"this is not a sqlite database at all, definitely not",
        )
        .unwrap();
        let (db, report) = Storage::open(&path).unwrap();
        assert!(report.corrupt_backup.as_ref().unwrap().exists());
        assert_eq!(db.schema_version().unwrap(), schema::SCHEMA_VERSION);
        drop(db);
        let (_db, report) = Storage::open(&path).unwrap();
        assert!(report.corrupt_backup.is_none());
    }
}

# Architecture

How Kombini Mod Manager is built: the decisions behind it, the crates, the rules it enforces in the
game folder, and the edge cases it handles. Section numbers are referenced from code comments.

Tests never write to a real game folder; they build synthetic game folders in temp directories.
A few read-only tests use a real install when the `DEADLOCK_PAK` environment variable points at its
`game\citadel\pak01_dir.vpk`, and are skipped otherwise.

## 1. Product decisions

- App name: **Kombini Mod Manager**. License: **MIT** (`LICENSE`).
- App identifier: `com.kombini.modmanager`. UI: **GPUI + gpui-component** (native Rust, GPU-rendered;
  no web engine), for low memory use and a single-language stack.
- App data: `%LOCALAPPDATA%\Kombini\ModManager\` (via `directories::ProjectDirs::from("com", "Kombini", "ModManager")`,
  `data_local_dir()`); cache: `cache_dir()`. Overrides: env `KOMBINI_DATA_DIR`, `KOMBINI_CACHE_DIR`.
- Windows first; Linux compiles and works best-effort; macOS not targeted.
- RAR: `unrar` crate (official UnRAR source compiled in; `THIRD-PARTY-NOTICES.md` carries the UnRAR
  license). If it fails, silently retry with Windows `%SystemRoot%\System32\tar.exe`; log both errors.
  ZIP: `zip`. 7z: `sevenz-rust2`.
- The managed `gameinfo.gi` block is marked so an existing block is found and adopted:
  `// >>> Kombini Mod Manager (managed block) >>>` and `// <<< Kombini Mod Manager <<<`.

## 2. Repository layout

```text
kombini-mod-manager/
├─ Cargo.toml                 # workspace (resolver 3, edition 2024)
├─ crates/
│  ├─ kombini-core/           # domain types + pure algorithms (layout, order, conflicts, heroes, safety)
│  ├─ kombini-vpk/            # VPK read/inspect/extract/write/merge
│  ├─ kombini-gameinfo/       # gameinfo.gi status/ensure/unpatch
│  ├─ kombini-archive/        # magic detection + safe extraction (zip/7z/rar/vpk)
│  ├─ kombini-steam/          # Steam libraries, build info, Steam news, launch, process detection
│  ├─ kombini-storage/        # SQLite (rusqlite bundled) + migrations + blob store
│  ├─ kombini-source2/        # Source 2 resource container + KV3 binary codec (vendored morphic, MIT)
│  ├─ kombini-repair/         # mod compatibility/attachment repairs (ported vpkmerge, MIT)
│  ├─ kombini-engine/         # application services: library, transactions, snapshots, health, launch
│  ├─ kombini-gamebanana/     # GameBanana apiv11 client
│  ├─ kombini-download/       # download queue
│  ├─ kombini-import/         # adopt unmanaged, interchange, mod-profile/mp1, DMM
│  ├─ kombini-cli/            # `kombini` binary: headless harness for every engine operation
│  └─ kombini-app/            # GPUI desktop app (gpui + gpui-component), the `Kombini Mod Manager` binary
└─ docs/
```

`kombini-core` stays free of IO and SQLite; `kombini-engine` owns the use cases (library, load order,
installer, snapshots, health).

Common conventions for all crates:
- Errors: `thiserror` enums per crate. The engine maps everything to `kombini_core::AppError`
  `{ code: ErrorCode, message: String, detail: Option<String> }`, where `ErrorCode` is a stable,
  `serde(rename_all = "snake_case")` enum (§9).
- Logging: `tracing`. No `println!` outside the CLI.
- No `unwrap()`/`expect()` in non-test code except for invariants that are provably unreachable
  (comment why).
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must pass.
- All file paths in public APIs are `&Path`/`PathBuf`; game paths inside VPKs are `String`
  using `/` separators, compared lowercase.

## 3. Deadlock rules (enforced)

- Addon shard N (1..=10): `game/citadel/addons` for 1, `game/citadel/addons{N}` for 2..=10.
- Mounted root: exactly `pakNN_dir.vpk`, NN = 01..=99 (case-insensitive match, written lowercase).
- Chunk: `pakNN_XXX.vpk` (XXX three digits) where NN equals its root.
- Lower (shard, NN) wins conflicts: shard 1 slot 1 is highest priority.
- Disabled managed file: `<mod_id>_<original_name>` in shard 1 (`addons`). The game ignores it.
- Reserved working dir: `game/citadel/addons/_kombini/` (subfolders are ignored by the game):
  `_kombini/txn/<txn_id>/` staging, `_kombini/txn/<txn_id>.journal.json`, `_kombini/trash/`.
- Foreign = any `*.vpk` directly in a shard folder that is not a managed file. Never renamed, moved,
  overwritten, or deleted. A foreign `pakNN_dir.vpk` reserves slot (shard, NN).
- gameinfo managed block = addon Game paths for shards `1..=max_used_shard` (at least shard 1), then
  `Mod citadel`, `Write citadel`, `Mod core`, `Write core`. Inserted before base `Game citadel`.

## 4. Domain model (`kombini-core`)

```rust
pub struct ModId(pub String);        // "gb<submission>", "local_<12 hex of sha256 of first root>", "merged_<unix_ms>"
pub struct ProfileId(pub String);    // "default" or "p_<12 hex random>"
pub struct GroupId(pub String);      // "g_<12 hex random>"
pub struct GameId(pub String);       // first 12 hex of sha1(lowercased canonical game path)
pub struct BlobHash(pub String);     // 64 lowercase hex sha256

pub enum ModSource { GameBanana, Local, Merged }

pub struct ModFile {
    pub rel_path: String,            // unique key within the mod: path inside the source archive/folder, '/'-separated,
                                     // e.g. "Blue/pak01_dir.vpk"; equals original_name for top-level files (see §13.4)
    pub original_name: String,       // e.g. "pak01_dir.vpk" or "skin_dir.vpk" or "skin_000.vpk"
    pub blob: BlobHash,              // content in the blob store
    pub size: u64,
    pub is_chunk: bool,
    pub parent: Option<String>,      // root original_name for chunks
    pub chunk_index: Option<u16>,
    pub selected: bool,              // roots only; chunks mirror their root
    pub placement: Placement,        // where it is on disk right now (from reconciliation)
}
pub enum Placement {
    Disabled { file_name: String },              // "<mod_id>_<original>" in shard 1
    Mounted  { shard: u8, file_name: String },   // "pakNN_dir.vpk" / "pakNN_XXX.vpk"
    Missing,
}

pub struct Mod {
    pub id: ModId, pub title: String, pub source: ModSource,
    pub remote_id: Option<u64>, pub author: Option<String>, pub version: Option<String>,
    pub category: Option<String>, pub page_url: Option<String>, pub description: Option<String>,
    pub thumbnail_url: Option<String>, pub gallery: Vec<String>,
    pub file_id: Option<u64>, pub file_name: Option<String>, pub md5: Option<String>,
    pub variant_desc: Option<String>,
    pub installed_at: i64, pub updated_at: i64,   // unix seconds
    pub files: Vec<ModFile>,
    pub update: UpdateState,
    pub heroes: Vec<String>, pub hero_override: Option<String>,
    pub requirements: Vec<Requirement>, pub safety: Vec<SafetyFinding>,
    pub favorite: bool, pub source_key: Option<String>, pub notes: Option<String>,
}
pub struct UpdateState { pub available: bool, pub latest_version: Option<String>,
    pub file_id: Option<u64>, pub checked_at: Option<i64>, pub skipped_file_id: Option<u64> }

pub struct Profile {
    pub id: ProfileId, pub name: String,
    pub order: Vec<ModId>,           // every mod the profile has ordered (enabled or not); position survives disable
    pub enabled: BTreeSet<ModId>,    // authoritative on/off
    pub pinned: Vec<ModId>,          // forced first, in this relative order
    pub groups: Vec<Group>,
    pub ignored_conflicts: BTreeSet<String>,  // lowercase game paths
    pub autoexec: Option<String>,    // None = inherit current autoexec.cfg
    pub launch_options: Option<String>,  // None = app default
}
pub struct Group { pub id: GroupId, pub name: String, pub mod_ids: Vec<ModId>, pub collapsed: bool }
```

Plus `Requirement { name, url, submission_id: Option<u64>, author }`,
`SafetyFinding { level: Level, code: SafetyCode /* GameplayData|Skeleton|Executable */, message, samples: Vec<String> }`,
`Conflict { path, owners: Vec<ModId> /* load order */, winner: ModId }`,
`HealthIssue { level: Level /* Error|Warning|Info */, code: String, message: String, mod_id: Option<ModId>, fix: Option<FixAction> }`.

All types derive `Debug, Clone, Serialize, Deserialize, PartialEq`.

### 4.1 Effective load order (pure)
`effective_order(profile, mods) -> Vec<ModId>`: pinned ∩ enabled ∩ existing (pinned order), then
`order` ∩ enabled ∩ existing minus pinned, then enabled ∩ existing not yet listed sorted by id.

### 4.2 Layout assignment (pure)
`assign_layout(ordered: &[&Mod], foreign_slots: &BTreeSet<(u8,u8)>) -> Result<Layout, LayoutError>`:
cursor (shard 1, slot 1). For each mod in order, for each **selected root** in `files` order: advance the
cursor past foreign-reserved slots; assign `pak{NN:02}_dir.vpk` in that shard; its chunks get
`pak{NN:02}_{idx:03}.vpk` with their original `chunk_index`; advance. Slot 100 → next shard slot 1.
Past shard 10 → `LayoutError::TooManyRoots { needed, capacity }`. Unselected roots, their chunks, and all
files of non-enabled mods get `Disabled { disabled_name(mod_id, rel_path) }` (§13.4.7) in shard 1. Output maps
`(ModId, rel_path) -> Placement`. Must be deterministic.

### 4.3 Conflicts (pure)
Input: ordered enabled mods with their (lowercased, deduped per mod) path lists of mounted roots.
A path owned by ≥2 distinct mods is a conflict; winner = first in order; ignored paths filtered.

## 5. Storage (`kombini-storage`)

SQLite file `<data>/kombini.db`, WAL, `foreign_keys=ON`, migrations via `rusqlite_migration`.
Mods are stored as one JSON document per row (queried only by id); profile membership/order is
normalized as required by the prompt.

```sql
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);            -- JSON values
CREATE TABLE games (id TEXT PRIMARY KEY, path TEXT NOT NULL, last_build TEXT,
  active_profile TEXT NOT NULL DEFAULT 'default', committed_txn TEXT, created_at INTEGER NOT NULL);
CREATE TABLE mods (game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE, id TEXT NOT NULL,
  doc TEXT NOT NULL, PRIMARY KEY (game_id, id));
CREATE TABLE profiles (game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE, id TEXT NOT NULL,
  name TEXT NOT NULL, position INTEGER NOT NULL, autoexec TEXT, launch_options TEXT,
  ignored_conflicts TEXT NOT NULL DEFAULT '[]', PRIMARY KEY (game_id, id));
CREATE TABLE profile_mods (game_id TEXT NOT NULL, profile_id TEXT NOT NULL, mod_id TEXT NOT NULL,
  position INTEGER NOT NULL, enabled INTEGER NOT NULL, pinned_position INTEGER,
  PRIMARY KEY (game_id, profile_id, mod_id),
  FOREIGN KEY (game_id, profile_id) REFERENCES profiles(game_id, id) ON DELETE CASCADE);
CREATE TABLE groups (game_id TEXT NOT NULL, profile_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL,
  position INTEGER NOT NULL, collapsed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (game_id, id),
  FOREIGN KEY (game_id, profile_id) REFERENCES profiles(game_id, id) ON DELETE CASCADE);
CREATE TABLE group_mods (game_id TEXT NOT NULL, group_id TEXT NOT NULL, mod_id TEXT NOT NULL,
  position INTEGER NOT NULL, PRIMARY KEY (game_id, group_id, mod_id),
  FOREIGN KEY (game_id, group_id) REFERENCES groups(game_id, id) ON DELETE CASCADE);
CREATE TABLE downloads (id TEXT PRIMARY KEY, game_id TEXT, doc TEXT NOT NULL, status TEXT NOT NULL,
  created_at INTEGER NOT NULL);
CREATE TABLE actions (id INTEGER PRIMARY KEY AUTOINCREMENT, game_id TEXT NOT NULL, ts INTEGER NOT NULL,
  action TEXT NOT NULL, summary TEXT NOT NULL, details TEXT, snapshot_id TEXT);
CREATE TABLE snapshots (id TEXT PRIMARY KEY, game_id TEXT NOT NULL, label TEXT NOT NULL,
  created_at INTEGER NOT NULL, manifest TEXT NOT NULL, size INTEGER NOT NULL);
CREATE TABLE launches (id TEXT PRIMARY KEY, game_id TEXT NOT NULL, started_at INTEGER NOT NULL, doc TEXT NOT NULL);
CREATE TABLE blobs (hash TEXT PRIMARY KEY, size INTEGER NOT NULL, created_at INTEGER NOT NULL);
```

Blob store: `<data>/blobs/<hash[0..2]>/<hash>` (immutable, content addressed). Insert = stream-copy to
`<data>/blobs/tmp/<random>` while hashing, fsync, rename to final path (skip if exists). GC deletes blobs
not referenced by any mod doc or snapshot manifest (run on demand and after snapshot pruning).

Corruption: on open failure / `PRAGMA integrity_check` != ok, rename the db to
`kombini.db.corrupt-<ts>`, create a fresh db, then the engine rebuilds what it can from disk
(managed disabled names reveal mod ids; blob store keeps content) and reports what was lost.

## 6. Engine (`kombini-engine`)

`Engine` is `Send + Sync`, created with `Engine::open(EngineConfig { data_dir, cache_dir, game_path })`.
All game-folder mutations take `self.writer: tokio::sync::Mutex<()>`-equivalent (a `parking_lot::Mutex`
in sync code; the GPUI app calls engine methods on its background executor). The engine API is
**synchronous**; async lives only in the gamebanana/download crates and the app layer.

Long operations accept `&dyn Progress` (`fn stage(&self, label: &str)`, `fn progress(&self, done: u64, total: u64)`,
`fn cancelled(&self) -> bool`).

### 6.1 Layout transaction
`apply_layout(target: Layout)` — the only code path that renames/places/removes managed files:
1. Guard: game running (if setting `block_while_running`, default true) → `ErrorCode::GameRunning`.
2. Build ops by diffing current placements vs target: `Move{from,to}`, `Place{blob,to}`, `Remove{from}`.
3. Validate: no two ops share a target; a target that exists must be the `from` of some op in this txn;
   no op touches a foreign file.
4. Write journal `_kombini/txn/<txn_id>.journal.json` (`{id, created_at, ops, phase:"staging"}`), fsync.
5. Phase 1: every `Move.from`/`Remove.from` → `_kombini/txn/<id>/<n>`; every `Place` blob is copied to
   `_kombini/txn/<id>/<n>`. Update journal phase `"placing"`.
6. Phase 2: staged → final targets (removes → `_kombini/trash/<id>/`).
7. Commit to SQLite in one transaction: new placements + `games.committed_txn = <id>`.
8. Delete journal, staging dir, and trash for this txn.
9. `ensure_gameinfo(max_used_shard)`.

Recovery at `Engine::open` (before reconciliation): for each journal found: if
`games.committed_txn == journal.id` → roll forward (finish phase 2 for any op still staged), else roll back
(move everything back to its original `from`; placed blobs are simply deleted). Never delete a file when
both its source and destination exist — move the extra copy into `_kombini/recovered/<ts>/` and report it.

### 6.2 Reconciliation (`Engine::open` and `rescan()`)
Scan shard folders. For each managed file in state, verify its recorded placement exists with the
recorded size; otherwise look for it under the other possible name (disabled name or any
`pakNN_*.vpk` whose size+sha256 matches the blob). Unmatched state files → `Placement::Missing`.
Unmatched disk files → foreign list. Disk is truth; never delete anything during reconciliation.

### 6.3 Snapshots (content addressed)
A snapshot is a JSON manifest row: `{ app_version, game_build, label, created_at, active_profile,
mods: [Mod docs], profiles: [...], placements, autoexec: Option<String>, gameinfo_text: String,
foreign: [{shard, name, size}] }`. Mod content lives in the blob store, so snapshots are instant and
dedupe automatically. Restore = (auto pre-restore snapshot) → write mods/profiles back → `apply_layout`
of the snapshot's placements → write autoexec → **reapply the managed gameinfo block to the current file**
(never paste old gameinfo) → rescan. Foreign files are listed, never copied/restored.
Retention: keep newest 20 auto snapshots; manual snapshots are never pruned automatically.
"Export snapshot" writes a zip with manifest + referenced blobs.

### 6.4 Install
`install_files(paths, meta: InstallMeta, selection: Option<Vec<String>>) -> ModId`. Group roots/chunks by
base, validate every root with `kombini-vpk` (open + bounds check), refuse if no VPK, insert blobs, build
`Mod` (preserve enabled/order/favorite/source_key/selection-by-name if the id exists), detect heroes and
safety, place new files as Disabled via `apply_layout` (or the full enabled layout if it was enabled).
New mods start disabled. `install_archive(path, meta)` extracts with `kombini-archive` into
`<cache>/extract/<random>/` then calls `install_files`; the temp dir is removed afterwards; the archive
is never deleted.

## 7. App ↔ engine boundary

The GPUI app (`crates/kombini-app`) links the engine crates directly (no IPC, no bindings). One root
entity, `AppView`, owns an `Arc<Engine>` and an `Arc<LibraryState>` snapshot that is re-read after every
operation and on addons-watcher events. All engine, network and file work goes through
`AppView::run` / `run_with` (tracked operations, keyed for double-submit protection) or
`AppView::background` (untracked reads such as health checks), both on `cx.background_executor()`.
Progress reaches the UI through `OpProgress` (atomics + a stage label implementing
`kombini_engine::Progress`); a 100 ms ticker samples it, so updates are coalesced to ≤ 10 Hz and the
UI repaints only while something is in flight. The same tick drains the download manager's change flag,
the watcher's dirty flag and the single-instance inbox.

## 8. UI

GPUI + `gpui-component`/`gpui-kit` 0.10 (pinned; GPUI is pre-1.0, upgrades are deliberate). Dark theme
from the §8.1 tokens applied through `Theme::update` (so component tokens stay in sync); Segoe UI.
Layout: top bar (profile switcher, status pill, Reapply, Launch Vanilla, Launch Modded) → banner slot
(launch state, health errors with one-click fix, other managers / unmanaged files, bisect) → tabs
(My Mods, Discover, Downloads, Settings) → status bar (current operation with progress and Cancel,
Activity history).

- My Mods: `uniform_list` (virtualized) rows with rank, drag handle, thumbnail, hero/update/problem
  chips and an optimistic switch with per-row pending state; drag-and-drop reorders the enabled set
  (floating row preview, directional drop line, dimmed source row). Details pane: load order controls,
  variants, safety findings, requirements, description, inspect, uninstall (confirm + auto snapshot),
  favorite, skip-this-update, group chip. Filters: All / Enabled / Disabled / Favorites / Updates /
  Problems; "Update all" queues every available update.
- Multi-select: Ctrl-click toggles, Shift-click selects a range from the anchor; a floating action bar
  offers Enable, Disable, Favorite, Group…, Merge…, Uninstall and Clear.
- Groups modal: per-group switch (`set_group_enabled`), on/partial/off state, select members, rename,
  delete; groups are created from the selection. A mod belongs to one group at most.
- Merge: `Engine::merge_mods` combines the selected mods' active VPK roots into one new `Merged` mod
  (`merge_vpks` with inputs ordered lowest priority first + "last wins", so the higher mod in the load
  order wins every overlapping path, and overlaps are reported). Optionally disables the originals.
- Mod issues (`issues.rs`, engine `repair.rs`, based on Grimoire's mod repairs): a read-only scan
  compares every mod's copies of game files with `citadel/pak01_dir.vpk` (on startup, after installs,
  and from the "Mod issues" toolbar button). Findings: camera attachments (root_motion), outdated
  vdata fields, missing localization strings, skeleton sampling, sound bindings. Rows get a
  "Fix available"/"Outdated" chip and the details pane a Compatibility card. Fix (per mod or Fix all)
  heals a copy: attachments, then compatibility (with animation overrides when the mod owns them),
  then verifies the entry set is unchanged and untouched entries are byte-identical, checks the game
  pak did not change meanwhile, takes an auto snapshot and swaps the root blob in one `mutate`.
  Authored values are never overwritten. Chunked roots are staged as `rNN_dir.vpk` first (Grimoire
  only handles single-file VPKs). Settings has a Credits panel naming Grimoire/vpkmerge as the basis.
- Age estimate (`age.rs`, engine `timeline.rs`, core `freshness.rs`): the installed version comes from
  `game/citadel/steam.inf` (ClientVersion, VersionDate/Time, Pacific build clock treated as UTC-8) and
  the appmanifest buildid. Update dates come from Valve's Steam news API (`GetNewsForApp`, official
  announcements only, cached 6 h in `<cache>/game_updates.json`): titles with "Gameplay Update" are
  major, other "Update"/patchnotes posts minor, the rest ignored. A built-in list marks the major
  updates Valve did not title that way (hero releases, reworks) and keeps working offline. A newer
  installed build with no announcement counts as a minor update. A GameBanana mod's installed-file
  upload date is compared with the list: no update since → current; only minor ones → "N minor
  updates since"; a major one since → "May be outdated" chip (muted; confirmed content findings win:
  "Fix available" / "Needs update"). My Mods gains an Outdated filter; the details pane shows upload
  date, game version and the verdict with a patch-notes link; Discover cards get a badge and the
  detail pane a warning. Local mods have no date and get no estimate. SteamDB is not used: it forbids
  automated access and its data comes from these Steam APIs.
- Discover: debounced GameBanana search, sort, categories, paging, cached thumbnails, detail pane with
  per-file install (flagged/archived files are never installed automatically), requirements.
  Subcategory chips (`Mod/Categories?_idCategoryRow=`; Skins has one per hero), a "Hide may be
  outdated" toggle, a screenshot strip with a full-size lightbox (`gallery.rs`), and "Add from link"
  (any input `links::parse` accepts).
- Collections (`collections.rs`): `gamebanana.com/collections/<id>` links read
  `Collection/{id}/ProfilePage` and every `Collection/{id}/Items` page (15 per page, 50-page cap),
  keep Deadlock mods with files, preselect the ones not installed, and queue each mod's only current
  file. Mods with several files are listed for picking in Discover instead of guessing.
- Heroes tab (Hero Locker, `heroes.rs`): skins grouped by hero (`Mod::primary_hero`: override, then
  GameBanana category, then detected files), one-click "use this skin" that disables the hero's other
  skins in one transaction, default look, shuffle (per hero or all, optionally on Launch Modded), and
  a prompt before stacking two skins on one hero (setting). The tab opens on a hero-select grid
  in the game's own layout (engine `roster.rs`): heroes with `m_bDisabled` false and
  `EHeroDevState_Release` from `scripts/heroes.vdata_c`, sorted by the English
  `hero_<code>_sort` name, nine per row, with each hero's card portrait (`m_strIconHeroCard`,
  uncompressed BGRA `.vtex_c` decoded by source2 `texture.rs`) cached as PNG per game build in
  `<cache>/heroes/`. Tiles show the skin count, an accent border when a skin is on, and an amber
  stack badge when several are on; stacked skins get "Keep only this" in the hero view.
- Versions (`versions.rs`): updates keep the previous `VERSION_HISTORY` (3) versions in
  `Mod.history`; their blobs stay referenced, so restoring needs no download. The details pane shows
  the GameBanana changelog (`Mod/{id}/Updates`) and marks posts newer than the installed file.
- Bisect tests the "May be outdated"/issue mods first (`Bisect::start_prioritized`): a single round
  clears or confirms them before falling back to halving.
- Customize tab (`customize.rs`, `crosshair.rs`): a crosshair designer (live preview using the game's
  layout math, presets, loading from `userdata/<account>/1422450/local/cfg/machine_convars.vcfg`) and
  a curated autoexec command list. Both write managed blocks
  (`// >>> Kombini <name>` … `// <<< Kombini <name>`) into the profile's autoexec; other lines are
  never touched. Based on Grimoire's crosshair editor and command presets (MIT), credited.
- Sound previews (`sounds.rs`, `audio.rs`, source2 `sound.rs`): `.vsnd_c` metadata is the KV3 `CTRL`
  block (`m_vSound`); the audio follows the resource at the header's declared size (MP3 as stored,
  PCM wrapped in WAV). rodio plays it on a dedicated thread. The details pane lists a mod's sounds with
  Play and "Original" (the base game's sound at the same path); the file inspector has play buttons.
- Downloads: queue with pause/resume/cancel/retry, persisted, installed automatically when complete.
- Modals: confirm, one-line prompt, profiles (switch/create/delete/share `mp1:`/import), snapshots
  (take, restore, delete, export/import `.kmmsnap`), groups, merge, conflicts, health, activity, paged
  VPK inspector (200 rows per query via `VpkIndex::query`), unmanaged import.
- Settings: launch options save on Enter/blur with an inline "Saved" state (no Save button).
- Motion (`anim.rs`, GPUI `with_animation`, ease-out quint): modal fade + rise, tab content rise,
  banners drop in, details pane rises per selected mod, staggered Discover cards and group cards,
  breathing skeletons and busy status dot, sweeping indeterminate progress, rising multi-select bar.
  Animations are keyed by the state they reveal so they replay only when it changes.
- Feedback contract: buttons show loading/disabled immediately; progress UI appears after
  150 ms; ≤ 3 toasts; errors are toasts that stay until dismissed and carry an `ErrorCode`-specific hint.
- Launch: `LaunchPhase` state machine driven by process polling (750 ms while waiting, 2 s while
  running), configurable timeout, crash/short-session banner with "Find broken mod" (bisect).
- External requests: argv and `kombini://` / `deadlockmm://` links (registered per user, opt-in) are
  forwarded to the running instance over an authenticated loopback socket; non-GameBanana URLs,
  profile codes and bundles ask for confirmation first.
- Not built (scope decision 2026-11-17): Linux/portable/platform items and fair-play warnings;
  performance presets wait on a licensing decision (the known preset values come from GPL projects).
- Logs: `<data>/logs/app.log` (previous run in `app.old.log`), panics included.

## 9. Error codes (stable)

`game_not_configured, game_not_found, game_running, gameinfo_missing_search_paths, gameinfo_read_only,
gameinfo_write_failed, invalid_vpk, no_vpk_in_archive, unsupported_archive, archive_unsafe_path,
archive_too_large, extract_failed, layout_too_many_roots, layout_conflict, transaction_failed,
mod_not_found, profile_not_found, last_profile, io, permission_denied, network, rate_limited,
hash_mismatch, cancelled, invalid_input, storage, internal`.

## 10. Settings (key → default)

`game_path: null`, `block_while_running: true`, `auto_reapply_gameinfo: true`, `auto_snapshot: true`,
`snapshot_retention: 20`, `keep_archives: true`, `max_concurrent_downloads: 2`,
`check_updates_on_startup: true`, `update_check_interval_minutes: 60`, `default_launch_options: ""`,
`url_handlers_registered: false`, `bandwidth_limit_kbps: null`, `snapshot_before_launch: false`,
`launch_timeout_seconds: 60`, `hide_nsfw: true` (Discover hides records with `_bHasContentRatings` or an
`_sInitialVisibility` other than `show`).

## 13. Edge cases from other managers

Sources: Deadlock Mod Manager (GPL-3.0; behavior studied, **no code copied**), Grimoire (MIT), community docs.

### 13.1 gameinfo.gi
1. Only unconditional lines count as mounts. A line with a platform conditional (`Game citadel/addons [$WIN64]`)
   is never treated as ours and never removed; status reports it as foreign.
2. Compound role keys (`Game+Mod  citadel`) are split on `+` for status (they satisfy `Mod citadel`), and are
   never removed or rewritten.
3. Quoted keys/values (`"Game" "citadel/addons"`) are recognized for status.
4. Other tools' fenced blocks (a comment line containing `BEGIN`/`START`/`>>>` paired with one containing
   `END`/`<<<`, other than our markers) are never edited. If a fence or a line tagged `dmm-search-path` exists,
   status sets `other_manager = Some("DMM" | "unknown")` and the UI warns; we still insert our block, and
   remove their addon mounts only through an explicit user action ("Remove other manager's mounts").
5. Our markers must appear exactly once each, start before end, both inside SearchPaths. Duplicated, missing-pair
   or out-of-order markers → `ensure` refuses, writes nothing, and reports `gameinfo_markers_corrupt` with a
   "Repair markers" action (that action removes all our marker lines and the mounts between each pair, then
   inserts a fresh block).
6. After every rewrite, re-parse the new text and verify the invariants (one block, addon mounts above
   `Game citadel`, four Mod/Write roles, Game_UILanguage/Game_LowViolence lines unchanged, brace balance unchanged).
   If verification fails, do not write.
7. Never add `Game_Language`; leave an existing one untouched (report it as obsolete info).

### 13.2 Filesystem
1. Transient lock errors (Windows OS error 32 sharing violation, 33 lock violation, `WouldBlock`) are retried up
   to 5 times with 150 ms sleeps. Error 5 (access denied) is not retried; it maps to `permission_denied` with
   advice (read-only flag, antivirus/Controlled Folder Access, OneDrive, ownership).
2. Clear the read-only attribute on managed files before moving them, and on `gameinfo.gi` before replacing it.
3. Path scope: every path the engine writes is canonicalized and must lie under `game/citadel` (shards,
   `gameinfo.gi`(+`.bak`/`.tmp`), `cfg/autoexec.cfg`), or under the app data/cache dirs. Junctions/symlinks
   escaping that scope are rejected (`permission_denied` with path).
4. Re-check "game running" immediately before phase 2 of a layout transaction (the first check is not a lock).

### 13.3 Addons watcher
Watch `game/citadel` recursively with `notify`; react only to `*.vpk` create/remove/rename directly inside
`addons` / `addons2..10`, ignoring `_kombini/`; debounce 500 ms; then `rescan()` and emit `library://changed`.
Ignore events caused by our own transaction (suppress while the writer lock is held + 500 ms).

### 13.4 Archives and VPK discovery
1. Size budget = archive size × 100 clamped to [256 MiB, 16 GiB], charged **while writing** (zip/7z); RAR is charged
   from header-declared sizes before extracting. Max 10,000 entries.
2. Normalize `\` to `/`; reject `..`, absolute, root, and drive-letter components anywhere; skip `.` components.
   After extraction, walk the output without following links and reject any symlink/junction/hardlink.
3. Encrypted archives → `extract_failed` "This archive is password-protected." Multi-volume RAR
   (`.part1.rar`, `.r00`) with missing parts → clear error naming the missing volume.
4. Nested archives: if the extracted tree has no `.vpk` but contains `.zip/.7z/.rar`, extract those one level deep.
5. VPK discovery walks the whole tree, skipping `__MACOSX/`, dotfiles, and `Thumbs.db`. Files are grouped into
   bundles **per directory** (root `X_dir.vpk` + `X_NNN.vpk` in the same directory). Single-file VPKs without
   `_dir` (e.g. `cool_skin.vpk`) are roots with no chunks.
6. Variant folders: when several roots share the same `original_name` in different directories (very common:
   `Blue/pak01_dir.vpk`, `Red/pak01_dir.vpk`), they are variants. Default selection: only the first (sorted by
   rel_path) is selected; the UI shows them as a "choose one" radio group labelled by folder. Otherwise all roots
   are selected by default.
7. Disabled names use the sanitized rel_path: `<mod_id>_<rel_path with '/' replaced by '__'>`, so variants never
   collide. Characters outside `[A-Za-z0-9._-]` become `_`.
8. `.vpk` files that fail validation are skipped with a per-file warning; if none remain → `no_vpk_in_archive`.

### 13.5 Foreign/other-manager layouts
1. Foreign `pak<digits>_dir.vpk` with value 1..=99 (including `pak1_dir.vpk`, `pak001_dir.vpk`) reserves that slot.
   `pak00_dir.vpk` and >99 are reported as "ignored by the game".
2. Other-manager signals: `.dmm.json` in any shard, subfolders inside `addons*` that contain `pak*_dir.vpk`
   (DMM per-profile folders), an `addons1` folder (Grimoire), DMM gameinfo tags/fences. Show a banner; never
   modify their files; offer the adopt/import flow.

### 13.6 Launch session classification
Signals: process seen running, run duration, new files in `game/citadel/rpt` or `*.mdmp/*.dmp` newer than launch,
and `game/citadel/console.log` (only when launched with `-condebug`) ending with engine shutdown lines.
Classes: `crash` (new dump), `short_session` (< 60 s and no clean signal), `clean` (clean log or run ≥ 60 s with
no dump), `unknown` (never seen running). The crash banner shows for `crash`/`short_session` and asks the user to
confirm before suggesting bisect.

### 13.7 Missing files
A mod with all files Missing is "missing"; some Missing is "broken". Because every mod's content is in the blob
store, both are repairable with "Restore files" (re-place blobs via `apply_layout`). If a blob itself is
missing, the mod is "orphaned": offer Reinstall (GameBanana) or Remove.

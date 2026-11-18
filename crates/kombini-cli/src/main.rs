//! `kombini` — headless command-line harness for every engine operation.
//!
//! Read-only commands (`detect`, `vpk`, `gameinfo status`, `gb`) never open
//! the library. Commands that change files require `--game` or a saved game
//! path and refuse while Deadlock runs.

use clap::{Parser, Subcommand};
use kombini_core::{AppError, ModId, ProfileId};
use kombini_engine::install::InstallMeta;
use kombini_engine::snapshots::SnapshotKind;
use kombini_engine::{AppDirs, Engine, EngineConfig, Progress, SystemProcess};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "kombini",
    version,
    about = "Kombini Deadlock mod manager (headless)"
)]
struct Cli {
    /// Deadlock install folder (default: auto-detect).
    #[arg(long, global = true)]
    game: Option<PathBuf>,
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find Deadlock installs and Steam.
    Detect,
    /// Library overview.
    Status,
    /// List installed mods in load order.
    List,
    /// Install an archive, VPK, or folder.
    Install {
        paths: Vec<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        enable: bool,
    },
    Enable {
        ids: Vec<String>,
    },
    Disable {
        ids: Vec<String>,
    },
    Uninstall {
        ids: Vec<String>,
    },
    /// Set the order of enabled mods (first wins).
    Order {
        ids: Vec<String>,
    },
    /// Re-read the addon folders.
    Rescan,
    /// Health checks.
    Health,
    /// File conflicts between enabled mods.
    Conflicts,
    #[command(subcommand)]
    Gameinfo(GameinfoCmd),
    #[command(subcommand)]
    Snapshot(SnapshotCmd),
    #[command(subcommand)]
    Profile(ProfileCmd),
    #[command(subcommand)]
    Import(ImportCmd),
    /// List or search entries in a VPK (paged).
    Vpk {
        file: PathBuf,
        #[arg(long, default_value = "")]
        filter: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Browse GameBanana.
    Gb {
        #[arg(default_value = "")]
        search: String,
        #[arg(long, default_value_t = 1)]
        page: u32,
    },
    /// Launch Deadlock through Steam.
    Launch {
        #[arg(long)]
        vanilla: bool,
    },
    /// Delete stored files no mod or snapshot references.
    Gc,
}

#[derive(Subcommand)]
enum GameinfoCmd {
    Status,
    Ensure,
    Unpatch,
    RepairMarkers,
}

#[derive(Subcommand)]
enum SnapshotCmd {
    List,
    Create { label: String },
    Restore { id: String },
    Delete { id: String },
    Export { id: String, dest: PathBuf },
    Import { file: PathBuf },
}

#[derive(Subcommand)]
enum ProfileCmd {
    List,
    Create {
        name: String,
    },
    Switch {
        id: String,
    },
    Delete {
        id: String,
    },
    /// Print an `mp1:` share code for the active profile.
    Code,
    /// Apply an `mp1:` code or `.modprofile.json`.
    Apply {
        code: String,
    },
}

#[derive(Subcommand)]
enum ImportCmd {
    /// List unmanaged addon files that can be adopted.
    Scan,
    /// Adopt every unmanaged candidate.
    Adopt,
    /// Import a mod-interchange bundle.
    Interchange { path: PathBuf },
    /// Export a mod-interchange bundle.
    Export { dest: PathBuf },
}

/// Prints stage changes and a coarse percentage to stderr.
struct ConsoleProgress {
    last: std::sync::Mutex<(String, u64)>,
}

impl Progress for ConsoleProgress {
    fn stage(&self, label: &str) {
        eprintln!("» {label}");
    }
    fn progress(&self, done: u64, total: u64) {
        if total == 0 {
            return;
        }
        let pct = done * 100 / total;
        if let Ok(mut last) = self.last.lock() {
            if pct >= last.1 + 10 || (pct == 100 && last.1 != 100) {
                last.1 = pct;
                eprintln!("  {pct}%");
            }
            last.0.clear();
        }
    }
}

fn progress() -> ConsoleProgress {
    ConsoleProgress {
        last: std::sync::Mutex::new((String::new(), 0)),
    }
}

fn out<T: Serialize>(json: bool, value: &T, text: impl FnOnce(&T) -> String) {
    if json {
        match serde_json::to_string_pretty(value) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("error: {e}"),
        }
    } else {
        println!("{}", text(value));
    }
}

fn game_root(cli: &Cli) -> Result<PathBuf, AppError> {
    if let Some(g) = &cli.game {
        return Ok(g.clone());
    }
    kombini_steam::find_deadlock_installs()
        .into_iter()
        .next()
        .map(|i| i.root)
        .ok_or_else(|| {
            AppError::new(
                kombini_core::ErrorCode::GameNotConfigured,
                "Deadlock was not found; pass --game <folder>",
            )
        })
}

fn open(cli: &Cli) -> Result<Engine, AppError> {
    let (engine, report) = Engine::open(EngineConfig {
        dirs: AppDirs::from_env()?,
        game_root: game_root(cli)?,
        process: Arc::new(SystemProcess::new()),
    })?;
    for line in &report.recovered {
        eprintln!("note: {line}");
    }
    if report.gameinfo_reapplied {
        eprintln!("note: gameinfo.gi was reapplied (game update)");
    }
    Ok(engine)
}

fn ids(list: &[String]) -> Vec<ModId> {
    list.iter().map(ModId::new).collect()
}

fn run(cli: &Cli) -> Result<(), AppError> {
    let json = cli.json;
    match &cli.command {
        Command::Detect => {
            let installs = kombini_steam::find_deadlock_installs();
            let rows: Vec<serde_json::Value> = installs
                .iter()
                .map(|i| {
                    let b = kombini_steam::read_build_info(&i.root);
                    serde_json::json!({ "root": i.root, "library": i.library, "build": b.build_id, "fingerprint": b.fingerprint })
                })
                .collect();
            out(json, &rows, |r| {
                if r.is_empty() {
                    "No Deadlock install found".into()
                } else {
                    r.iter()
                        .map(|v| {
                            format!(
                                "{}  (build {})",
                                v["root"].as_str().unwrap_or("?"),
                                v["build"].as_str().unwrap_or("unknown")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            });
        }
        Command::Vpk {
            file,
            filter,
            offset,
            limit,
        } => {
            let archive = kombini_vpk::VpkArchive::open(file)
                .map_err(|e| AppError::new(kombini_core::ErrorCode::InvalidVpk, e.to_string()))?;
            let index = kombini_vpk::VpkIndex::new(archive);
            let page = index.query(&kombini_vpk::EntryQuery {
                filter: filter.clone(),
                offset: *offset,
                limit: *limit,
                sort: kombini_vpk::SortKey::Path,
                descending: false,
            });
            out(json, &page, |p| {
                let mut s: Vec<String> = p
                    .rows
                    .iter()
                    .map(|r| format!("{:>10}  {}", r.size, r.path))
                    .collect();
                s.push(format!("— {} of {} entries", p.rows.len(), p.total));
                s.join("\n")
            });
        }
        Command::Gb { search, page } => {
            let client = kombini_gamebanana::Client::new();
            let result = client
                .browse(&kombini_gamebanana::BrowseQuery {
                    page: *page,
                    per_page: 20,
                    search: search.clone(),
                    ..Default::default()
                })
                .map_err(AppError::from)?;
            out(json, &*result, |p| {
                p.records
                    .iter()
                    .map(|m| format!("{:>8}  {}  ({} likes)", m.id, m.name, m.likes))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Gameinfo(GameinfoCmd::Status) => {
            let root = kombini_steam::validate_game_root(&game_root(cli)?)
                .map_err(|e| AppError::new(kombini_core::ErrorCode::GameNotFound, e.to_string()))?;
            let status = kombini_gameinfo::GameInfoFile::for_game_root(&root).status();
            out(json, &status, |s| {
                let mut lines = vec![format!("{}  patched: {}", s.path.display(), s.patched)];
                lines.extend(s.issues.iter().map(|i| format!("  ! {}", i.message)));
                lines.join("\n")
            });
        }
        _ => run_engine(cli)?,
    }
    Ok(())
}

fn run_engine(cli: &Cli) -> Result<(), AppError> {
    let json = cli.json;
    let e = open(cli)?;
    let p = progress();
    let change = |r: kombini_engine::ChangeReport| {
        out(json, &r, |r| {
            let mut s = vec![r.summary.clone()];
            s.extend(r.warnings.iter().map(|w| format!("  ! {w}")));
            s.join("\n")
        })
    };
    match &cli.command {
        Command::Status => {
            let state = e.state();
            let v = serde_json::json!({
                "game": e.paths().root,
                "mods": state.mods.len(),
                "enabled": e.load_order().len(),
                "profile": state.profile().name,
                "foreign": e.foreign_files().len(),
                "other_managers": e.other_managers(),
            });
            out(json, &v, |v| {
                format!(
                    "{}\n{} mods, {} enabled, profile \"{}\", {} unmanaged files",
                    v["game"].as_str().unwrap_or(""),
                    v["mods"],
                    v["enabled"],
                    v["profile"].as_str().unwrap_or(""),
                    v["foreign"]
                )
            });
        }
        Command::List => {
            let order = e.load_order();
            let state = e.state();
            let mut rows: Vec<serde_json::Value> = Vec::new();
            for (i, id) in order.iter().enumerate() {
                if let Some(m) = state.mods.get(id) {
                    rows.push(serde_json::json!({"position": i + 1, "id": id, "title": m.title, "enabled": true, "slots": m.mounted_slots()}));
                }
            }
            for m in state.mods.values().filter(|m| !order.contains(&m.id)) {
                rows.push(serde_json::json!({"position": null, "id": m.id, "title": m.title, "enabled": false}));
            }
            out(json, &rows, |rows| {
                rows.iter()
                    .map(|r| {
                        format!(
                            "{:>4}  {:<24} {}",
                            r["position"]
                                .as_u64()
                                .map(|p| p.to_string())
                                .unwrap_or_else(|| "-".into()),
                            r["id"].as_str().unwrap_or(""),
                            r["title"].as_str().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Install {
            paths,
            title,
            enable,
        } => {
            let r = e.install_paths(
                paths,
                InstallMeta {
                    title: title.clone(),
                    enable: enable.then_some(true),
                    ..InstallMeta::default()
                },
                &p,
            )?;
            out(json, &r, |r| {
                let mut s = vec![format!("{} → {} ({} file(s))", r.title, r.mod_id, r.files)];
                s.extend(r.warnings.iter().map(|w| format!("  ! {w}")));
                s.extend(r.safety.iter().map(|f| format!("  ! {}", f.message)));
                s.join("\n")
            });
        }
        Command::Enable { ids: list } => change(e.set_enabled(&ids(list), true)?),
        Command::Disable { ids: list } => change(e.set_enabled(&ids(list), false)?),
        Command::Uninstall { ids: list } => change(e.uninstall(&ids(list))?),
        Command::Order { ids: list } => change(e.reorder_enabled(&ids(list))?),
        Command::Rescan => {
            let r = e.rescan()?;
            out(json, &r, |r| {
                format!(
                    "{} missing, {} relocated, {} unmanaged",
                    r.missing_files, r.relocated, r.foreign_files
                )
            });
        }
        Command::Health => {
            let r = e.health();
            out(json, &r, |r| {
                if r.issues.is_empty() {
                    "Everything looks good".into()
                } else {
                    r.issues
                        .iter()
                        .map(|i| format!("[{:?}] {}", i.level, i.message))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            });
        }
        Command::Conflicts => {
            let r = e.conflicts();
            out(json, &r, |r| {
                r.pairs
                    .iter()
                    .map(|p| format!("{} overrides {} ({} files)", p.winner, p.loser, p.paths))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Gameinfo(cmd) => {
            let r = match cmd {
                GameinfoCmd::Ensure => e.ensure_gameinfo()?,
                GameinfoCmd::Unpatch => e.unpatch_gameinfo()?,
                GameinfoCmd::RepairMarkers => e.repair_gameinfo_markers()?,
                GameinfoCmd::Status => return Ok(()),
            };
            out(json, &r, |r| format!("changed: {}", r.changed));
        }
        Command::Snapshot(cmd) => match cmd {
            SnapshotCmd::List => {
                let list = e.list_snapshots()?;
                out(json, &list, |l| {
                    l.iter()
                        .map(|s| format!("{}  [{}] {} — {} mods", s.id, s.kind, s.label, s.mods))
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            }
            SnapshotCmd::Create { label } => {
                let s = e.create_snapshot(label, SnapshotKind::Manual)?;
                out(json, &s, |s| format!("Created {}", s.id));
            }
            SnapshotCmd::Restore { id } => {
                let r = e.restore_snapshot(id, &p)?;
                out(json, &r, |r| r.change.summary.clone());
            }
            SnapshotCmd::Delete { id } => {
                e.delete_snapshot(id)?;
                out(json, &id, |id| format!("Deleted {id}"));
            }
            SnapshotCmd::Export { id, dest } => {
                e.export_snapshot(id, dest, &p)?;
                out(json, &dest, |d| format!("Wrote {}", d.display()));
            }
            SnapshotCmd::Import { file } => {
                let s = e.import_snapshot(file, &p)?;
                out(json, &s, |s| format!("Imported as {}", s.id));
            }
        },
        Command::Profile(cmd) => match cmd {
            ProfileCmd::List => {
                let state = e.state();
                let rows: Vec<serde_json::Value> = state
                    .profiles
                    .iter()
                    .map(|p| serde_json::json!({"id": p.id, "name": p.name, "active": p.id == state.active_profile, "enabled": p.enabled.len()}))
                    .collect();
                out(json, &rows, |rows| {
                    rows.iter()
                        .map(|r| {
                            format!(
                                "{} {:<12} {}",
                                if r["active"].as_bool().unwrap_or(false) {
                                    "*"
                                } else {
                                    " "
                                },
                                r["id"].as_str().unwrap_or(""),
                                r["name"].as_str().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            }
            ProfileCmd::Create { name } => {
                let id = e.create_profile(name, None)?;
                out(json, &id, |id| format!("Created {id}"));
            }
            ProfileCmd::Switch { id } => change(e.activate_profile(&ProfileId::new(id), &p)?),
            ProfileCmd::Delete { id } => change(e.delete_profile(&ProfileId::new(id))?),
            ProfileCmd::Code => {
                let (doc, warnings) = kombini_import::modprofile::export(&e, None)?;
                for w in warnings {
                    eprintln!("note: {w}");
                }
                let code = kombini_import::modprofile::encode(&doc)?;
                out(json, &code, Clone::clone);
            }
            ProfileCmd::Apply { code } => {
                let text = if std::path::Path::new(code).is_file() {
                    std::fs::read_to_string(code).map_err(|err| {
                        AppError::from_io("Reading the profile", std::path::Path::new(code), &err)
                    })?
                } else {
                    code.clone()
                };
                let doc = kombini_import::modprofile::decode(&text)?;
                let r = kombini_import::modprofile::apply(&e, &doc, true, &p)?;
                out(json, &r, |r| {
                    let mut s = vec![format!(
                        "Profile \"{}\": {} enabled",
                        r.name,
                        r.enabled.len()
                    )];
                    s.extend(r.missing.iter().map(|m| {
                        format!(
                            "  missing: {} (gamebanana.com/mods/{})",
                            m.name.as_deref().unwrap_or("?"),
                            m.submission_id
                        )
                    }));
                    s.join("\n")
                });
            }
        },
        Command::Import(cmd) => match cmd {
            ImportCmd::Scan => {
                let mut c = kombini_import::unmanaged::scan(&e);
                let notes = kombini_import::unmanaged::enrich_from_dmm(&e.paths().citadel, &mut c);
                for n in notes {
                    eprintln!("note: {n}");
                }
                out(json, &c, |c| {
                    c.iter()
                        .map(|c| {
                            format!(
                                "{:<10} {} ({} file(s))",
                                if c.enabled { "mounted" } else { "parked" },
                                c.title,
                                c.files.len()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            }
            ImportCmd::Adopt => {
                let mut c = kombini_import::unmanaged::scan(&e);
                kombini_import::unmanaged::enrich_from_dmm(&e.paths().citadel, &mut c);
                let r = kombini_import::unmanaged::adopt(&e, &c, &p)?;
                out(json, &r, |r| {
                    let mut s = vec![format!("Adopted {} mod(s)", r.adopted.len())];
                    s.extend(r.failed.iter().map(|(t, err)| format!("  ! {t}: {err}")));
                    s.join("\n")
                });
            }
            ImportCmd::Interchange { path } => {
                let r = kombini_import::interchange::import(&e, path, None, &p)?;
                out(json, &r, |r| {
                    format!(
                        "Imported {} mod(s), skipped {}",
                        r.imported.len(),
                        r.skipped.len()
                    )
                });
            }
            ImportCmd::Export { dest } => {
                let m = kombini_import::interchange::export(&e, dest, true, &p)?;
                out(json, &m, |m| format!("Wrote {}", m.display()));
            }
        },
        Command::Launch { vanilla } => {
            let mode = if *vanilla {
                kombini_engine::launch::LaunchMode::Vanilla
            } else {
                kombini_engine::launch::LaunchMode::Modded
            };
            let t = e.launch(mode)?;
            out(json, &t, |t| {
                format!("Asked Steam to start Deadlock ({})", t.method)
            });
        }
        Command::Gc => {
            let r = e.gc_blobs()?;
            out(json, &r, |r| format!("{r:?}"));
        }
        Command::Detect | Command::Vpk { .. } | Command::Gb { .. } => {}
    }
    Ok(())
}

fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("KOMBINI_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .try_init();
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if cli.json {
                eprintln!("{}", serde_json::to_string(&e).unwrap_or_default());
            } else {
                eprintln!("error [{}]: {e}", e.code);
            }
            ExitCode::FAILURE
        }
    }
}

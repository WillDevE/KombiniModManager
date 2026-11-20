//! Launching Deadlock through Steam and classifying play sessions (§13.6).
//!
//! The UI drives [`LaunchPhase`] with [`LaunchPhase::next`]; the engine does
//! the preparation (gameinfo, snapshot, vanilla unpatch) and bookkeeping.

use crate::snapshots::SnapshotKind;
use crate::{Engine, Result};
use kombini_core::{AppError, ErrorCode, ModId, effective_order};
use kombini_storage::LaunchRow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const VANILLA_PENDING_KEY: &str = "vanilla_pending_reapply";
/// Whether mods are switched on for the game (the title-bar toggle). Off
/// keeps every mod installed but takes Kombini's block out of gameinfo.gi,
/// so the game starts vanilla until it is switched back on.
pub const MODS_ACTIVE_KEY: &str = "mods_active";
/// Sessions shorter than this are reported as "short" (possible crash on load).
pub const SHORT_SESSION_SECS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    Modded,
    Vanilla,
}

/// The launch state machine shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPhase {
    Idle,
    Preparing,
    Launching,
    WaitingForProcess,
    Running,
    Exited,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchEvent {
    Start,
    Prepared,
    SteamStarted,
    ProcessSeen,
    ProcessGone,
    Timeout,
    Error,
    Reset,
}

impl LaunchPhase {
    /// Pure transition function. Invalid events leave the phase unchanged.
    pub fn next(self, event: LaunchEvent) -> Self {
        use LaunchEvent as E;
        use LaunchPhase as P;
        match (self, event) {
            (P::Idle | P::Exited | P::Failed | P::TimedOut, E::Start) => P::Preparing,
            (P::Preparing, E::Prepared) => P::Launching,
            (P::Launching, E::SteamStarted) => P::WaitingForProcess,
            (P::WaitingForProcess, E::ProcessSeen) => P::Running,
            (P::WaitingForProcess, E::Timeout) => P::TimedOut,
            (P::Running, E::ProcessGone) => P::Exited,
            (P::Preparing | P::Launching | P::WaitingForProcess, E::Error) => P::Failed,
            (_, E::Reset) => P::Idle,
            (phase, _) => phase,
        }
    }

    /// True while a launch is in flight (buttons disabled).
    pub fn busy(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Launching | Self::WaitingForProcess | Self::Running
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    Crash,
    ShortSession,
    Clean,
    /// The game process was never seen.
    Unknown,
}

pub fn classify(saw_process: bool, duration_secs: i64, crash_dump: bool) -> SessionOutcome {
    if !saw_process {
        SessionOutcome::Unknown
    } else if crash_dump {
        SessionOutcome::Crash
    } else if duration_secs < SHORT_SESSION_SECS {
        SessionOutcome::ShortSession
    } else {
        SessionOutcome::Clean
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchTicket {
    pub id: String,
    pub mode: LaunchMode,
    pub started_at: i64,
    pub options: String,
    pub method: String,
    pub enabled_mods: Vec<ModId>,
    pub game_build: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRecord {
    #[serde(flatten)]
    pub ticket: LaunchTicket,
    pub ended_at: Option<i64>,
    pub duration_secs: Option<i64>,
    pub outcome: Option<SessionOutcome>,
}

/// True if a `.mdmp` crash dump newer than `since` exists in the usual places.
pub fn crash_dump_since(root: &Path, since: SystemTime) -> bool {
    let dirs = [
        root.join("game").join("citadel"),
        root.join("game").join("bin").join("win64"),
        root.join("game").join("citadel").join("dumps"),
        root.to_path_buf(),
    ];
    dirs.iter().any(|dir| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                name.ends_with(".mdmp")
                    && e.metadata()
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t >= since)
            })
        })
    })
}

impl Engine {
    /// Launch options: profile override or app default, plus autoexec flags
    /// when the profile manages an autoexec.
    pub fn launch_options(&self, mode: LaunchMode) -> String {
        let (profile_opts, has_autoexec) = self.with_state(|s| {
            (
                s.profile().launch_options.clone(),
                s.profile().autoexec.is_some(),
            )
        });
        let base = profile_opts.unwrap_or_else(|| self.settings().default_launch_options);
        if mode == LaunchMode::Modded && has_autoexec && !kombini_steam::has_autoexec_options(&base)
        {
            kombini_steam::merge_launch_options(&base, "-console +exec autoexec")
        } else {
            base
        }
    }

    /// Prepares the game folder and asks Steam to start Deadlock.
    pub fn launch(&self, mode: LaunchMode) -> Result<LaunchTicket> {
        if self.process.is_running() {
            return Err(AppError::new(
                ErrorCode::GameRunning,
                "Deadlock is already running",
            ));
        }
        let build = kombini_steam::read_build_info(&self.paths.root);
        if build.update_in_progress {
            return Err(AppError::new(
                ErrorCode::GameRunning,
                "Steam is updating Deadlock. Wait for the update to finish, then launch again.",
            ));
        }
        let enabled_mods = self.with_state(|s| {
            let existing: BTreeSet<ModId> = s.mods.keys().cloned().collect();
            effective_order(s.profile(), &existing)
        });
        match mode {
            LaunchMode::Modded => {
                if self.settings().snapshot_before_launch
                    && let Err(e) = self.create_snapshot("Before launch", SnapshotKind::Auto)
                {
                    tracing::warn!(error = %e, "pre-launch snapshot failed");
                }
                let mounted = self.with_state(|s| {
                    s.mods
                        .values()
                        .any(|m| m.files.iter().any(|f| f.placement.is_mounted()))
                }) || self.foreign.read().iter().any(|f| f.slot.is_some());
                if mounted {
                    self.ensure_gameinfo()?;
                }
            }
            LaunchMode::Vanilla if !self.mods_active() => {
                // Mods are switched off: gameinfo is already vanilla.
                self.unpatch_gameinfo()?;
            }
            LaunchMode::Vanilla => {
                self.db.lock().set_setting(VANILLA_PENDING_KEY, &true)?;
                if let Err(e) = self.unpatch_gameinfo() {
                    let _ = self.db.lock().set_setting(VANILLA_PENDING_KEY, &false);
                    return Err(e);
                }
            }
        }
        let options = self.launch_options(mode);
        let steam_dir = kombini_steam::steam_dir_for(&self.paths.root);
        let method = kombini_steam::launch(steam_dir.as_deref(), &options).map_err(|e| {
            if mode == LaunchMode::Vanilla {
                let _ = self.restore_after_vanilla();
            }
            AppError::new(ErrorCode::Internal, format!("Could not start Steam: {e}"))
        })?;
        let ticket = LaunchTicket {
            id: format!(
                "l{}{}",
                kombini_storage::now(),
                kombini_storage::random_hex(2)
            ),
            mode,
            started_at: kombini_storage::now(),
            options,
            method: format!("{method:?}"),
            enabled_mods: if mode == LaunchMode::Modded {
                enabled_mods
            } else {
                Vec::new()
            },
            game_build: build.fingerprint,
        };
        self.save_launch_record(&LaunchRecord {
            ticket: ticket.clone(),
            ended_at: None,
            duration_secs: None,
            outcome: None,
        });
        self.log(
            "launch",
            match mode {
                LaunchMode::Modded => "Launched Deadlock with mods",
                LaunchMode::Vanilla => "Launched Deadlock without mods",
            },
            None,
        );
        Ok(ticket)
    }

    /// True unless the user switched mods off.
    pub fn mods_active(&self) -> bool {
        self.db
            .lock()
            .get_setting::<bool>(MODS_ACTIVE_KEY)
            .ok()
            .flatten()
            .unwrap_or(true)
    }

    /// Switches mods on or off for the game. Installed and enabled mods stay
    /// as they are; only gameinfo.gi changes, and it stays that way after
    /// Kombini closes.
    pub fn set_mods_active(&self, active: bool) -> Result<()> {
        if self.process.is_running() {
            return Err(AppError::new(
                ErrorCode::GameRunning,
                "Close Deadlock before switching mods on or off",
            ));
        }
        self.db.lock().set_setting(MODS_ACTIVE_KEY, &active)?;
        let applied = if active {
            self.ensure_gameinfo().map(|_| ())
        } else {
            self.unpatch_gameinfo().map(|_| ())
        };
        if let Err(e) = applied {
            let _ = self.db.lock().set_setting(MODS_ACTIVE_KEY, &!active);
            return Err(e);
        }
        self.log(
            "mods",
            if active {
                "Switched mods on"
            } else {
                "Switched mods off"
            },
            None,
        );
        Ok(())
    }

    /// Records the end of a session and undoes the vanilla unpatch.
    pub fn finish_launch(
        &self,
        ticket: &LaunchTicket,
        saw_process: bool,
        running_secs: i64,
    ) -> Result<LaunchRecord> {
        let since = SystemTime::UNIX_EPOCH
            + Duration::from_secs(u64::try_from(ticket.started_at).unwrap_or(0));
        let outcome = classify(
            saw_process,
            running_secs,
            saw_process && crash_dump_since(&self.paths.root, since),
        );
        let record = LaunchRecord {
            ticket: ticket.clone(),
            ended_at: Some(kombini_storage::now()),
            duration_secs: saw_process.then_some(running_secs),
            outcome: Some(outcome),
        };
        self.save_launch_record(&record);
        if ticket.mode == LaunchMode::Vanilla {
            self.restore_after_vanilla()?;
        }
        Ok(record)
    }

    fn restore_after_vanilla(&self) -> Result<()> {
        self.db.lock().set_setting(VANILLA_PENDING_KEY, &false)?;
        self.ensure_gameinfo().map(|_| ())
    }

    fn save_launch_record(&self, record: &LaunchRecord) {
        let row = LaunchRow {
            id: record.ticket.id.clone(),
            started_at: record.ticket.started_at,
            doc: serde_json::to_value(record).unwrap_or_default(),
        };
        if let Err(e) = self.db.lock().save_launch(self.game_id.as_str(), &row) {
            tracing::warn!(error = %e, "could not record the launch");
        }
    }

    pub fn recent_launches(&self, limit: usize) -> Result<Vec<LaunchRecord>> {
        let rows = self
            .db
            .lock()
            .recent_launches(self.game_id.as_str(), limit)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| serde_json::from_value(r.doc).ok())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine() {
        use LaunchEvent as E;
        use LaunchPhase as P;
        let mut p = P::Idle;
        for (e, want) in [
            (E::Start, P::Preparing),
            (E::Prepared, P::Launching),
            (E::SteamStarted, P::WaitingForProcess),
            (E::ProcessSeen, P::Running),
            (E::Start, P::Running),
            (E::ProcessGone, P::Exited),
            (E::Start, P::Preparing),
            (E::Error, P::Failed),
            (E::Reset, P::Idle),
        ] {
            p = p.next(e);
            assert_eq!(p, want, "after {e:?}");
        }
        assert_eq!(P::WaitingForProcess.next(E::Timeout), P::TimedOut);
        assert!(P::Running.busy() && !P::Exited.busy());
    }

    #[test]
    fn classification() {
        assert_eq!(classify(false, 0, false), SessionOutcome::Unknown);
        assert_eq!(classify(true, 10, true), SessionOutcome::Crash);
        assert_eq!(classify(true, 10, false), SessionOutcome::ShortSession);
        assert_eq!(classify(true, 600, false), SessionOutcome::Clean);
    }
}

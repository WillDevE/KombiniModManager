//! RAR extraction: the built-in UnRAR (the `unrar` crate) first, the system `tar.exe`/`bsdtar`
//! (libarchive) as a silent fallback.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use unrar::Archive;
use unrar::error::Code;
use walkdir::WalkDir;

use crate::limits::{Ctx, Limits};
use crate::safety::{
    clear_dir, create_parent_dirs, ensure_no_link_ancestors, io_err, is_root_entry, join_rel,
    post_validate, sanitize_entry,
};
use crate::{ArchiveError, Backend};

/// Tiny RAR archive used by [`probe_builtin`].
static PROBE_RAR: &[u8] = include_bytes!("../tests/fixtures/probe.rar");

/// `CREATE_NO_WINDOW`, so helper processes never flash a console window in a GUI app.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How often the tar fallback checks for cancellation and for exceeding the size limit.
const TAR_POLL: Duration = Duration::from_millis(25);
const TAR_SIZE_CHECK: Duration = Duration::from_millis(500);

fn map_unrar(err: unrar::error::UnrarError) -> ArchiveError {
    match err.code {
        Code::MissingPassword | Code::BadPassword => ArchiveError::Encrypted,
        _ => ArchiveError::Failed {
            backend: Backend::UnrarBuiltin,
            message: format!("{err:?}"),
        },
    }
}

/// Errors after which retrying with another backend cannot help or would be unsafe.
fn is_final(err: &ArchiveError) -> bool {
    !matches!(
        err,
        ArchiveError::Failed { .. } | ArchiveError::Io { .. } | ArchiveError::Unsupported(_)
    )
}

fn error_message(err: &ArchiveError) -> String {
    match err {
        ArchiveError::Failed { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Multi-volume detection
// ---------------------------------------------------------------------------------------------

/// How the volumes of a multi-volume set are named, derived from the opened file's name.
enum VolumeNaming {
    /// `<prefix><N><suffix>` where the number is zero-padded to `width` (`x.part01.rar`).
    Part {
        prefix: String,
        number: u64,
        width: usize,
        suffix: String,
    },
    /// `<stem>.rNN`: the first volume is `<stem>.rar`, then `.r00`, `.r01`, ...
    Old {
        stem: String,
        number: u64,
    },
    /// `<stem>.rar` without a part number.
    Plain {
        stem: String,
    },
    Other,
}

fn parse_naming(file_name: &str) -> VolumeNaming {
    let lower = file_name.to_ascii_lowercase();
    if let Some(base_lower) = lower.strip_suffix(".rar") {
        let base = &file_name[..base_lower.len()];
        if let Some(pos) = base_lower.rfind(".part") {
            let digits = &base_lower[pos + 5..];
            if !digits.is_empty()
                && digits.len() <= 9
                && digits.bytes().all(|b| b.is_ascii_digit())
                && let Ok(number) = digits.parse::<u64>()
                && number >= 1
            {
                return VolumeNaming::Part {
                    prefix: base[..pos + 5].to_string(),
                    number,
                    width: digits.len(),
                    suffix: file_name[base.len()..].to_string(),
                };
            }
        }
        return VolumeNaming::Plain {
            stem: base.to_string(),
        };
    }
    let bytes = lower.as_bytes();
    if bytes.len() > 4 {
        let tail = &lower[lower.len() - 4..];
        if tail.starts_with(".r")
            && tail[2..].bytes().all(|b| b.is_ascii_digit())
            && let Ok(number) = tail[2..].parse::<u64>()
        {
            return VolumeNaming::Old {
                stem: file_name[..file_name.len() - 4].to_string(),
                number,
            };
        }
    }
    VolumeNaming::Other
}

/// Reads the volume flags from the archive header: `(is_volume, continues_in_next_volume)`.
/// Returns `None` when the archive cannot be opened here (the extractor reports that error).
fn header_volume_info(path: &Path) -> Option<(bool, bool)> {
    let mut list = Archive::new(path).open_for_listing_split().ok()?;
    let is_volume = list.volume_info() != unrar::VolumeInfo::None;
    let mut continues = false;
    for entry in &mut list {
        match entry {
            Ok(header) => continues |= header.is_split_after(),
            Err(_) => break,
        }
    }
    Some((is_volume, continues))
}

/// Verifies that the other volumes of a multi-volume RAR set exist next to `archive`.
/// The first missing volume is reported through `MissingVolume`.
fn check_volumes(archive: &Path) -> Result<(), ArchiveError> {
    let Some(file_name) = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
    else {
        return Ok(());
    };
    let naming = parse_naming(&file_name);
    let missing = |name: &str| -> bool { !archive.with_file_name(name).exists() };
    let err = |name: String| Err(ArchiveError::MissingVolume(name));

    let (is_volume, continues) = header_volume_info(archive).unwrap_or((false, false));
    match naming {
        VolumeNaming::Part {
            prefix,
            number,
            width,
            suffix,
        } => {
            let name = |k: u64| format!("{prefix}{k:0width$}{suffix}");
            for k in 1..number {
                if missing(&name(k)) {
                    return err(name(k));
                }
            }
            if continues && missing(&name(number + 1)) {
                return err(name(number + 1));
            }
        }
        VolumeNaming::Old { stem, number } => {
            let first = format!("{stem}.rar");
            if missing(&first) {
                return err(first);
            }
            for k in 0..number {
                let name = format!("{stem}.r{k:02}");
                if missing(&name) {
                    return err(name);
                }
            }
            if continues {
                let next = format!("{stem}.r{:02}", number + 1);
                if missing(&next) {
                    return err(next);
                }
            }
        }
        VolumeNaming::Plain { stem } => {
            if continues {
                let next = format!("{stem}.r00");
                if missing(&next) {
                    return err(next);
                }
            } else if is_volume {
                return err("the first volume of this archive".to_string());
            }
        }
        VolumeNaming::Other => {
            if is_volume {
                return err("the other volumes of this archive".to_string());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Built-in UnRAR
// ---------------------------------------------------------------------------------------------

struct PlannedEntry {
    rel: String,
    is_dir: bool,
}

/// Lists, validates and extracts with the built-in UnRAR. Sizes are enforced from the declared
/// (header) sizes before anything is extracted; the decoder never produces more than declared.
fn extract_builtin(
    archive_path: &Path,
    dest: &Path,
    ctx: &mut Ctx<'_>,
) -> Result<(), ArchiveError> {
    let list = Archive::new(archive_path)
        .open_for_listing()
        .map_err(map_unrar)?;
    if list.has_encrypted_headers() {
        return Err(ArchiveError::Encrypted);
    }
    let mut plan: Vec<PlannedEntry> = Vec::new();
    let mut declared = 0u64;
    for item in list {
        let header = item.map_err(map_unrar)?;
        let raw = header.filename.to_string_lossy().into_owned();
        if header.is_directory() && is_root_entry(&raw) {
            continue;
        }
        let rel = sanitize_entry(&raw).ok_or_else(|| ArchiveError::UnsafePath(raw.clone()))?;
        if header.is_encrypted() {
            return Err(ArchiveError::Encrypted);
        }
        if plan.len() >= ctx.limits.max_entries {
            return Err(ArchiveError::TooManyEntries {
                limit: ctx.limits.max_entries,
            });
        }
        declared = declared.saturating_add(header.unpacked_size);
        if declared > ctx.limits.max_bytes {
            return Err(ArchiveError::TooLarge {
                limit: ctx.limits.max_bytes,
            });
        }
        plan.push(PlannedEntry {
            rel,
            is_dir: header.is_directory(),
        });
    }
    if declared > 0 {
        ctx.set_total(declared);
    }

    let mut open = Archive::new(archive_path)
        .open_for_processing()
        .map_err(map_unrar)?;
    let mut processed = 0usize;
    while let Some(header) = open.read_header().map_err(map_unrar)? {
        ctx.check_cancel()?;
        let raw = header.entry().filename.to_string_lossy().into_owned();
        let is_dir = header.entry().is_directory();
        let size = header.entry().unpacked_size;
        if header.entry().is_encrypted() {
            return Err(ArchiveError::Encrypted);
        }
        if is_dir && is_root_entry(&raw) {
            open = header.skip().map_err(map_unrar)?;
            continue;
        }
        let rel = sanitize_entry(&raw).ok_or_else(|| ArchiveError::UnsafePath(raw.clone()))?;
        // The process pass must see exactly what the validated listing described.
        match plan.get(processed) {
            Some(planned) if planned.rel == rel && planned.is_dir == is_dir => {}
            _ => {
                return Err(ArchiveError::Failed {
                    backend: Backend::UnrarBuiltin,
                    message: format!(
                        "archive entries changed between listing and extraction: {raw}"
                    ),
                });
            }
        }
        processed += 1;
        // Never let the extractor write through a link it (or an earlier entry) created.
        ensure_no_link_ancestors(dest, &rel)?;
        let out = join_rel(dest, &rel);
        if is_dir {
            fs::create_dir_all(&out).map_err(|e| io_err(&out, e))?;
            open = header.skip().map_err(map_unrar)?;
        } else {
            create_parent_dirs(&out)?;
            open = header.extract_to(&out).map_err(map_unrar)?;
            ctx.charge(size)?;
        }
    }
    ctx.report();
    Ok(())
}

/// Extracts a RAR archive. Tries the built-in UnRAR first and silently retries with the system
/// tar when that fails for a reason other than encryption, a missing volume, a limit or an
/// unsafe archive. Returns the backend used and any warnings.
pub(crate) fn extract_rar(
    archive: &Path,
    dest: &Path,
    ctx: &mut Ctx<'_>,
) -> Result<(Backend, Vec<String>), ArchiveError> {
    check_volumes(archive)?;
    let err = match extract_builtin(archive, dest, ctx) {
        Ok(()) => return Ok((Backend::UnrarBuiltin, Vec::new())),
        Err(err) if is_final(&err) => return Err(err),
        Err(err) => err,
    };
    let unrar_message = error_message(&err);
    tracing::warn!(
        archive = %archive.display(),
        error = %unrar_message,
        "built-in RAR extraction failed; retrying with the system tar"
    );
    match extract_rar_with_tar(archive, dest, ctx.limits, ctx.cancel_fn()) {
        Ok(()) => Ok((
            Backend::SystemTar,
            vec![format!(
                "Built-in RAR extraction failed ({unrar_message}); used Windows tar.exe instead."
            )],
        )),
        Err(ArchiveError::Failed { message, .. }) => {
            tracing::warn!(
                archive = %archive.display(),
                error = %message,
                "system tar fallback for RAR failed as well"
            );
            Err(ArchiveError::Failed {
                backend: Backend::UnrarBuiltin,
                message: format!("{unrar_message}; fallback tar.exe: {message}"),
            })
        }
        Err(other) => Err(other),
    }
}

// ---------------------------------------------------------------------------------------------
// System tar fallback
// ---------------------------------------------------------------------------------------------

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Locates a working system tar and returns its path and the first line of `--version`.
/// Windows: `%SystemRoot%\System32\tar.exe`; other systems: `bsdtar` on `PATH`.
pub(crate) fn system_tar() -> Option<(PathBuf, String)> {
    #[cfg(windows)]
    let path = {
        let root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir"))?;
        let path = PathBuf::from(root).join("System32").join("tar.exe");
        if !path.is_file() {
            return None;
        }
        path
    };
    #[cfg(not(windows))]
    let path = PathBuf::from("bsdtar");

    let mut cmd = Command::new(&path);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    Some((path, version))
}

/// Sum of the sizes of all regular files below `dir`, stopping early once `stop_above` is passed.
fn dir_size_capped(dir: &Path, stop_above: u64) -> u64 {
    let mut total = 0u64;
    for entry in WalkDir::new(dir).follow_links(false).into_iter().flatten() {
        if entry.file_type().is_file()
            && let Ok(meta) = entry.metadata()
        {
            total = total.saturating_add(meta.len());
            if total > stop_above {
                break;
            }
        }
    }
    total
}

fn kill_and_wait(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for `child`, killing it when the caller cancels or the output grows beyond the limit.
fn wait_for_tar(
    child: &mut Child,
    dest: &Path,
    limits: Limits,
    cancel: &dyn Fn() -> bool,
) -> Result<ExitStatus, ArchiveError> {
    let mut last_size_check = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(e) => {
                kill_and_wait(child);
                return Err(ArchiveError::Failed {
                    backend: Backend::SystemTar,
                    message: format!("could not wait for tar: {e}"),
                });
            }
        }
        if cancel() {
            kill_and_wait(child);
            return Err(ArchiveError::Cancelled);
        }
        if last_size_check.elapsed() >= TAR_SIZE_CHECK {
            last_size_check = Instant::now();
            if dir_size_capped(dest, limits.max_bytes) > limits.max_bytes {
                kill_and_wait(child);
                return Err(ArchiveError::TooLarge {
                    limit: limits.max_bytes,
                });
            }
        }
        std::thread::sleep(TAR_POLL);
    }
}

/// Extracts a RAR archive with the system tar (`tar.exe -xf <archive> -C <dest>`, no shell).
///
/// `dest` is cleared first. After tar finishes the result is validated: links are rejected and
/// the actual entry/byte counts are enforced against `limits` (the destination is emptied again
/// when validation fails). A tar failure is reported as `Failed { backend: SystemTar, .. }`
/// carrying tar's stderr.
pub(crate) fn extract_rar_with_tar(
    archive: &Path,
    dest: &Path,
    limits: Limits,
    cancel: &dyn Fn() -> bool,
) -> Result<(), ArchiveError> {
    let Some((tar, _version)) = system_tar() else {
        return Err(ArchiveError::Failed {
            backend: Backend::SystemTar,
            message: "tar.exe is not available on this system".to_string(),
        });
    };
    clear_dir(dest)?;
    let archive_abs = std::path::absolute(archive).map_err(|e| io_err(archive, e))?;
    let dest_abs = std::path::absolute(dest).map_err(|e| io_err(dest, e))?;

    let mut cmd = Command::new(&tar);
    cmd.arg("-xf")
        .arg(&archive_abs)
        .arg("-C")
        .arg(&dest_abs)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| ArchiveError::Failed {
        backend: Backend::SystemTar,
        message: format!("could not start {}: {e}", tar.display()),
    })?;
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).trim().to_string()
        })
    });

    let waited = wait_for_tar(&mut child, dest, limits, cancel);
    let stderr = stderr_reader
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    let outcome = match waited {
        Err(err) => Err(err),
        Ok(status) if !status.success() => Err(ArchiveError::Failed {
            backend: Backend::SystemTar,
            message: if stderr.is_empty() {
                format!("tar exited with {status}")
            } else {
                stderr
            },
        }),
        Ok(_) => validate_tar_result(dest, limits),
    };
    if outcome.is_err() {
        let _ = clear_dir(dest);
    }
    outcome
}

fn validate_tar_result(dest: &Path, limits: Limits) -> Result<(), ArchiveError> {
    let (files, bytes) = post_validate(dest)?;
    if files.len() > limits.max_entries {
        return Err(ArchiveError::TooManyEntries {
            limit: limits.max_entries,
        });
    }
    if bytes > limits.max_bytes {
        return Err(ArchiveError::TooLarge {
            limit: limits.max_bytes,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Capability probe
// ---------------------------------------------------------------------------------------------

/// True when the built-in UnRAR can actually list the embedded probe archive. This exercises the
/// real backend, not just linkage.
pub(crate) fn probe_builtin() -> bool {
    let path = std::env::temp_dir().join(format!(
        "kombini-rar-probe-{:016x}.rar",
        rand::random::<u64>()
    ));
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| file.write_all(PROBE_RAR));
    let ok = written.is_ok()
        && Archive::new(&path)
            .open_for_listing()
            .ok()
            .and_then(|mut list| list.next())
            .is_some_and(|first| first.is_ok());
    let _ = fs::remove_file(&path);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_part() {
        match parse_naming("Mod.Part02.RAR") {
            VolumeNaming::Part {
                prefix,
                number,
                width,
                suffix,
            } => {
                assert_eq!(prefix, "Mod.Part");
                assert_eq!(number, 2);
                assert_eq!(width, 2);
                assert_eq!(suffix, ".RAR");
            }
            _ => panic!("expected part naming"),
        }
        assert!(matches!(
            parse_naming("mod.rar"),
            VolumeNaming::Plain { .. }
        ));
        assert!(matches!(
            parse_naming("mod.part0.rar"),
            VolumeNaming::Plain { .. }
        ));
        assert!(matches!(
            parse_naming("mod.r01"),
            VolumeNaming::Old { number: 1, .. }
        ));
        assert!(matches!(parse_naming("mod.zip"), VolumeNaming::Other));
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use crate::discover_vpks;

    fn live_rars() -> Vec<PathBuf> {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return Vec::new();
        };
        let downloads = PathBuf::from(local)
            .join("Kombini")
            .join("KombiniModManager")
            .join("downloads");
        [
            "1854979_yamato_red_d0d08.rar",
            "1870920_reze_human_327e5.rar",
        ]
        .iter()
        .map(|name| downloads.join(name))
        .filter(|path| path.is_file())
        .collect()
    }

    #[test]
    fn tar_fallback_extracts_the_probe_archive() {
        if system_tar().is_none() {
            println!("skipping: no system tar");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("probe.rar");
        fs::write(&archive, PROBE_RAR).unwrap();
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("stale.txt"), b"stale").unwrap();
        extract_rar_with_tar(&archive, &dest, Limits::for_archive_size(87), &|| false).unwrap();
        assert!(
            !dest.join("stale.txt").exists(),
            "dest must be cleared first"
        );
        assert_eq!(
            fs::read_to_string(dest.join("VERSION")).unwrap(),
            "unrar-0.4.0"
        );
    }

    #[test]
    fn tar_fallback_enforces_limits_on_the_result() {
        if system_tar().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("probe.rar");
        fs::write(&archive, PROBE_RAR).unwrap();
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap();
        let tiny = Limits {
            max_bytes: 3,
            max_entries: 10,
        };
        let err = extract_rar_with_tar(&archive, &dest, tiny, &|| false).unwrap_err();
        assert!(
            matches!(err, ArchiveError::TooLarge { limit: 3 }),
            "{err:?}"
        );
        assert!(
            fs::read_dir(&dest).unwrap().next().is_none(),
            "dest must be emptied"
        );
    }

    #[test]
    fn tar_fallback_failure_carries_tar_stderr() {
        if system_tar().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("broken.rar");
        fs::write(&archive, b"this is not an archive").unwrap();
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap();
        let err = extract_rar_with_tar(&archive, &dest, Limits::for_archive_size(22), &|| false)
            .unwrap_err();
        assert!(
            matches!(
                err,
                ArchiveError::Failed {
                    backend: Backend::SystemTar,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    #[ignore = "uses real GameBanana downloads from the old app's folder (read-only)"]
    fn live_tar_fallback_extracts_real_rars() {
        let archives = live_rars();
        assert!(!archives.is_empty(), "no live RAR downloads found");
        for original in archives {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join(original.file_name().unwrap());
            fs::copy(&original, &archive).unwrap();
            let dest = dir.path().join("out");
            fs::create_dir(&dest).unwrap();
            let size = fs::metadata(&archive).unwrap().len();
            let started = Instant::now();
            extract_rar_with_tar(&archive, &dest, Limits::for_archive_size(size), &|| false)
                .unwrap();
            let elapsed = started.elapsed();
            let (files, bytes) = post_validate(&dest).unwrap();
            let vpks = discover_vpks(&dest);
            println!("== tar fallback: {}", original.display());
            println!("   elapsed: {elapsed:?}, bytes: {bytes}");
            println!("   files  : {files:#?}");
            println!("   vpks   : {vpks:#?}");
            assert!(!vpks.is_empty(), "expected at least one .vpk");
        }
    }
}

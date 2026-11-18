//! Safe zip/7z/rar extraction with magic-byte detection.
//!
//! Every archive is treated as hostile input: the format is decided from magic bytes (never the
//! extension), entry names are sanitized before anything is written, zip-bomb limits are charged
//! while bytes are written, links are rejected, and the extracted tree is validated afterwards.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod detect;
mod discover;
mod limits;
mod rar;
mod safety;
mod sevenz;
mod zipx;

use std::fs;
use std::path::{Path, PathBuf};

pub use detect::{ArchiveKind, detect, detect_bytes};
pub use discover::{DiscoveredVpk, discover_vpks};
pub use limits::Limits;
pub use safety::{post_validate, sanitize_entry};

use limits::{Ctx, copy_limited};
use safety::{create_output_file, io_err, join_rel};

/// The extraction backend that produced a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// The `zip` crate.
    Zip,
    /// The `sevenz-rust2` crate.
    SevenZ,
    /// The built-in UnRAR (the `unrar` crate).
    UnrarBuiltin,
    /// The system `tar.exe` / `bsdtar` (libarchive).
    SystemTar,
    /// A plain file copy (bare VPK).
    Copy,
}

/// What an extraction produced.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractReport {
    /// The backend used for the primary extraction.
    pub backend: Backend,
    /// All regular files in the destination: relative, `/`-separated, sorted.
    pub files: Vec<String>,
    /// Total size of `files` in bytes.
    pub bytes: u64,
    /// Relative paths of nested archives that were expanded (and removed).
    pub nested: Vec<String>,
    /// Non-fatal notes (for example a fallback to the system tar).
    pub warnings: Vec<String>,
}

/// Everything that can go wrong while detecting or extracting an archive.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("{0}: unsupported or unrecognized archive format")]
    Unsupported(PathBuf),
    #[error("unsafe path in archive: {0}")]
    UnsafePath(String),
    #[error("archive contains a link, which is not allowed: {0}")]
    Link(String),
    #[error("archive expands beyond the extraction size limit ({limit} bytes)")]
    TooLarge { limit: u64 },
    #[error("archive has too many entries (limit {limit})")]
    TooManyEntries { limit: usize },
    #[error("This archive is password-protected.")]
    Encrypted,
    #[error("This is part of a multi-volume archive; missing volume: {0}")]
    MissingVolume(String),
    #[error("extraction failed ({backend:?}): {message}")]
    Failed { backend: Backend, message: String },
    #[error("destination {0} must be an empty directory")]
    DestinationNotEmpty(PathBuf),
    #[error("cancelled")]
    Cancelled,
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Options for [`extract`].
pub struct ExtractOptions<'a> {
    /// Zip-bomb limits.
    pub limits: Limits,
    /// Polled during extraction; returning `true` aborts with [`ArchiveError::Cancelled`].
    pub cancel: &'a dyn Fn() -> bool,
    /// Called with `(bytes extracted so far, total if known)`.
    pub progress: &'a dyn Fn(u64, Option<u64>),
}

/// Which extraction backends work on this machine.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendStatus {
    pub zip: bool,
    pub sevenz: bool,
    /// True when the built-in UnRAR can list the embedded probe archive.
    pub rar_builtin: bool,
    /// Path of a working system tar (`%SystemRoot%\System32\tar.exe`, or `bsdtar` elsewhere).
    pub system_tar: Option<PathBuf>,
    /// First line of `tar --version`.
    pub system_tar_version: Option<String>,
}

/// Probes the extraction backends. `rar_builtin` really lists an embedded RAR archive with the
/// built-in UnRAR, so it tests the actual backend and not just that the library is linked in.
pub fn backend_status() -> BackendStatus {
    let tar = rar::system_tar();
    BackendStatus {
        zip: true,
        sevenz: true,
        rar_builtin: rar::probe_builtin(),
        system_tar: tar.as_ref().map(|(path, _)| path.clone()),
        system_tar_version: tar.map(|(_, version)| version),
    }
}

/// Makes sure `dest` exists and is an empty directory.
fn prepare_dest(dest: &Path) -> Result<(), ArchiveError> {
    match fs::metadata(dest) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(ArchiveError::DestinationNotEmpty(dest.to_path_buf()));
            }
            let mut entries = fs::read_dir(dest).map_err(|e| io_err(dest, e))?;
            if entries.next().is_some() {
                return Err(ArchiveError::DestinationNotEmpty(dest.to_path_buf()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(dest).map_err(|e| io_err(dest, e))
        }
        Err(e) => Err(io_err(dest, e)),
    }
}

/// Copies a bare VPK into `dest` under its own file name.
fn copy_vpk(archive: &Path, dest: &Path, ctx: &mut Ctx<'_>) -> Result<(), ArchiveError> {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let rel = sanitize_entry(&name).ok_or_else(|| ArchiveError::UnsafePath(name.clone()))?;
    let out = join_rel(dest, &rel);
    let mut src = fs::File::open(archive).map_err(|e| io_err(archive, e))?;
    if let Ok(meta) = src.metadata() {
        ctx.set_total(meta.len());
    }
    let mut dst = create_output_file(&out, &rel, Backend::Copy)?;
    copy_limited(&mut src, &mut dst, ctx, Backend::Copy, &out)?;
    ctx.report();
    Ok(())
}

/// Runs the primary extraction for an already detected `kind`. No nested expansion and no
/// post-validation happen here.
fn extract_primary(
    kind: ArchiveKind,
    archive: &Path,
    dest: &Path,
    ctx: &mut Ctx<'_>,
) -> Result<(Backend, Vec<String>), ArchiveError> {
    match kind {
        ArchiveKind::Zip => zipx::extract_zip(archive, dest, ctx).map(|()| (Backend::Zip, vec![])),
        ArchiveKind::SevenZ => {
            sevenz::extract_sevenz(archive, dest, ctx).map(|()| (Backend::SevenZ, vec![]))
        }
        ArchiveKind::Rar => rar::extract_rar(archive, dest, ctx),
        ArchiveKind::Vpk => copy_vpk(archive, dest, ctx).map(|()| (Backend::Copy, vec![])),
        ArchiveKind::Unknown => Err(ArchiveError::Unsupported(archive.to_path_buf())),
    }
}

/// `a/b/inner.7z` becomes `a/b/inner`; names without an extension are kept.
fn strip_extension(rel: &str) -> &str {
    let name_start = rel.rfind('/').map_or(0, |i| i + 1);
    match rel[name_start..].rfind('.') {
        Some(dot) if dot > 0 => &rel[..name_start + dot],
        _ => rel,
    }
}

/// Nested archive expansion, one level deep. When the extracted tree has no `.vpk`, every
/// contained zip/7z/rar is extracted into `<rel without extension>__extracted/` using whatever
/// byte/entry budget is left, and the nested archive file is deleted.
fn expand_nested(
    dest: &Path,
    opts: &ExtractOptions<'_>,
    files: &[String],
    bytes: u64,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, ArchiveError> {
    let mut expanded = Vec::new();
    if !discover_vpks(dest).is_empty() {
        return Ok(expanded);
    }
    let mut candidates = Vec::new();
    for rel in files {
        let path = join_rel(dest, rel);
        let kind = detect(&path).map_err(|e| io_err(&path, e))?;
        if matches!(
            kind,
            ArchiveKind::Zip | ArchiveKind::SevenZ | ArchiveKind::Rar
        ) {
            candidates.push((rel.clone(), kind));
        }
    }

    let mut used_bytes = bytes;
    let mut used_entries = files.len();
    for (rel, kind) in candidates {
        if (opts.cancel)() {
            return Err(ArchiveError::Cancelled);
        }
        let src = join_rel(dest, &rel);
        let nested_dest = join_rel(dest, &format!("{}__extracted", strip_extension(&rel)));
        prepare_dest(&nested_dest)?;
        let remaining = Limits {
            max_bytes: opts.limits.max_bytes.saturating_sub(used_bytes),
            max_entries: opts.limits.max_entries.saturating_sub(used_entries),
        };
        let base = used_bytes;
        let progress = move |done: u64, total: Option<u64>| {
            (opts.progress)(
                base.saturating_add(done),
                total.map(|t| base.saturating_add(t)),
            );
        };
        let mut ctx = Ctx::new(remaining, opts.cancel, &progress);
        let (_backend, nested_warnings) = extract_primary(kind, &src, &nested_dest, &mut ctx)?;
        warnings.extend(nested_warnings);
        fs::remove_file(&src).map_err(|e| io_err(&src, e))?;
        let (now_files, now_bytes) = post_validate(dest)?;
        used_bytes = now_bytes;
        used_entries = now_files.len();
        expanded.push(rel);
    }
    Ok(expanded)
}

/// Extracts `archive` into `dest` (created if missing; must be empty) and returns what was
/// extracted. The format comes from [`detect`]. A bare VPK is copied into `dest` under its file
/// name (backend [`Backend::Copy`]); unrecognized files fail with [`ArchiveError::Unsupported`].
///
/// After the primary extraction the result is validated with [`post_validate`]; if it contains
/// no `.vpk`, nested zip/7z/rar archives are expanded one level deep. The source archive is
/// never modified or deleted.
pub fn extract(
    archive: &Path,
    dest: &Path,
    opts: &ExtractOptions<'_>,
) -> Result<ExtractReport, ArchiveError> {
    let kind = detect(archive).map_err(|e| io_err(archive, e))?;
    if kind == ArchiveKind::Unknown {
        return Err(ArchiveError::Unsupported(archive.to_path_buf()));
    }
    prepare_dest(dest)?;
    if (opts.cancel)() {
        return Err(ArchiveError::Cancelled);
    }

    let mut ctx = Ctx::new(opts.limits, opts.cancel, opts.progress);
    let (backend, mut warnings) = extract_primary(kind, archive, dest, &mut ctx)?;

    let (files, bytes) = post_validate(dest)?;
    if files.len() > opts.limits.max_entries {
        return Err(ArchiveError::TooManyEntries {
            limit: opts.limits.max_entries,
        });
    }
    if bytes > opts.limits.max_bytes {
        return Err(ArchiveError::TooLarge {
            limit: opts.limits.max_bytes,
        });
    }

    let nested = expand_nested(dest, opts, &files, bytes, &mut warnings)?;

    let (files, bytes) = if nested.is_empty() {
        (files, bytes)
    } else {
        post_validate(dest)?
    };
    Ok(ExtractReport {
        backend,
        files,
        bytes,
        nested,
        warnings,
    })
}

//! Merge several VPKs (plus loose files) into one embedded-data VPK.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Result, VpkError};
use crate::extract::safe_relpath;
use crate::read::VpkArchive;
use crate::write::{EntrySource, normalize_entry_path, write_vpk_inner};

/// What to do when several inputs contain the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// Later inputs win.
    Last,
    /// Earlier inputs win.
    First,
    /// Any collision is an error.
    Error,
}

/// Options for [`merge_vpks`].
pub struct MergeOptions {
    pub policy: CollisionPolicy,
    /// Path to input index; the named input wins that path regardless of policy.
    pub overrides: HashMap<String, usize>,
    /// Loose files merged as a final, highest-priority input (subject to the policy).
    pub extra_files: Vec<(String, PathBuf)>,
}

/// Summary of a finished merge.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MergeReport {
    pub output: PathBuf,
    pub total_entries: usize,
    pub overridden_paths: Vec<String>,
    pub inputs: usize,
    pub strict: bool,
}

#[derive(Clone, Copy)]
enum Winner {
    Input(usize),
    Extra(usize),
}

/// Merge `inputs` into `output`. Data is streamed from the inputs; nothing is loaded wholesale.
pub fn merge_vpks(
    inputs: &[PathBuf],
    output: &Path,
    opts: &MergeOptions,
    cancel: &dyn Fn() -> bool,
) -> Result<MergeReport> {
    if inputs.is_empty() {
        return Err(VpkError::Invalid("no input VPKs".to_string()));
    }
    let mut archives: Vec<Arc<VpkArchive>> = Vec::with_capacity(inputs.len());
    for path in inputs {
        if cancel() {
            return Err(VpkError::Cancelled);
        }
        archives.push(Arc::new(VpkArchive::open(path)?));
    }

    let mut merged: HashMap<String, Winner> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut overridden: Vec<String> = Vec::new();
    for (index, archive) in archives.iter().enumerate() {
        if cancel() {
            return Err(VpkError::Cancelled);
        }
        for file_path in archive.file_paths() {
            let ov = opts.overrides.get(file_path).copied();
            if merged.contains_key(file_path) {
                if let Some(winner) = ov {
                    if winner == index {
                        merged.insert(file_path.to_string(), Winner::Input(index));
                        overridden.push(file_path.to_string());
                    }
                    continue;
                }
                match opts.policy {
                    CollisionPolicy::Error => {
                        return Err(VpkError::Collision(file_path.to_string()));
                    }
                    CollisionPolicy::First => continue,
                    CollisionPolicy::Last => {
                        merged.insert(file_path.to_string(), Winner::Input(index));
                        overridden.push(file_path.to_string());
                    }
                }
            } else {
                if ov.is_some_and(|w| w != index) {
                    continue;
                }
                merged.insert(file_path.to_string(), Winner::Input(index));
                order.push(file_path.to_string());
            }
        }
    }
    for (i, (file_path, _)) in opts.extra_files.iter().enumerate() {
        if safe_relpath(file_path).is_none() {
            return Err(VpkError::UnsafePath(file_path.clone()));
        }
        if merged.contains_key(file_path.as_str()) {
            overridden.push(file_path.clone());
            match opts.policy {
                CollisionPolicy::Error => return Err(VpkError::Collision(file_path.clone())),
                CollisionPolicy::First => continue,
                CollisionPolicy::Last => {}
            }
        } else {
            order.push(file_path.clone());
        }
        merged.insert(file_path.clone(), Winner::Extra(i));
    }

    let mut entries: Vec<(String, EntrySource)> = Vec::with_capacity(order.len());
    let mut expected: HashSet<String> = HashSet::with_capacity(order.len());
    for path in &order {
        let Some(&winner) = merged.get(path) else {
            continue;
        };
        let source = match winner {
            Winner::Input(i) => EntrySource::FromVpk {
                archive: Arc::clone(&archives[i]),
                path: path.clone(),
            },
            Winner::Extra(i) => EntrySource::File(opts.extra_files[i].1.clone()),
        };
        expected.insert(normalize_entry_path(path)?);
        entries.push((path.clone(), source));
    }
    let total_entries = entries.len();

    let (_, crcs) = write_vpk_inner(output, entries, cancel)?;
    if let Err(reason) = verify_output(output, &expected, &crcs, cancel) {
        if matches!(reason, VpkError::Cancelled) {
            return Err(reason);
        }
        let _ = std::fs::remove_file(output);
        return Err(VpkError::Invalid(format!(
            "merged VPK failed verification: {reason}"
        )));
    }

    overridden.sort();
    overridden.dedup();
    Ok(MergeReport {
        output: output.to_path_buf(),
        total_entries,
        overridden_paths: overridden,
        inputs: inputs.len(),
        strict: opts.policy == CollisionPolicy::Error,
    })
}

fn verify_output(
    output: &Path,
    expected: &HashSet<String>,
    crcs: &HashMap<String, u32>,
    cancel: &dyn Fn() -> bool,
) -> Result<()> {
    let written = VpkArchive::open(output)?;
    let actual: HashSet<&str> = written.entries().iter().map(|e| e.path.as_str()).collect();
    if actual.len() != expected.len() || !expected.iter().all(|p| actual.contains(p.as_str())) {
        return Err(VpkError::Invalid(format!(
            "path set mismatch (expected {}, found {})",
            expected.len(),
            actual.len()
        )));
    }
    for e in written.entries() {
        if crcs.get(&e.path) != Some(&e.crc32) {
            return Err(VpkError::Invalid(format!("CRC mismatch for {}", e.path)));
        }
    }
    let report = written.verify(cancel)?;
    if let Some(first) = report.failures.first() {
        return Err(VpkError::Invalid(format!(
            "{} entries failed CRC verification (first: {first})",
            report.failures.len()
        )));
    }
    Ok(())
}

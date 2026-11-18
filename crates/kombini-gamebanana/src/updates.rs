//! Update detection. Pure: compares what is installed with a
//! fetched profile page.

use crate::model::{GbFile, ModProfile};
use serde::{Deserialize, Serialize};

/// What we know about the installed copy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub file_id: Option<u64>,
    pub md5: Option<String>,
    pub version: Option<String>,
    /// Remote timestamp recorded at install (`_tsDateAdded` of the file or
    /// the submission's update date).
    pub remote_updated_at: Option<i64>,
    pub variant_desc: Option<String>,
    /// Downloaded file name (e.g. `skin_v2.zip`).
    pub file_name: Option<String>,
    /// Update the user chose to skip; stays skipped until a newer file id.
    pub skipped_file_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateCandidate {
    pub file: GbFile,
    pub reason: UpdateReason,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateReason {
    /// Same file id, but its checksum or upload date changed.
    FileChanged,
    /// The installed file was replaced; matched by variant description.
    SameVariant,
    /// Matched by file name.
    SameFileName,
    /// Only one current file exists.
    OnlyFile,
}

fn stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let base = lower
        .rsplit_once('.')
        .map(|(b, _)| b.to_owned())
        .unwrap_or(lower);
    // Strip trailing version-ish tokens ("_v2", "-1.3", " 2") so renamed
    // releases still match.
    let trimmed = base
        .trim_end_matches(|c: char| {
            c.is_ascii_digit() || c == '.' || c == '_' || c == '-' || c == ' '
        })
        .trim_end_matches(['v', 'V'])
        .trim_end_matches(['_', '-', ' ']);
    if trimmed.is_empty() {
        base
    } else {
        trimmed.to_owned()
    }
}

fn names_match(local: &str, remote: &str) -> bool {
    if local.is_empty() || remote.is_empty() {
        return false;
    }
    local.eq_ignore_ascii_case(remote) || stem(local) == stem(remote)
}

/// Returns the file to update to, or `None` when up to date.
pub fn find_update(installed: &Installed, profile: &ModProfile) -> Option<UpdateCandidate> {
    let candidate = find_update_inner(installed, profile)?;
    if installed.skipped_file_id == Some(candidate.file.id) {
        return None;
    }
    Some(candidate)
}

fn find_update_inner(installed: &Installed, profile: &ModProfile) -> Option<UpdateCandidate> {
    let current = profile.current_files();
    let make = |file: &GbFile, reason| UpdateCandidate {
        file: file.clone(),
        reason,
        version: profile.version.clone(),
    };
    let local_md5 = installed.md5.as_deref().map(str::to_ascii_lowercase);

    if let Some(file_id) = installed.file_id
        && let Some(exact) = current.iter().find(|f| f.id == file_id)
    {
        let remote_md5 = exact.md5.as_deref().map(str::to_ascii_lowercase);
        return match (&local_md5, &remote_md5) {
            (Some(l), Some(r)) if l == r => None,
            (Some(_), Some(_)) => Some(make(exact, UpdateReason::FileChanged)),
            _ => {
                let newer = matches!(
                    (exact.date_added, installed.remote_updated_at),
                    (Some(remote), Some(local)) if remote > local
                );
                newer.then(|| make(exact, UpdateReason::FileChanged))
            }
        };
    }

    // Installed file archived or removed: find its successor.
    if let Some(variant) = installed.variant_desc.as_deref().filter(|v| !v.is_empty())
        && let Some(f) = current
            .iter()
            .find(|f| f.description.as_deref() == Some(variant))
    {
        return Some(make(f, UpdateReason::SameVariant));
    }
    if let Some(name) = installed.file_name.as_deref()
        && let Some(f) = current.iter().find(|f| names_match(name, &f.file_name))
        && Some(f.id) != installed.file_id
    {
        return Some(make(f, UpdateReason::SameFileName));
    }
    if let [only] = current.as_slice()
        && Some(only.id) != installed.file_id
    {
        return Some(make(only, UpdateReason::OnlyFile));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: u64, name: &str, md5: &str, added: i64) -> GbFile {
        GbFile {
            id,
            file_name: name.into(),
            md5: Some(md5.into()),
            date_added: Some(added),
            ..GbFile::default()
        }
    }

    fn profile(files: Vec<GbFile>) -> ModProfile {
        ModProfile {
            files,
            version: Some("2.0".into()),
            ..ModProfile::default()
        }
    }

    #[test]
    fn same_file_same_md5_is_current() {
        let p = profile(vec![file(1, "a.zip", "AA", 100)]);
        let i = Installed {
            file_id: Some(1),
            md5: Some("aa".into()),
            ..Installed::default()
        };
        assert_eq!(find_update(&i, &p), None);
    }

    #[test]
    fn same_file_changed_md5() {
        let p = profile(vec![file(1, "a.zip", "BB", 100)]);
        let i = Installed {
            file_id: Some(1),
            md5: Some("aa".into()),
            ..Installed::default()
        };
        assert_eq!(
            find_update(&i, &p).unwrap().reason,
            UpdateReason::FileChanged
        );
    }

    #[test]
    fn replaced_file_matched_by_variant_then_name_then_only() {
        let mut blue = file(5, "skin_blue_v2.zip", "x", 200);
        blue.description = Some("Blue".into());
        let red = file(6, "skin_red_v2.zip", "y", 200);
        let p = profile(vec![blue, red]);
        let by_variant = Installed {
            file_id: Some(1),
            variant_desc: Some("Blue".into()),
            ..Installed::default()
        };
        assert_eq!(find_update(&by_variant, &p).unwrap().file.id, 5);
        let by_name = Installed {
            file_id: Some(2),
            file_name: Some("skin_red_v1.zip".into()),
            ..Installed::default()
        };
        assert_eq!(
            find_update(&by_name, &p).unwrap().reason,
            UpdateReason::SameFileName
        );
        let ambiguous = Installed {
            file_id: Some(3),
            ..Installed::default()
        };
        assert_eq!(find_update(&ambiguous, &p), None);
        let single = profile(vec![file(9, "renamed.zip", "z", 300)]);
        assert_eq!(
            find_update(&ambiguous, &single).unwrap().reason,
            UpdateReason::OnlyFile
        );
    }

    #[test]
    fn archived_files_never_chosen_and_skips_stick() {
        let mut old = file(1, "a.zip", "AA", 100);
        old.archived = true;
        let p = profile(vec![old, file(2, "a_v2.zip", "BB", 200)]);
        let i = Installed {
            file_id: Some(1),
            md5: Some("aa".into()),
            file_name: Some("a.zip".into()),
            ..Installed::default()
        };
        assert_eq!(find_update(&i, &p).unwrap().file.id, 2);
        let skipped = Installed {
            skipped_file_id: Some(2),
            ..i
        };
        assert_eq!(find_update(&skipped, &p), None);
    }
}

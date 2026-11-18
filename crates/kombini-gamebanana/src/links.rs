//! One-click install inputs: GameBanana URLs, deep links, share codes,
//! archives and interchange files.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// URL schemes the app registers.
pub const SCHEMES: &[&str] = &["kombini", "deadlockmm"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallRequest {
    GameBanana {
        submission_id: u64,
        file_id: Option<u64>,
    },
    /// A GameBanana collection (`gamebanana.com/collections/<id>`).
    Collection {
        collection_id: u64,
    },
    /// A local archive or VPK.
    Archive {
        path: PathBuf,
    },
    /// A remote archive URL (not GameBanana).
    ArchiveUrl {
        url: String,
    },
    /// `mp1:` share code or a `.modprofile.json` document.
    ProfileCode {
        text: String,
    },
    Interchange {
        path: PathBuf,
    },
}

const ARCHIVE_EXT: &[&str] = &["vpk", "zip", "rar", "7z"];

fn query_param<'a>(query: &'a str, keys: &[&str]) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        keys.iter()
            .any(|key| k.eq_ignore_ascii_case(key))
            .then_some(v)
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let hex = |b: u8| char::from(b).to_digit(16);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn split_url(url: &str) -> (&str, &str, &str) {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let (host_path, query) = after_scheme.split_once('?').unwrap_or((after_scheme, ""));
    let query = query.split('#').next().unwrap_or("");
    let (host, path) = host_path.split_once('/').unwrap_or((host_path, ""));
    (host, path, query)
}

/// Interprets pasted text, a dropped path, or a deep link.
pub fn parse(input: &str) -> Option<InstallRequest> {
    let text = input.trim().trim_matches('"');
    if text.is_empty() {
        return None;
    }
    if text.starts_with("mp1:") || text.starts_with('{') {
        return Some(InstallRequest::ProfileCode {
            text: text.to_owned(),
        });
    }
    let lower = text.to_ascii_lowercase();
    if let Some(scheme) = SCHEMES.iter().find(|s| lower.starts_with(&format!("{s}:"))) {
        let rest = &text[scheme.len() + 1..];
        let (host, path, query) = split_url(rest);
        if let Some(target) = query_param(query, &["url", "u"]) {
            return parse(&percent_decode(target));
        }
        // kombini://install/<submission>[/<file>]  or kombini:gb/<id>
        let parts: Vec<&str> = std::iter::once(host)
            .chain(path.split('/'))
            .filter(|p| !p.is_empty())
            .collect();
        let numbers: Vec<u64> = parts.iter().filter_map(|p| p.parse().ok()).collect();
        return numbers
            .first()
            .map(|&submission_id| InstallRequest::GameBanana {
                submission_id,
                file_id: numbers.get(1).copied(),
            });
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let (host, path, query) = split_url(text);
        let host = host.to_ascii_lowercase();
        let gamebanana = host == "gamebanana.com" || host.ends_with(".gamebanana.com");
        if gamebanana
            && let Some(rest) = path.to_ascii_lowercase().strip_prefix("collections/")
            && let Ok(collection_id) = rest
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
        {
            return Some(InstallRequest::Collection { collection_id });
        }
        if gamebanana
            && let Some(id) = crate::requirements::submission_id_from_url(&format!("/{path}"))
        {
            let file_id =
                query_param(query, &["file", "fileid", "dl"]).and_then(|v| v.parse().ok());
            return Some(InstallRequest::GameBanana {
                submission_id: id,
                file_id,
            });
        }
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if ARCHIVE_EXT.contains(&ext.as_str()) {
            return Some(InstallRequest::ArchiveUrl {
                url: text.to_owned(),
            });
        }
        return None;
    }
    let path = PathBuf::from(text);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name == "mod-interchange.json"
        || (path.is_dir() && path.join("mod-interchange.json").is_file())
    {
        return Some(InstallRequest::Interchange { path });
    }
    if name.ends_with(".modprofile.json") {
        return std::fs::read_to_string(&path)
            .ok()
            .map(|text| InstallRequest::ProfileCode { text });
    }
    let ext = name.rsplit('.').next().unwrap_or("");
    if ARCHIVE_EXT.contains(&ext) && path.exists() {
        return Some(InstallRequest::Archive { path });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_links() {
        assert_eq!(
            parse("https://gamebanana.com/mods/650634"),
            Some(InstallRequest::GameBanana {
                submission_id: 650634,
                file_id: None
            })
        );
        assert_eq!(
            parse("https://gamebanana.com/collections/164637"),
            Some(InstallRequest::Collection {
                collection_id: 164637
            })
        );
        assert_eq!(
            parse("https://gamebanana.com/mods/650634"),
            Some(InstallRequest::GameBanana {
                submission_id: 650634,
                file_id: None
            })
        );
        assert_eq!(
            parse("https://gamebanana.com/mods/12?file=99#x"),
            Some(InstallRequest::GameBanana {
                submission_id: 12,
                file_id: Some(99)
            })
        );
        assert_eq!(
            parse("kombini://install/12/34"),
            Some(InstallRequest::GameBanana {
                submission_id: 12,
                file_id: Some(34)
            })
        );
        assert_eq!(
            parse("deadlockmm://open?url=https%3A%2F%2Fgamebanana.com%2Fmods%2F5"),
            Some(InstallRequest::GameBanana {
                submission_id: 5,
                file_id: None
            })
        );
        assert_eq!(
            parse("https://example.com/files/skin.zip"),
            Some(InstallRequest::ArchiveUrl {
                url: "https://example.com/files/skin.zip".into()
            })
        );
        assert!(matches!(
            parse("mp1:abc"),
            Some(InstallRequest::ProfileCode { .. })
        ));
        assert_eq!(parse("https://example.com/page"), None);
        assert_eq!(parse("  "), None);
        assert_eq!(percent_decode("a%2Fb+c%zz"), "a/b c%zz");
    }
}

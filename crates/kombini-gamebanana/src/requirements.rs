//! `_aRequirements` parsing. GameBanana returns strings, objects, or
//! `[name, url]` pairs depending on age of the submission.

use kombini_core::Requirement;
use serde_json::Value;

/// Submission id from `.../mods/<id>` (also sounds/tools/maps/wips).
pub fn submission_id_from_url(url: &str) -> Option<u64> {
    let lower = url.to_ascii_lowercase();
    for kind in [
        "/mods/", "/sounds/", "/tools/", "/maps/", "/wips/", "/skins/",
    ] {
        if let Some(pos) = lower.find(kind) {
            let digits: String = lower[pos + kind.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(id) = digits.parse() {
                return Some(id);
            }
        }
    }
    None
}

fn from_value(value: &Value) -> Option<Requirement> {
    match value {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            let is_url = s.starts_with("http://") || s.starts_with("https://");
            Some(Requirement {
                name: s.to_owned(),
                url: is_url.then(|| s.to_owned()),
                submission_id: submission_id_from_url(s),
                author: None,
            })
        }
        Value::Array(pair) => {
            let name = pair.first()?.as_str()?.trim().to_owned();
            let url = pair.get(1).and_then(Value::as_str).map(str::to_owned);
            Some(Requirement {
                submission_id: url.as_deref().and_then(submission_id_from_url),
                name: if name.is_empty() { url.clone()? } else { name },
                url,
                author: None,
            })
        }
        Value::Object(map) => {
            let get = |keys: &[&str]| {
                keys.iter()
                    .find_map(|k| map.get(*k).and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            };
            let name = get(&[
                "_sName",
                "_sTitle",
                "_sValue",
                "name",
                "title",
                "_sDescription",
            ]);
            let url = get(&["_sUrl", "_sLink", "_sProfileUrl", "url", "link"]);
            let id = ["_idRow", "id", "submissionId"]
                .iter()
                .find_map(|k| match map.get(*k) {
                    Some(Value::Number(n)) => n.as_u64(),
                    Some(Value::String(s)) => s.parse().ok(),
                    _ => None,
                })
                .or_else(|| url.as_deref().and_then(submission_id_from_url));
            if name.is_none() && url.is_none() && id.is_none() {
                return None;
            }
            Some(Requirement {
                name: name
                    .or_else(|| url.clone())
                    .unwrap_or_else(|| format!("Submission {}", id.unwrap_or_default())),
                url,
                submission_id: id,
                author: get(&["_sAuthor", "author"]),
            })
        }
        _ => None,
    }
}

/// Parses and de-duplicates requirements.
pub fn parse(raw: &Value) -> Vec<Requirement> {
    let items: Vec<&Value> = match raw {
        Value::Array(list) => list.iter().collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    };
    let mut out: Vec<Requirement> = Vec::new();
    for item in items {
        if let Some(r) = from_value(item) {
            let dup = out.iter().any(|o| {
                o.name.eq_ignore_ascii_case(&r.name)
                    && o.submission_id == r.submission_id
                    && o.url == r.url
            });
            if !dup {
                out.push(r);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shapes() {
        let raw = json!([
            ["Base HUD", "https://gamebanana.com/mods/12345"],
            "https://gamebanana.com/mods/999",
            {"_sName": "Font pack", "_idRow": "77"},
            ["Base HUD", "https://gamebanana.com/mods/12345"],
            42
        ]);
        let r = parse(&raw);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].submission_id, Some(12345));
        assert_eq!(r[1].submission_id, Some(999));
        assert_eq!(r[2].submission_id, Some(77));
        assert!(parse(&Value::Null).is_empty());
    }
}

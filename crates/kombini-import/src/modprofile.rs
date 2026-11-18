//! Grimoire-compatible `mod-profile` documents and `mp1:` share codes.
//!
//! `mp1:` = URL-safe base64 (no padding) of gzip-compressed JSON.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use kombini_core::{AppError, ErrorCode, ModId, Profile, ProfileId};
use kombini_engine::{Engine, Progress};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const FORMAT: &str = "mod-profile";
const MAX_DECODED: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModRef {
    pub submission_id: Option<u64>,
    pub file_id: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Hint {
    pub name: Option<String>,
    pub category: Option<String>,
    pub file_label: Option<String>,
    pub original_file_name: Option<String>,
    pub nsfw: Option<bool>,
    pub is_archived: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    pub source: String,
    #[serde(rename = "ref")]
    pub reference: ModRef,
    pub enabled: Option<bool>,
    pub priority: Option<i64>,
    pub hint: Hint,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Document {
    pub format: String,
    pub schema_version: String,
    pub game: serde_json::Value,
    pub exported_at: Option<String>,
    pub exported_by: serde_json::Value,
    pub profile: serde_json::Value,
    pub mods: Vec<Entry>,
    pub extensions: serde_json::Value,
}

impl Document {
    pub fn name(&self) -> String {
        self.profile
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or("Imported profile")
            .to_owned()
    }
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::InvalidInput, msg)
}

/// Parses an `mp1:` code or raw JSON and validates format, schema and game.
pub fn decode(text: &str) -> Result<Document, AppError> {
    let text = text.trim();
    let json = if let Some(code) = text.strip_prefix("mp1:") {
        let raw = URL_SAFE_NO_PAD
            .decode(code.trim().trim_end_matches('='))
            .map_err(|_| invalid("This share code is damaged (not valid base64)"))?;
        let mut out = String::new();
        GzDecoder::new(raw.as_slice())
            .take(MAX_DECODED)
            .read_to_string(&mut out)
            .map_err(|_| invalid("This share code is damaged (could not decompress)"))?;
        out
    } else {
        text.to_owned()
    };
    let doc: Document =
        serde_json::from_str(&json).map_err(|e| invalid(format!("Not a mod profile: {e}")))?;
    if doc.format != FORMAT {
        return Err(invalid("Not a mod profile"));
    }
    let major = doc
        .schema_version
        .split('.')
        .next()
        .and_then(|m| m.parse::<u32>().ok())
        .unwrap_or(0);
    if major != 1 {
        return Err(invalid(format!(
            "Unsupported mod profile version {}",
            doc.schema_version
        )));
    }
    let steam = doc.game.get("steamAppId").and_then(json_u64);
    let gb = doc.game.get("gameBananaGameId").and_then(json_u64);
    if steam.is_some_and(|s| s != u64::from(kombini_core::APP_ID)) && gb.is_some_and(|g| g != 20948)
    {
        return Err(invalid("This profile is for a different game"));
    }
    Ok(doc)
}

fn json_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}

pub fn encode(doc: &Document) -> Result<String, AppError> {
    let json = serde_json::to_vec(doc).map_err(|e| AppError::internal(e.to_string()))?;
    let mut enc = GzEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&json)
        .map_err(|e| AppError::internal(e.to_string()))?;
    let gz = enc
        .finish()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(format!("mp1:{}", URL_SAFE_NO_PAD.encode(gz)))
}

/// Builds a document from a profile. Local (non-GameBanana) mods cannot be
/// shared and are listed in the returned warnings.
pub fn export(
    engine: &Engine,
    profile_id: Option<&ProfileId>,
) -> Result<(Document, Vec<String>), AppError> {
    let state = engine.state();
    let profile = match profile_id {
        Some(id) => state
            .profiles
            .iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| AppError::new(ErrorCode::ProfileNotFound, "Profile not found"))?,
        None => state.profile(),
    };
    let mut warnings = Vec::new();
    let mut mods = Vec::new();
    for id in &profile.order {
        let Some(m) = state.mods.get(id) else {
            continue;
        };
        let Some(remote) = m.remote_id else {
            warnings.push(format!("{} is a local mod and was left out", m.title));
            continue;
        };
        mods.push(Entry {
            source: "gamebanana".into(),
            reference: ModRef {
                submission_id: Some(remote),
                file_id: m.file_id,
            },
            enabled: Some(profile.enabled.contains(id)),
            priority: Some(mods.len() as i64),
            hint: Hint {
                name: Some(m.title.clone()),
                category: m
                    .category
                    .clone()
                    .or_else(|| m.primary_hero().map(str::to_owned)),
                file_label: m.variant_desc.clone(),
                original_file_name: m.file_name.clone(),
                nsfw: Some(false),
                is_archived: Some(false),
            },
        });
    }
    let pinned: Vec<u64> = profile
        .pinned
        .iter()
        .filter_map(|id| state.mods.get(id)?.remote_id)
        .collect();
    let doc = Document {
        format: FORMAT.into(),
        schema_version: "1.1".into(),
        game: serde_json::json!({ "steamAppId": kombini_core::APP_ID, "gameBananaGameId": 20948 }),
        exported_at: Some(format!("{}", kombini_storage::now())),
        exported_by: serde_json::json!({ "tool": "kombini-rust", "version": env!("CARGO_PKG_VERSION") }),
        profile: serde_json::json!({ "name": profile.name, "description": "", "author": "" }),
        mods,
        extensions: serde_json::json!({
            "kombini": { "launchOptions": profile.launch_options, "pinnedSubmissions": pinned },
            "grimoire": {
                "autoexecCommands": profile
                    .autoexec
                    .as_deref()
                    .map(|a| a.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>())
                    .unwrap_or_default()
            }
        }),
    };
    Ok((doc, warnings))
}

/// A mod the profile needs that is not installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Missing {
    pub submission_id: u64,
    pub file_id: Option<u64>,
    pub name: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileImport {
    pub profile_id: ProfileId,
    pub name: String,
    pub enabled: Vec<ModId>,
    pub missing: Vec<Missing>,
}

/// Creates a profile from the document using installed mods. Missing mods
/// are returned so the app can queue downloads (then call this again).
pub fn apply(
    engine: &Engine,
    doc: &Document,
    activate: bool,
    progress: &dyn Progress,
) -> Result<ProfileImport, AppError> {
    let name = doc.name();
    let (by_remote, existing) = engine.with_state(|s| {
        let map: std::collections::BTreeMap<u64, ModId> = s
            .mods
            .values()
            .filter_map(|m| m.remote_id.map(|r| (r, m.id.clone())))
            .collect();
        (map, s.profiles.iter().find(|p| p.name == name).cloned())
    });
    let mut profile = existing.unwrap_or_else(|| {
        Profile::new(
            ProfileId::new(format!("p{}", kombini_storage::random_hex(4))),
            name.clone(),
        )
    });
    let mut entries = doc.mods.clone();
    entries.sort_by_key(|e| e.priority.unwrap_or(i64::MAX));
    let mut order = Vec::new();
    let mut enabled = Vec::new();
    let mut missing = Vec::new();
    for e in &entries {
        let Some(sub) = e.reference.submission_id else {
            continue;
        };
        let on = e.enabled.unwrap_or(true);
        match by_remote.get(&sub) {
            Some(id) => {
                order.push(id.clone());
                if on {
                    enabled.push(id.clone());
                }
            }
            None => missing.push(Missing {
                submission_id: sub,
                file_id: e.reference.file_id,
                name: e.hint.name.clone(),
                enabled: on,
            }),
        }
    }
    profile.enabled = enabled.iter().cloned().collect();
    profile.order.retain(|m| !order.contains(m));
    order.append(&mut profile.order);
    profile.order = order;
    if let Some(cmds) = doc
        .extensions
        .pointer("/grimoire/autoexecCommands")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
    {
        profile.autoexec = Some(
            cmds.iter()
                .filter_map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if let Some(lo) = doc
        .extensions
        .pointer("/kombini/launchOptions")
        .and_then(|v| v.as_str())
    {
        profile.launch_options = Some(lo.to_owned());
    }
    if let Some(pins) = doc
        .extensions
        .pointer("/kombini/pinnedSubmissions")
        .and_then(|v| v.as_array())
    {
        profile.pinned = pins
            .iter()
            .filter_map(|p| by_remote.get(&p.as_u64()?).cloned())
            .collect();
    }
    let id = profile.id.clone();
    engine.put_profile(profile, activate, progress)?;
    Ok(ProfileImport {
        profile_id: id,
        name,
        enabled,
        missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_validation() {
        let doc = Document {
            format: FORMAT.into(),
            schema_version: "1.1".into(),
            game: serde_json::json!({"steamAppId": 1422450}),
            profile: serde_json::json!({"name": "Ranked"}),
            mods: vec![Entry {
                source: "gamebanana".into(),
                reference: ModRef {
                    submission_id: Some(5),
                    file_id: Some(9),
                },
                enabled: Some(true),
                priority: Some(0),
                hint: Hint::default(),
            }],
            ..Document::default()
        };
        let code = encode(&doc).unwrap();
        assert!(code.starts_with("mp1:") && !code.contains('='));
        let back = decode(&code).unwrap();
        assert_eq!(back.name(), "Ranked");
        assert_eq!(back.mods[0].reference.submission_id, Some(5));
        assert!(decode("mp1:!!!").is_err());
        assert!(decode(r#"{"format":"mod-profile","schemaVersion":"2.0"}"#).is_err());
        assert!(
            decode(r#"{"format":"mod-profile","schemaVersion":"1.0","game":{"steamAppId":1,"gameBananaGameId":2}}"#)
                .is_err()
        );
        assert!(decode(r#"{"format":"other"}"#).is_err());
    }
}

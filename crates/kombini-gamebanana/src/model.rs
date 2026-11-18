//! apiv11 response types. GameBanana mixes numbers and numeric strings, so
//! numeric fields use lenient deserializers; unknown fields are ignored.

use serde::{Deserialize, Deserializer, Serialize};

fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(lenient_opt_u64(d)?.unwrap_or(0))
}

fn lenient_opt_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn lenient_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Bool(b) => b,
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        serde_json::Value::String(s) => matches!(s.as_str(), "1" | "true" | "True"),
        _ => false,
    })
}

fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// `_aPreviewMedia._aImages[]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewImage {
    #[serde(rename = "_sBaseUrl", default)]
    pub base_url: String,
    #[serde(rename = "_sFile", default)]
    pub file: String,
    #[serde(rename = "_sFile220", default)]
    pub file_220: Option<String>,
    #[serde(rename = "_sFile530", default)]
    pub file_530: Option<String>,
    #[serde(rename = "_sFile800", default)]
    pub file_800: Option<String>,
    #[serde(rename = "_sCaption", default)]
    pub caption: Option<String>,
}

impl PreviewImage {
    /// URL of the smallest variant at least `width` wide (falls back to the original).
    pub fn url(&self, width: u32) -> String {
        let file = match width {
            0..=220 => self.file_220.as_ref().or(self.file_530.as_ref()),
            221..=530 => self.file_530.as_ref().or(self.file_800.as_ref()),
            _ => self.file_800.as_ref(),
        }
        .unwrap_or(&self.file);
        format!("{}/{}", self.base_url.trim_end_matches('/'), file)
    }

    /// The full-size upload.
    pub fn original_url(&self) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), self.file)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewMedia {
    #[serde(rename = "_aImages", default)]
    pub images: Vec<PreviewImage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    #[serde(rename = "_idRow", deserialize_with = "lenient_u64", default)]
    pub id: u64,
    #[serde(rename = "_sName", default)]
    pub name: String,
    #[serde(rename = "_sProfileUrl", default)]
    pub profile_url: Option<String>,
    #[serde(rename = "_sAvatarUrl", default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryRef {
    #[serde(rename = "_idRow", deserialize_with = "lenient_opt_u64", default)]
    pub id: Option<u64>,
    #[serde(rename = "_sName", default)]
    pub name: String,
    #[serde(rename = "_sProfileUrl", default)]
    pub profile_url: Option<String>,
    #[serde(rename = "_sIconUrl", deserialize_with = "lenient_string", default)]
    pub icon_url: Option<String>,
}

impl CategoryRef {
    /// Id from the record or parsed from `.../mods/cats/<id>`.
    pub fn id(&self) -> Option<u64> {
        self.id.or_else(|| {
            self.profile_url
                .as_deref()?
                .rsplit('/')
                .next()?
                .parse()
                .ok()
        })
    }
}

/// `/Mod/Categories` rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    #[serde(rename = "_idRow", deserialize_with = "lenient_u64", default)]
    pub id: u64,
    #[serde(rename = "_sName", default)]
    pub name: String,
    #[serde(rename = "_nItemCount", deserialize_with = "lenient_u64", default)]
    pub item_count: u64,
    #[serde(rename = "_sIconUrl", deserialize_with = "lenient_string", default)]
    pub icon_url: Option<String>,
    #[serde(rename = "_bIsObsolete", deserialize_with = "lenient_bool", default)]
    pub obsolete: bool,
}

/// A downloadable file of a submission.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GbFile {
    #[serde(rename = "_idRow", deserialize_with = "lenient_u64", default)]
    pub id: u64,
    #[serde(rename = "_sFile", default)]
    pub file_name: String,
    #[serde(rename = "_nFilesize", deserialize_with = "lenient_u64", default)]
    pub size: u64,
    #[serde(rename = "_tsDateAdded", deserialize_with = "lenient_i64", default)]
    pub date_added: Option<i64>,
    #[serde(rename = "_nDownloadCount", deserialize_with = "lenient_u64", default)]
    pub download_count: u64,
    #[serde(rename = "_sDownloadUrl", deserialize_with = "lenient_string", default)]
    pub download_url: Option<String>,
    #[serde(rename = "_sMd5Checksum", deserialize_with = "lenient_string", default)]
    pub md5: Option<String>,
    /// Variant label the author wrote for this file.
    #[serde(rename = "_sDescription", deserialize_with = "lenient_string", default)]
    pub description: Option<String>,
    #[serde(rename = "_bIsArchived", deserialize_with = "lenient_bool", default)]
    pub archived: bool,
    #[serde(
        rename = "_sAnalysisResult",
        deserialize_with = "lenient_string",
        default
    )]
    pub analysis_result: Option<String>,
    #[serde(rename = "_sAvResult", deserialize_with = "lenient_string", default)]
    pub av_result: Option<String>,
}

impl GbFile {
    /// `_sDownloadUrl` is authoritative; `/dl/<id>` is the fallback.
    pub fn url(&self) -> String {
        self.download_url
            .clone()
            .unwrap_or_else(|| format!("{}/{}", crate::DOWNLOAD_BASE, self.id))
    }

    /// GameBanana's antivirus flagged the file.
    pub fn flagged(&self) -> bool {
        self.av_result
            .as_deref()
            .is_some_and(|r| !r.eq_ignore_ascii_case("clean"))
    }
}

/// `/Mod/Index` records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSummary {
    #[serde(rename = "_idRow", deserialize_with = "lenient_u64", default)]
    pub id: u64,
    #[serde(rename = "_sName", default)]
    pub name: String,
    #[serde(rename = "_sProfileUrl", default)]
    pub profile_url: Option<String>,
    #[serde(rename = "_tsDateAdded", deserialize_with = "lenient_i64", default)]
    pub date_added: Option<i64>,
    #[serde(rename = "_tsDateModified", deserialize_with = "lenient_i64", default)]
    pub date_modified: Option<i64>,
    #[serde(rename = "_tsDateUpdated", deserialize_with = "lenient_i64", default)]
    pub date_updated: Option<i64>,
    #[serde(rename = "_sVersion", deserialize_with = "lenient_string", default)]
    pub version: Option<String>,
    #[serde(rename = "_aSubmitter", default)]
    pub submitter: Option<Member>,
    #[serde(rename = "_aRootCategory", default)]
    pub root_category: Option<CategoryRef>,
    #[serde(rename = "_aPreviewMedia", default)]
    pub preview: Option<PreviewMedia>,
    #[serde(rename = "_nLikeCount", deserialize_with = "lenient_u64", default)]
    pub likes: u64,
    #[serde(rename = "_nViewCount", deserialize_with = "lenient_u64", default)]
    pub views: u64,
    #[serde(rename = "_bIsObsolete", deserialize_with = "lenient_bool", default)]
    pub obsolete: bool,
    #[serde(rename = "_bHasFiles", deserialize_with = "lenient_bool", default)]
    pub has_files: bool,
    #[serde(
        rename = "_bHasContentRatings",
        deserialize_with = "lenient_bool",
        default
    )]
    pub has_content_ratings: bool,
    /// "show", "warn" (mature, click-through) or "hide" (mature, hidden by default).
    #[serde(
        rename = "_sInitialVisibility",
        deserialize_with = "lenient_string",
        default
    )]
    pub initial_visibility: Option<String>,
    /// "Mod", "Tool", "Wip"... (set on collection items and search results).
    #[serde(rename = "_sModelName", deserialize_with = "lenient_string", default)]
    pub model_name: Option<String>,
    #[serde(rename = "_aGame", default)]
    pub game: Option<GameRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameRef {
    #[serde(rename = "_idRow", deserialize_with = "lenient_u64", default)]
    pub id: u64,
    #[serde(rename = "_sName", default)]
    pub name: String,
}

/// `/Collection/{id}/ProfilePage` (the fields Kombini uses).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionProfile {
    #[serde(rename = "_idRow", deserialize_with = "lenient_u64", default)]
    pub id: u64,
    #[serde(rename = "_sName", default)]
    pub name: String,
    #[serde(rename = "_aSubmitter", default)]
    pub submitter: Option<Member>,
    #[serde(rename = "_nItemCount", deserialize_with = "lenient_u64", default)]
    pub item_count: u64,
}

/// A collection and all of its items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collection {
    pub profile: CollectionProfile,
    pub items: Vec<ModSummary>,
}

impl Collection {
    /// Items Kombini can install: Deadlock mods (any game id 0 is trusted).
    pub fn installable(&self) -> impl Iterator<Item = &ModSummary> {
        self.items.iter().filter(|m| m.is_installable())
    }
}

impl ModSummary {
    /// A Deadlock mod (not a tool, WiP or another game's submission).
    pub fn is_installable(&self) -> bool {
        self.model_name
            .as_deref()
            .is_none_or(|m| m.eq_ignore_ascii_case("Mod"))
            && self
                .game
                .as_ref()
                .is_none_or(|g| g.id == 0 || g.id == crate::DEADLOCK_GAME_ID)
            && self.has_files
    }

    /// GameBanana treats this mod as mature content: it carries content
    /// ratings or is not shown by default.
    pub fn is_nsfw(&self) -> bool {
        self.has_content_ratings
            || self
                .initial_visibility
                .as_deref()
                .is_some_and(|v| !v.eq_ignore_ascii_case("show"))
    }

    pub fn thumbnail(&self, width: u32) -> Option<String> {
        self.preview.as_ref()?.images.first().map(|i| i.url(width))
    }

    /// Last meaningful change (update, else modification, else creation).
    pub fn updated(&self) -> Option<i64> {
        self.date_updated.or(self.date_modified).or(self.date_added)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseMetadata {
    #[serde(rename = "_nRecordCount", deserialize_with = "lenient_u64", default)]
    pub record_count: u64,
    #[serde(rename = "_bIsComplete", deserialize_with = "lenient_bool", default)]
    pub complete: bool,
    #[serde(rename = "_nPerpage", deserialize_with = "lenient_u64", default)]
    pub per_page: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowsePage {
    #[serde(rename = "_aMetadata", default)]
    pub metadata: BrowseMetadata,
    #[serde(rename = "_aRecords", default)]
    pub records: Vec<ModSummary>,
}

/// One line of an update's changelog.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeItem {
    #[serde(default)]
    pub text: String,
    /// "Addition", "Adjustment", "Bugfix", ...
    #[serde(default)]
    pub cat: String,
}

/// `/Mod/<id>/Updates` record: an author's update post.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GbUpdate {
    #[serde(rename = "_idRow", deserialize_with = "lenient_u64", default)]
    pub id: u64,
    #[serde(rename = "_sName", default)]
    pub name: String,
    #[serde(rename = "_sVersion", deserialize_with = "lenient_string", default)]
    pub version: Option<String>,
    #[serde(rename = "_tsDateAdded", deserialize_with = "lenient_i64", default)]
    pub date_added: Option<i64>,
    /// HTML body.
    #[serde(rename = "_sText", deserialize_with = "lenient_string", default)]
    pub text: Option<String>,
    #[serde(rename = "_aChangeLog", default)]
    pub changes: Vec<ChangeItem>,
    #[serde(rename = "_sProfileUrl", deserialize_with = "lenient_string", default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatesPage {
    #[serde(rename = "_aRecords", default)]
    pub records: Vec<GbUpdate>,
}

/// `/Mod/<id>/ProfilePage`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModProfile {
    #[serde(rename = "_idRow", deserialize_with = "lenient_u64", default)]
    pub id: u64,
    #[serde(rename = "_sName", default)]
    pub name: String,
    #[serde(rename = "_sProfileUrl", default)]
    pub profile_url: Option<String>,
    /// HTML body.
    #[serde(rename = "_sText", deserialize_with = "lenient_string", default)]
    pub text: Option<String>,
    /// One-line summary.
    #[serde(rename = "_sDescription", deserialize_with = "lenient_string", default)]
    pub description: Option<String>,
    #[serde(rename = "_sVersion", deserialize_with = "lenient_string", default)]
    pub version: Option<String>,
    #[serde(rename = "_tsDateAdded", deserialize_with = "lenient_i64", default)]
    pub date_added: Option<i64>,
    #[serde(rename = "_tsDateModified", deserialize_with = "lenient_i64", default)]
    pub date_modified: Option<i64>,
    #[serde(rename = "_tsDateUpdated", deserialize_with = "lenient_i64", default)]
    pub date_updated: Option<i64>,
    #[serde(rename = "_aSubmitter", default)]
    pub submitter: Option<Member>,
    #[serde(rename = "_aCategory", default)]
    pub category: Option<CategoryRef>,
    #[serde(rename = "_aFiles", default)]
    pub files: Vec<GbFile>,
    #[serde(rename = "_aArchivedFiles", default)]
    pub archived_files: Vec<GbFile>,
    #[serde(rename = "_aRequirements", default)]
    pub requirements: serde_json::Value,
    #[serde(rename = "_aPreviewMedia", default)]
    pub preview: Option<PreviewMedia>,
    #[serde(rename = "_nLikeCount", deserialize_with = "lenient_u64", default)]
    pub likes: u64,
    #[serde(rename = "_nViewCount", deserialize_with = "lenient_u64", default)]
    pub views: u64,
    #[serde(rename = "_nDownloadCount", deserialize_with = "lenient_u64", default)]
    pub downloads: u64,
    #[serde(rename = "_bIsObsolete", deserialize_with = "lenient_bool", default)]
    pub obsolete: bool,
    #[serde(rename = "_bIsTrashed", deserialize_with = "lenient_bool", default)]
    pub trashed: bool,
    #[serde(rename = "_bIsWithheld", deserialize_with = "lenient_bool", default)]
    pub withheld: bool,
    #[serde(rename = "_bIsPrivate", deserialize_with = "lenient_bool", default)]
    pub private: bool,
}

impl ModProfile {
    /// Files the user may install by default (never archived ones).
    pub fn current_files(&self) -> Vec<&GbFile> {
        self.files.iter().filter(|f| !f.archived).collect()
    }

    /// Every file including archived ones (for explicit "install older file").
    pub fn all_files(&self) -> impl Iterator<Item = &GbFile> {
        self.files.iter().chain(self.archived_files.iter())
    }

    pub fn file(&self, id: u64) -> Option<&GbFile> {
        self.all_files().find(|f| f.id == id)
    }

    pub fn updated(&self) -> Option<i64> {
        self.date_updated.or(self.date_modified).or(self.date_added)
    }

    /// Screenshots in order (empty when the mod has none).
    pub fn gallery(&self) -> &[PreviewImage] {
        self.preview
            .as_ref()
            .map(|p| p.images.as_slice())
            .unwrap_or(&[])
    }

    pub fn images(&self) -> Vec<String> {
        self.preview
            .as_ref()
            .map(|p| p.images.iter().map(|i| i.url(800)).collect())
            .unwrap_or_default()
    }

    pub fn requirements(&self) -> Vec<kombini_core::Requirement> {
        crate::requirements::parse(&self.requirements)
    }

    /// Plain-text description for display.
    pub fn description_text(&self) -> String {
        self.text
            .as_deref()
            .map(crate::html::to_text)
            .filter(|t| !t.is_empty())
            .or_else(|| self.description.clone())
            .unwrap_or_default()
    }
}

/// API error body (`{"_sErrorCode": "...", "_sErrorMessage": "..."}`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ApiErrorBody {
    #[serde(rename = "_sErrorCode", default)]
    pub code: Option<String>,
    #[serde(rename = "_sErrorMessage", default)]
    pub message: Option<String>,
}

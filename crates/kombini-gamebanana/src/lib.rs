//! Blocking GameBanana apiv11 client for the Deadlock catalogue.
//!
//! The API is public and needs no key. Responses are cached in memory for
//! 15 minutes, images on disk. Callers run requests on a background thread.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod html;
pub mod links;
pub mod model;
pub mod requirements;
pub mod updates;

pub use model::{
    BrowsePage, Category, CategoryRef, ChangeItem, Collection, CollectionProfile, GbFile, GbUpdate,
    Member, ModProfile, ModSummary, PreviewImage,
};

use kombini_core::{AppError, ErrorCode};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const API_BASE: &str = "https://gamebanana.com/apiv11";
pub const DOWNLOAD_BASE: &str = "https://gamebanana.com/dl";
pub const DEADLOCK_GAME_ID: u64 = 20948;
pub const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_JSON: u64 = 16 * 1024 * 1024;
const MAX_IMAGE: u64 = 12 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum GbError {
    #[error("Could not reach GameBanana: {0}")]
    Network(String),
    #[error("GameBanana is rate limiting requests; try again in {retry_after_secs} s")]
    RateLimited { retry_after_secs: u64 },
    #[error("GameBanana returned HTTP {status}")]
    Http { status: u16 },
    #[error("{0}")]
    NotFound(String),
    #[error("Unexpected response from GameBanana: {0}")]
    Decode(String),
    #[error("Refusing to fetch {0}")]
    Forbidden(String),
}

impl From<GbError> for AppError {
    fn from(err: GbError) -> Self {
        let code = match &err {
            GbError::RateLimited { .. } => ErrorCode::RateLimited,
            GbError::NotFound(_) => ErrorCode::ModNotFound,
            GbError::Forbidden(_) => ErrorCode::InvalidInput,
            _ => ErrorCode::Network,
        };
        AppError::new(code, err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, GbError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Sort {
    #[default]
    MostDownloaded,
    Newest,
    LatestUpdated,
    MostLiked,
    MostViewed,
}

impl Sort {
    pub fn api(self) -> &'static str {
        match self {
            Self::MostDownloaded => "Generic_MostDownloaded",
            Self::Newest => "Generic_Newest",
            Self::LatestUpdated => "Generic_LatestUpdated",
            Self::MostLiked => "Generic_MostLiked",
            Self::MostViewed => "Generic_MostViewed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MostDownloaded => "Most downloaded",
            Self::Newest => "Newest",
            Self::LatestUpdated => "Recently updated",
            Self::MostLiked => "Most liked",
            Self::MostViewed => "Most viewed",
        }
    }

    pub const ALL: [Sort; 5] = [
        Self::MostDownloaded,
        Self::Newest,
        Self::LatestUpdated,
        Self::MostLiked,
        Self::MostViewed,
    ];
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseQuery {
    pub page: u32,
    pub per_page: u32,
    pub sort: Sort,
    pub search: String,
    pub category: Option<u64>,
}

type Cache<K, V> = Mutex<HashMap<K, (Instant, Arc<V>)>>;

pub struct Client {
    agent: ureq::Agent,
    base: String,
    profiles: Cache<u64, ModProfile>,
    pages: Cache<String, BrowsePage>,
    categories: Cache<(), Vec<Category>>,
    subcategories: Cache<u64, Vec<Category>>,
}

fn user_agent() -> String {
    format!(
        "KombiniModManager/{} (Deadlock mod manager; Rust)",
        env!("CARGO_PKG_VERSION")
    )
}

fn cached<K: std::hash::Hash + Eq, V>(cache: &Cache<K, V>, key: &K) -> Option<Arc<V>> {
    cache
        .lock()
        .get(key)
        .filter(|(at, _)| at.elapsed() < CACHE_TTL)
        .map(|(_, v)| v.clone())
}

fn stale<K: std::hash::Hash + Eq, V>(cache: &Cache<K, V>, key: &K) -> Option<Arc<V>> {
    cache.lock().get(key).map(|(_, v)| v.clone())
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self::with_base(API_BASE)
    }

    pub fn with_base(base: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(user_agent())
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(45)))
            .max_redirects(5)
            .build()
            .into();
        Self {
            agent,
            base: base.trim_end_matches('/').to_owned(),
            profiles: Mutex::new(HashMap::new()),
            pages: Mutex::new(HashMap::new()),
            categories: Mutex::new(HashMap::new()),
            subcategories: Mutex::new(HashMap::new()),
        }
    }

    /// GET JSON with up to 2 retries on 5xx/network errors (0.6 s, 1.2 s).
    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        let mut attempt: u32 = 0;
        loop {
            let mut req = self.agent.get(&url).header("Accept", "application/json");
            for (k, v) in query {
                req = req.query(*k, v);
            }
            let outcome = req.call();
            let retryable = match outcome {
                Ok(mut resp) => {
                    let status = resp.status().as_u16();
                    if status == 429 {
                        let retry_after_secs = resp
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(30);
                        return Err(GbError::RateLimited { retry_after_secs });
                    }
                    let text = resp
                        .body_mut()
                        .with_config()
                        .limit(MAX_JSON)
                        .read_to_string()
                        .map_err(|e| GbError::Network(e.to_string()))?;
                    if (200..300).contains(&status) || status == 404 {
                        if let Ok(err) = serde_json::from_str::<model::ApiErrorBody>(&text)
                            && let Some(code) = err.code
                        {
                            return Err(GbError::NotFound(err.message.unwrap_or(code)));
                        }
                        if status == 404 {
                            return Err(GbError::NotFound("Not found on GameBanana".into()));
                        }
                        return serde_json::from_str(&text)
                            .map_err(|e| GbError::Decode(e.to_string()));
                    }
                    if status < 500 {
                        return Err(GbError::Http { status });
                    }
                    GbError::Http { status }
                }
                Err(e) => GbError::Network(e.to_string()),
            };
            if attempt >= 2 {
                return Err(retryable);
            }
            attempt += 1;
            tracing::warn!(attempt, error = %retryable, url, "GameBanana request failed; retrying");
            std::thread::sleep(Duration::from_millis(600 * u64::from(attempt)));
        }
    }

    /// One page of Deadlock mods.
    pub fn browse(&self, q: &BrowseQuery) -> Result<Arc<BrowsePage>> {
        let mut query = vec![
            ("_nPage", q.page.max(1).to_string()),
            ("_nPerpage", q.per_page.clamp(1, 50).to_string()),
            ("_aFilters[Generic_Game]", DEADLOCK_GAME_ID.to_string()),
            ("_sSort", q.sort.api().to_owned()),
        ];
        // Mod/Index ignores search terms; searching uses Util/Search/Results
        // (best-match order, no category filter).
        let search = q.search.trim();
        let endpoint = if search.len() >= 2 {
            query = vec![
                ("_sModelName", "Mod".to_owned()),
                ("_sOrder", "best_match".to_owned()),
                ("_idGameRow", DEADLOCK_GAME_ID.to_string()),
                ("_sSearchString", search.to_owned()),
                ("_nPage", q.page.max(1).to_string()),
                ("_nPerpage", q.per_page.clamp(1, 50).to_string()),
            ];
            "Util/Search/Results"
        } else {
            if let Some(cat) = q.category {
                query.push(("_aFilters[Generic_Category]", cat.to_string()));
            }
            "Mod/Index"
        };
        let key = format!("{query:?}");
        if let Some(hit) = cached(&self.pages, &key) {
            return Ok(hit);
        }
        match self.get_json::<BrowsePage>(endpoint, &query) {
            Ok(page) => {
                let page = Arc::new(page);
                self.pages
                    .lock()
                    .insert(key, (Instant::now(), page.clone()));
                Ok(page)
            }
            Err(e) => stale(&self.pages, &key).ok_or(e),
        }
    }

    /// Full mod page with files. `force` bypasses the cache (update checks).
    pub fn profile(&self, id: u64, force: bool) -> Result<Arc<ModProfile>> {
        if !force && let Some(hit) = cached(&self.profiles, &id) {
            return Ok(hit);
        }
        match self.get_json::<ModProfile>(&format!("Mod/{id}/ProfilePage"), &[]) {
            Ok(mut p) => {
                if p.id == 0 {
                    p.id = id;
                }
                let p = Arc::new(p);
                self.profiles.lock().insert(id, (Instant::now(), p.clone()));
                Ok(p)
            }
            Err(e @ GbError::NotFound(_)) => Err(e),
            Err(e) => stale(&self.profiles, &id).ok_or(e),
        }
    }

    /// Child categories of `parent` (Skins → one per hero), A to Z.
    pub fn subcategories(&self, parent: u64) -> Result<Arc<Vec<Category>>> {
        if let Some(hit) = cached(&self.subcategories, &parent) {
            return Ok(hit);
        }
        let query = [
            ("_idCategoryRow", parent.to_string()),
            ("_sSort", "a_to_z".to_owned()),
            ("_bShowEmpty", "true".to_owned()),
        ];
        match self.get_json::<Vec<Category>>("Mod/Categories", &query) {
            Ok(list) => {
                let list = Arc::new(list);
                self.subcategories
                    .lock()
                    .insert(parent, (Instant::now(), list.clone()));
                Ok(list)
            }
            Err(e) => stale(&self.subcategories, &parent).ok_or(e),
        }
    }

    /// The author's update posts (changelog), newest first.
    pub fn updates(&self, id: u64, count: u32) -> Result<Vec<GbUpdate>> {
        let page: model::UpdatesPage = self.get_json(
            &format!("Mod/{id}/Updates"),
            &[
                ("_nPage", "1".to_owned()),
                ("_nPerpage", count.clamp(1, 50).to_string()),
            ],
        )?;
        Ok(page.records)
    }

    /// A collection with all of its items (the API pages them by 15).
    pub fn collection(&self, id: u64) -> Result<Collection> {
        /// Stops runaway paging; 50 pages is 750 items.
        const MAX_PAGES: u32 = 50;
        let profile: CollectionProfile =
            self.get_json(&format!("Collection/{id}/ProfilePage"), &[])?;
        let mut items: Vec<ModSummary> = Vec::new();
        for page in 1..=MAX_PAGES {
            let batch: BrowsePage = self.get_json(
                &format!("Collection/{id}/Items"),
                &[("_nPage", page.to_string())],
            )?;
            let done = batch.metadata.complete || batch.records.is_empty();
            items.extend(batch.records);
            if done {
                break;
            }
        }
        items.dedup_by_key(|m| m.id);
        Ok(Collection { profile, items })
    }

    pub fn categories(&self) -> Result<Arc<Vec<Category>>> {
        if let Some(hit) = cached(&self.categories, &()) {
            return Ok(hit);
        }
        let query = [
            ("_idGameRow", DEADLOCK_GAME_ID.to_string()),
            ("_sSort", "a_to_z".to_owned()),
            ("_bShowEmpty", "true".to_owned()),
        ];
        match self.get_json::<Vec<Category>>("Mod/Categories", &query) {
            Ok(list) => {
                let list = Arc::new(list.into_iter().filter(|c| !c.obsolete).collect::<Vec<_>>());
                self.categories
                    .lock()
                    .insert((), (Instant::now(), list.clone()));
                Ok(list)
            }
            Err(e) => stale(&self.categories, &()).ok_or(e),
        }
    }

    /// The underlying agent (downloads share connection pooling).
    pub fn agent(&self) -> &ureq::Agent {
        &self.agent
    }
}

/// Hosts images may be fetched from.
pub fn is_allowed_image_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("").to_ascii_lowercase();
    host == "gamebanana.com" || host.ends_with(".gamebanana.com")
}

/// Image fetch with a disk cache under `cache_dir` (keyed by URL hash).
pub struct ImageCache {
    agent: ureq::Agent,
    dir: PathBuf,
}

impl ImageCache {
    pub fn new(dir: &Path) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(user_agent())
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self {
            agent,
            dir: dir.to_path_buf(),
        }
    }

    pub fn path_for(&self, url: &str) -> PathBuf {
        let digest = hex::encode(Sha256::digest(url.as_bytes()));
        self.dir.join(&digest[..2]).join(format!("{digest}.img"))
    }

    /// Returns the cached file path, downloading on a miss.
    pub fn get(&self, url: &str) -> Result<PathBuf> {
        if !is_allowed_image_url(url) {
            return Err(GbError::Forbidden(url.to_owned()));
        }
        let path = self.path_for(url);
        if path.is_file() {
            return Ok(path);
        }
        let mut resp = self
            .agent
            .get(url)
            .call()
            .map_err(|e| GbError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(GbError::Http { status });
        }
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(MAX_IMAGE)
            .read_to_vec()
            .map_err(|e| GbError::Network(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| GbError::Network(e.to_string()))?;
        }
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, &bytes)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| GbError::Network(e.to_string()))?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_update_posts() {
        let json = r#"{"_aMetadata":{"_nRecordCount":1},"_aRecords":[{"_idRow":477064,"_sName":"Clean up",
            "_tsDateAdded":1794924818,"_sProfileUrl":"https://gamebanana.com/updates/477064","_sText":"",
            "_sVersion":"1.1","_aChangeLog":[{"text":"Cleaned up the file names.","cat":"Adjustment"}]},
            {"_idRow":2,"_sName":"Old","_tsDateAdded":"5","_aChangeLog":[]}]}"#;
        let page: model::UpdatesPage = serde_json::from_str(json).unwrap();
        assert_eq!(page.records.len(), 2);
        let u = &page.records[0];
        assert_eq!(u.version.as_deref(), Some("1.1"));
        assert_eq!(u.changes[0].cat, "Adjustment");
        assert_eq!(page.records[1].date_added, Some(5));
    }

    #[test]
    fn collection_items_filter_to_deadlock_mods() {
        let json = r#"{"_aMetadata":{"_nRecordCount":4,"_bIsComplete":true,"_nPerpage":15},"_aRecords":[
            {"_idRow":1,"_sModelName":"Mod","_sName":"a","_bHasFiles":true,"_aGame":{"_idRow":20948,"_sName":"Deadlock"}},
            {"_idRow":2,"_sModelName":"Tool","_sName":"b","_bHasFiles":true,"_aGame":{"_idRow":20948}},
            {"_idRow":3,"_sModelName":"Mod","_sName":"c","_bHasFiles":true,"_aGame":{"_idRow":297}},
            {"_idRow":4,"_sModelName":"Mod","_sName":"d","_bHasFiles":false,"_aGame":{"_idRow":20948}}]}"#;
        let page: BrowsePage = serde_json::from_str(json).unwrap();
        let c = Collection {
            profile: CollectionProfile::default(),
            items: page.records,
        };
        let ids: Vec<u64> = c.installable().map(|m| m.id).collect();
        assert_eq!(ids, [1]);
        let p: CollectionProfile = serde_json::from_str(
            r#"{"_idRow":164637,"_sName":"Picks","_nItemCount":241,"_aSubmitter":{"_idRow":1,"_sName":"x"}}"#,
        )
        .unwrap();
        assert_eq!((p.id, p.item_count), (164637, 241));
    }

    #[test]
    fn nsfw_from_content_ratings_or_visibility() {
        let records = r#"[
            {"_idRow":1,"_sName":"a","_bHasContentRatings":false,"_sInitialVisibility":"show"},
            {"_idRow":2,"_sName":"b","_bHasContentRatings":true,"_sInitialVisibility":"warn"},
            {"_idRow":3,"_sName":"c","_bHasContentRatings":false,"_sInitialVisibility":"hide"},
            {"_idRow":4,"_sName":"d","_bHasContentRatings":true},
            {"_idRow":5,"_sName":"e"}
        ]"#;
        let r: Vec<ModSummary> = serde_json::from_str(records).unwrap();
        let nsfw: Vec<bool> = r.iter().map(ModSummary::is_nsfw).collect();
        assert_eq!(nsfw, [false, true, true, true, false]);
    }

    #[test]
    fn parses_real_shapes() {
        let index = r#"{"_aMetadata":{"_nRecordCount":6280,"_bIsComplete":false,"_nPerpage":2},
            "_aRecords":[{"_idRow":650634,"_sName":"QOL Lock","_sProfileUrl":"https://gamebanana.com/mods/650634",
            "_tsDateAdded":1770149407,"_tsDateModified":1794371049,"_bHasFiles":true,"_aTags":[],
            "_aPreviewMedia":{"_aImages":[{"_sType":"screenshot","_sBaseUrl":"https://images.gamebanana.com/img/ss/mods","_sFile":"a.jpg","_sFile220":"220-90_a.jpg","_sFile530":"530-90_a.jpg"}]},
            "_aSubmitter":{"_idRow":5146349,"_sName":"civo"},
            "_aRootCategory":{"_sName":"Quality of Life/Fixes","_sProfileUrl":"https://gamebanana.com/mods/cats/46154","_sIconUrl":""},
            "_sVersion":"4.0.5","_tsDateUpdated":1792185473,"_bIsObsolete":false,"_nLikeCount":311,"_nViewCount":"238621"}]}"#;
        let page: BrowsePage = serde_json::from_str(index).unwrap();
        assert_eq!(page.metadata.record_count, 6280);
        let m = &page.records[0];
        assert_eq!((m.id, m.views, m.likes), (650634, 238621, 311));
        assert_eq!(m.root_category.as_ref().unwrap().id(), Some(46154));
        assert_eq!(
            m.thumbnail(200).unwrap(),
            "https://images.gamebanana.com/img/ss/mods/220-90_a.jpg"
        );
        let profile = r#"{"_idRow":650634,"_sName":"QOL","_sText":"<p>Hi</p>","_aFiles":[{"_idRow":1850914,
            "_sFile":"qollock.zip","_nFilesize":10116149,"_tsDateAdded":1792185439,
            "_sDownloadUrl":"https://gamebanana.com/dl/1850914","_sMd5Checksum":"8901a2ab","_sAvResult":"clean","_bIsArchived":false}],
            "_aArchivedFiles":[{"_idRow":1627686,"_sFile":"old.zip","_bIsArchived":true}],"_aRequirements":null}"#;
        let p: ModProfile = serde_json::from_str(profile).unwrap();
        assert_eq!(p.current_files().len(), 1);
        assert_eq!(
            p.file(1627686).unwrap().url(),
            "https://gamebanana.com/dl/1627686"
        );
        assert!(!p.files[0].flagged());
        assert_eq!(p.description_text(), "Hi");
        assert!(p.requirements().is_empty());
    }

    #[test]
    fn image_hosts() {
        assert!(is_allowed_image_url(
            "https://images.gamebanana.com/img/a.png"
        ));
        assert!(!is_allowed_image_url("http://images.gamebanana.com/a.png"));
        assert!(!is_allowed_image_url(
            "https://evil.com/gamebanana.com/a.png"
        ));
    }

    /// Hits the real API; run with `--ignored`.
    #[test]
    #[ignore]
    fn live_collection() {
        let c = Client::new().collection(164637).unwrap();
        assert!(c.profile.item_count > 15);
        assert_eq!(c.items.len() as u64, c.profile.item_count);
        assert!(c.installable().count() > 0);
    }

    #[test]
    #[ignore]
    fn live_browse_and_profile() {
        let c = Client::new();
        let page = c
            .browse(&BrowseQuery {
                page: 1,
                per_page: 5,
                ..BrowseQuery::default()
            })
            .unwrap();
        assert!(!page.records.is_empty());
        let p = c.profile(page.records[0].id, false).unwrap();
        assert!(!p.name.is_empty());
        assert!(!c.categories().unwrap().is_empty());
        assert!(matches!(
            c.profile(999_999_999, true),
            Err(GbError::NotFound(_))
        ));
    }
}

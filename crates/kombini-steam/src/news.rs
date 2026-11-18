//! Deadlock announcements from Valve's public Steam news API
//! (`ISteamNews/GetNewsForApp`, no key). Used for game update dates.

use serde::Deserialize;
use std::time::Duration;

const NEWS_URL: &str = "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/";
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// One announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsItem {
    pub title: String,
    /// Unix seconds.
    pub date: i64,
    pub url: String,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
struct Envelope {
    appnews: AppNews,
}

#[derive(Deserialize)]
struct AppNews {
    #[serde(default)]
    newsitems: Vec<RawItem>,
}

#[derive(Deserialize)]
struct RawItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    date: i64,
    #[serde(default)]
    url: String,
    #[serde(default)]
    tags: Vec<String>,
}

/// Parses a `GetNewsForApp` v2 response.
pub fn parse_news(json: &str) -> Result<Vec<NewsItem>, String> {
    let env: Envelope = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(env
        .appnews
        .newsitems
        .into_iter()
        .filter(|i| i.date > 0 && !i.title.trim().is_empty())
        .map(|i| NewsItem {
            title: i.title.trim().to_owned(),
            date: i.date,
            url: i.url,
            tags: i.tags,
        })
        .collect())
}

/// Fetches the latest `count` Deadlock announcements (official feed only).
pub fn fetch_news(count: u32) -> Result<Vec<NewsItem>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(format!(
            "KombiniModManager/{} (Deadlock mod manager; Rust)",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut resp = agent
        .get(NEWS_URL)
        .query("appid", crate::APP_ID.to_string())
        .query("count", count.to_string())
        .query("maxlength", "1")
        .query("feeds", "steam_community_announcements")
        .call()
        .map_err(|e| e.to_string())?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_news(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_news_response() {
        let json = r#"{"appnews":{"appid":1422450,"newsitems":[
            {"gid":"1","title":"Minor Update - 11-04-2026","url":"https://s/1","date":1793908000,"tags":["patchnotes"]},
            {"gid":"2","title":"Paint the Town","url":"https://s/2","date":1792184407},
            {"gid":"3","title":"  ","url":"","date":5}
        ],"count":3}}"#;
        let items = parse_news(json).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].tags, vec!["patchnotes".to_owned()]);
        assert!(items[1].tags.is_empty());
        assert!(parse_news("{}").is_err());
    }
}

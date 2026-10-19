//! Douyin adapter boundary.
//!
//! The resolver reads only a user-provided, system-keyring session. Platform
//! protocol changes are isolated in `DouyinLookup`, whose implementation must
//! use authorized endpoints and must not bypass access controls or DRM.
use crate::{
    credentials::{CredentialStore, credential_key},
    model::{Platform, RecordingTask, ResolvedStream},
    platform::{ResolveError, StreamResolver},
};
use async_trait::async_trait;
use chrono::Utc;
use regex::Regex;
use std::sync::{Arc, OnceLock};

#[async_trait]
pub trait DouyinLookup: Send + Sync {
    async fn resolve_authorized(
        &self,
        room_url: &url::Url,
        cookie: &str,
    ) -> Result<ResolvedStream, ResolveError>;
}

pub struct DouyinResolver {
    account_id: String,
    credentials: Arc<dyn CredentialStore>,
    lookup: Arc<dyn DouyinLookup>,
}

impl DouyinResolver {
    pub fn new(
        account_id: impl Into<String>,
        credentials: Arc<dyn CredentialStore>,
        lookup: Arc<dyn DouyinLookup>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            credentials,
            lookup,
        }
    }
}

#[async_trait]
impl StreamResolver for DouyinResolver {
    fn platform(&self) -> Platform {
        Platform::Douyin
    }

    async fn resolve(&self, task: &RecordingTask) -> Result<ResolvedStream, ResolveError> {
        let key = credential_key(Platform::Douyin, &self.account_id);
        let cookie = self
            .credentials
            .get(&key)
            .map_err(|_error| ResolveError::Failed("cannot access saved Douyin session".into()))?;
        let cookie = cookie
            .filter(|value| !value.trim().is_empty())
            .ok_or(ResolveError::AuthenticationRequired)?;
        self.lookup
            .resolve_authorized(&task.room_url, &cookie)
            .await
    }
}

/// Authorized web lookup for room pages that expose a playable HLS or FLV URL
/// in their server response. It intentionally does not generate signatures or
/// work around a page that withholds the stream metadata.
pub struct DouyinWebLookup {
    client: reqwest::Client,
}

impl DouyinWebLookup {
    pub fn new() -> Result<Self, ResolveError> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; LiveArchive/0.1)")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|error| ResolveError::Failed(format!("creating HTTP client: {error}")))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl DouyinLookup for DouyinWebLookup {
    async fn resolve_authorized(
        &self,
        room_url: &url::Url,
        cookie: &str,
    ) -> Result<ResolvedStream, ResolveError> {
        if !is_douyin_host(room_url) {
            return Err(ResolveError::Failed(
                "Douyin session credentials may only be sent to a douyin.com room URL".into(),
            ));
        }
        let response = self
            .client
            .get(room_url.clone())
            .header(reqwest::header::COOKIE, cookie)
            .send()
            .await
            .map_err(|error| ResolveError::Failed(format!("requesting room page: {error}")))?;
        match response.status().as_u16() {
            401 | 403 => return Err(ResolveError::AuthenticationRequired),
            404 => return Err(ResolveError::Offline),
            _ if !response.status().is_success() => {
                return Err(ResolveError::Failed(format!(
                    "room page returned HTTP {}",
                    response.status()
                )));
            }
            _ => {}
        }
        let page = response
            .text()
            .await
            .map_err(|error| ResolveError::Failed(format!("reading room page: {error}")))?;
        let stream_url = extract_stream_url(&page).ok_or(ResolveError::Offline)?;
        Ok(ResolvedStream {
            stream_url,
            title: extract_title(&page).unwrap_or_else(|| "Douyin live".into()),
            started_at: Utc::now(),
        })
    }
}

fn is_douyin_host(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host == "douyin.com" || host.ends_with(".douyin.com"))
}

fn extract_stream_url(page: &str) -> Option<url::Url> {
    static URL: OnceLock<Regex> = OnceLock::new();
    let normalized = page
        .replace("\\/", "/")
        .replace("\\u002F", "/")
        .replace("\\u0026", "&")
        .replace("&amp;", "&");
    let matcher = URL.get_or_init(|| {
        Regex::new(r#"https?://[^\"'\s<>]+?\.(?:m3u8|flv)(?:\?[^\"'\s<>]*)?"#).unwrap()
    });
    matcher
        .find(&normalized)
        .and_then(|value| url::Url::parse(value.as_str()).ok())
}

fn extract_title(page: &str) -> Option<String> {
    static USER_NICKNAME: OnceLock<Regex> = OnceLock::new();
    static ANCHOR_NAME: OnceLock<Regex> = OnceLock::new();
    static META_TITLE: OnceLock<Regex> = OnceLock::new();
    static HTML_TITLE: OnceLock<Regex> = OnceLock::new();
    let page = page
        .replace("\\\"", "\"")
        .replace("\\u002F", "/")
        .replace("\\u003C", "<")
        .replace("\\u003E", ">")
        .replace("&amp;", "&");
    // Douyin's room response associates the broadcaster with `data.user.nickname`.
    // Some page-state variants use `owner.nickname` instead.  Prefer those
    // structured fields over a page-wide `nickname`: the latter often contains
    // a template placeholder such as `$undefined` before room state is filled.
    let user_nickname = USER_NICKNAME.get_or_init(|| {
        Regex::new(
            r#"(?s)\"(?:user|owner)\"\s*:\s*\{[^{}]{0,4096}?\"nickname\"\s*:\s*\"((?:\\.|[^\"])*)\""#,
        )
        .unwrap()
    });
    let anchor_name = ANCHOR_NAME.get_or_init(|| {
        Regex::new(
            r#"\"(?:anchor_name|anchorName|anchor_nickname|anchorNickname|nickname|nick_name|nickName|user_name|userName|owner_name|ownerName)\"\s*:\s*\"([^\"]{1,120})\""#,
        )
        .unwrap()
    });
    let meta_title = META_TITLE.get_or_init(|| {
        Regex::new(
            r#"(?is)<meta[^>]+(?:property|name)=[\"'](?:og:title|twitter:title)[\"'][^>]+content=[\"']([^\"']{1,120})[\"']|<meta[^>]+content=[\"']([^\"']{1,120})[\"'][^>]+(?:property|name)=[\"'](?:og:title|twitter:title)[\"']"#,
        )
        .unwrap()
    });
    let html_title = HTML_TITLE
        .get_or_init(|| Regex::new(r#"(?is)<title[^>]*>\s*([^<]{1,120}?)\s*</title>"#).unwrap());
    user_nickname
        .captures_iter(&page)
        .filter_map(|captures| captures.get(1))
        .find_map(|value| normalized_creator_name(value.as_str()))
        .or_else(|| {
            anchor_name
                .captures_iter(&page)
                .filter_map(|captures| captures.get(1))
                .find_map(|value| normalized_creator_name(value.as_str()))
        })
        .or_else(|| {
            meta_title
                .captures(&page)
                .and_then(|captures| captures.get(1).or_else(|| captures.get(2)))
                .and_then(|value| normalized_creator_name(value.as_str()))
        })
        .or_else(|| {
            html_title
                .captures(&page)
                .and_then(|captures| captures.get(1))
                .and_then(|value| normalized_creator_name(value.as_str()))
        })
}

fn normalized_creator_name(value: &str) -> Option<String> {
    let value = value
        .trim()
        .split(" - 抖音")
        .next()
        .unwrap_or_default()
        .trim();
    let lowercase = value.to_ascii_lowercase();
    (!value.is_empty()
        && !matches!(
            lowercase.as_str(),
            "undefined"
                | "$undefined"
                | "null"
                | "$null"
                | "none"
                | "nil"
                | "[object object]"
                | "抖音直播"
                | "抖音直播间"
        ))
    .then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_escaped_hls_url_and_title() {
        let fixture = r#"{"nickname":"主播","hls_pull_url":"https:\/\/live.example.test\/path\/index.m3u8?token=a\u0026q=1"}"#;
        assert_eq!(extract_title(fixture).as_deref(), Some("主播"));
        assert_eq!(
            extract_stream_url(fixture).unwrap().as_str(),
            "https://live.example.test/path/index.m3u8?token=a&q=1"
        );
    }

    #[test]
    fn extracts_common_douyin_anchor_name_fields() {
        assert_eq!(
            extract_title(r#"{"anchorName":"实际主播"}"#).as_deref(),
            Some("实际主播")
        );
    }

    #[test]
    fn extracts_escaped_anchor_name_before_a_generic_page_title() {
        let fixture = r#"{\"title\":\"抖音直播间\",\"anchorNickname\":\"真实主播\"}"#;
        assert_eq!(extract_title(fixture).as_deref(), Some("真实主播"));
    }

    #[test]
    fn ignores_undefined_creator_field() {
        let fixture = r#"{\"nickname\":\"$undefined\",\"anchorName\":\"真实主播\"}"#;
        assert_eq!(extract_title(fixture).as_deref(), Some("真实主播"));
    }

    #[test]
    fn prefers_douyin_room_user_nickname_over_page_placeholders() {
        let fixture =
            r#"{"nickname":"$undefined","data":{"user":{"nickname":"实际主播","id_str":"1"}}}"#;
        assert_eq!(extract_title(fixture).as_deref(), Some("实际主播"));
    }

    #[test]
    fn accepts_douyin_room_owner_nickname_variant() {
        let fixture = r#"{"room":{"owner":{"nickname":"实际主播"}}}"#;
        assert_eq!(extract_title(fixture).as_deref(), Some("实际主播"));
    }
}

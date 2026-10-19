//! Handoff between the consented local capture proxy and the recorder.
//!
//! The network interception engine publishes only streams observed in the
//! user's own session. This module never derives encryption keys or attempts
//! to access content that was not captured by that approved proxy session.
use crate::{
    model::{Platform, RecordingTask, ResolvedStream},
    platform::{ResolveError, StreamResolver},
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use url::Url;

#[derive(Debug, Clone)]
pub struct CapturedWeChatStream {
    pub page_url: Url,
    pub stream_url: Url,
    pub title: String,
    pub anchor_username: Option<String>,
    pub frontend_task_id: Option<String>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct WeChatCaptureStore {
    by_page_url: Mutex<HashMap<String, CapturedWeChatStream>>,
}

impl WeChatCaptureStore {
    /// Adds a stream that has already passed the capture ingress policy.
    ///
    /// The direct capture endpoint admits HTTPS only, while the unchanged
    /// reference Channels frontend can legitimately report an HTTP CDN URL.
    /// Keeping the policy at ingress lets this store remain an inert handoff
    /// cache and avoids rejecting that latter, explicitly authorized path.
    pub fn publish(&self, stream: CapturedWeChatStream) -> Result<(), ResolveError> {
        if !matches!(stream.stream_url.scheme(), "http" | "https") {
            return Err(ResolveError::Failed(
                "captured media URL must use HTTP or HTTPS".into(),
            ));
        }
        self.by_page_url
            .lock()
            .unwrap()
            .insert(normalize_page_url(&stream.page_url), stream);
        Ok(())
    }

    pub fn clear(&self) {
        self.by_page_url.lock().unwrap().clear();
    }

    /// Returns whether a stream has been captured for this local task identity.
    /// This lets the command restore the system proxy as soon as the first
    /// media request is safely handed off to the recorder.
    pub fn contains_page(&self, page_url: &Url) -> bool {
        self.by_page_url
            .lock()
            .unwrap()
            .contains_key(&normalize_page_url(page_url))
    }

    pub fn observed(&self) -> Vec<CapturedWeChatStream> {
        self.by_page_url.lock().unwrap().values().cloned().collect()
    }
}

pub struct WeChatChannelsResolver {
    captures: Arc<WeChatCaptureStore>,
    max_age: Duration,
}

impl WeChatChannelsResolver {
    pub fn new(captures: Arc<WeChatCaptureStore>) -> Self {
        Self {
            captures,
            max_age: Duration::minutes(10),
        }
    }
    pub fn with_max_age(captures: Arc<WeChatCaptureStore>, max_age: Duration) -> Self {
        Self { captures, max_age }
    }
}

#[async_trait]
impl StreamResolver for WeChatChannelsResolver {
    fn platform(&self) -> Platform {
        Platform::WeChatChannels
    }
    async fn resolve(&self, task: &RecordingTask) -> Result<ResolvedStream, ResolveError> {
        let captured = self
            .captures
            .by_page_url
            .lock()
            .unwrap()
            .get(&normalize_page_url(&task.room_url))
            .cloned()
            .ok_or(ResolveError::AuthenticationRequired)?;
        if Utc::now() - captured.captured_at > self.max_age {
            return Err(ResolveError::Offline);
        }
        Ok(ResolvedStream {
            stream_url: captured.stream_url,
            title: captured.title,
            started_at: captured.captured_at,
        })
    }
}

fn normalize_page_url(url: &Url) -> String {
    let mut normalized = url.clone();
    normalized.set_fragment(None);
    normalized.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_store_matches_page_while_ignoring_fragment() {
        let store = WeChatCaptureStore::default();
        let page_url = Url::parse("https://channels.weixin.qq.com/live-archive/creator").unwrap();
        store
            .publish(CapturedWeChatStream {
                page_url: page_url.clone(),
                stream_url: Url::parse("https://media.example.test/live.flv").unwrap(),
                title: "Creator".into(),
                anchor_username: Some("creator-id".into()),
                frontend_task_id: None,
                captured_at: Utc::now(),
            })
            .unwrap();

        assert!(store.contains_page(
            &Url::parse("https://channels.weixin.qq.com/live-archive/creator#player").unwrap()
        ));
    }
}

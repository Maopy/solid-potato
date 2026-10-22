//! Select only URLs advertised by the current Channels live response.
use serde_json::Value;

fn tier(value: &str) -> u8 {
    let s = value.to_lowercase();
    if ["origin", "原画", "蓝光"].iter().any(|v| s.contains(v)) {
        6
    } else if ["uhd", "4k", "超高清"].iter().any(|v| s.contains(v)) {
        5
    } else if ["full_hd", "fhd", "1080", "超清"]
        .iter()
        .any(|v| s.contains(v))
    {
        4
    } else if s == "hd" || s.starts_with("hd_") || s.contains("高清") || s.contains("720") {
        3
    } else if s == "sd" || s.contains("标清") {
        2
    } else if s == "ld" || s.contains("流畅") {
        1
    } else {
        0
    }
}
fn number(v: Option<&Value>) -> u64 {
    v.and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .unwrap_or(0)
}
fn score(v: &Value) -> (u8, u64, u64) {
    let label = [
        "tag_name",
        "tagName",
        "video_quality_level_desc",
        "videoQualityLevelDesc",
        "name",
    ]
    .iter()
    .filter_map(|k| v.get(k).and_then(Value::as_str))
    .map(tier)
    .max()
    .unwrap_or(0);
    (
        label,
        number(v.get("width")).saturating_mul(number(v.get("height"))),
        number(v.get("bitrate")).max(number(v.get("rate"))),
    )
}
fn valid_url(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
fn visit(
    v: &Value,
    depth: usize,
    remaining: &mut usize,
    best: &mut Option<((u8, u64, u64), String)>,
) {
    if depth > 10 || *remaining == 0 {
        return;
    }
    *remaining -= 1;
    match v {
        Value::Object(map) => {
            if let Some(url) = map
                .get("liveCdnUrl")
                .and_then(Value::as_str)
                .filter(|u| valid_url(u))
            {
                if best.is_none() {
                    *best = Some(((0, 0, 0), url.to_owned()));
                }
            }
            for (key, child) in map.iter().take(100) {
                if matches!(
                    key.as_str(),
                    "recording_streams" | "cdn_trans_info" | "cdnTransInfo"
                ) {
                    if let Some(options) = child.as_array() {
                        for option in options.iter().take(64) {
                            if let Some(url) = option
                                .get("url")
                                .and_then(Value::as_str)
                                .filter(|u| valid_url(u))
                            {
                                let candidate = score(option);
                                if best
                                    .as_ref()
                                    .is_none_or(|(current, _)| candidate > *current)
                                {
                                    *best = Some((candidate, url.to_owned()));
                                }
                            }
                        }
                    }
                } else {
                    visit(child, depth + 1, remaining, best);
                }
            }
        }
        Value::Array(items) => {
            for child in items.iter().take(64) {
                visit(child, depth + 1, remaining, best);
            }
        }
        Value::String(s) if s.len() < 262144 && s.trim_start().starts_with('{') => {
            if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                visit(&parsed, depth + 1, remaining, best);
            }
        }
        _ => {}
    }
}
pub(crate) fn wechat_stream(live: &Value) -> Option<String> {
    let mut best = None;
    visit(live, 0, &mut 2048, &mut best);
    best.map(|(_, url)| url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn selects_highest_advertised_quality_and_keeps_signed_url_unchanged() {
        let v = json!({"liveSdkInfo":{"liveCdnUrl":"https://media.test/default"},"recording_streams":[
            {"url":"https://media.test/low","tag_name":"sd","rate":900},
            {"url":"https://media.test/origin?token=signed&quality=source","video_quality_level_desc":"原画","rate":2000},
            {"url":"https://media.test/hd","video_quality_level_desc":"高清","rate":5000}]});
        assert_eq!(
            wechat_stream(&v).as_deref(),
            Some("https://media.test/origin?token=signed&quality=source")
        );
    }
    #[test]
    fn native_options_can_be_nested_or_json_encoded_and_use_resolution_then_rate() {
        let v = json!({"liveSdkInfo":{"liveCdnUrl":"https://media.test/default","sdkInfo":json!({"cdn_trans_info":[
            {"url":"https://media.test/720","width":720,"height":1280,"rate":"5000"},
            {"url":"https://media.test/1080low","width":1080,"height":1920,"rate":2000},
            {"url":"https://media.test/1080high","width":1080,"height":1920,"rate":4000}]}).to_string()}});
        assert_eq!(
            wechat_stream(&v).as_deref(),
            Some("https://media.test/1080high")
        );
    }
    #[test]
    fn falls_back_without_alternatives_and_ignores_unrelated_and_invalid_urls() {
        let v = json!({"liveSdkInfo":{"liveCdnUrl":"https://media.test/default"},"recommendation":{"url":"https://other.test/stream","tag_name":"原画"},"recording_streams":[{"url":"file:///tmp/video","tag_name":"原画"},{"url":"https://user:password@media.test/stream","tag_name":"原画"}]});
        assert_eq!(
            wechat_stream(&v).as_deref(),
            Some("https://media.test/default")
        );
        assert_eq!(wechat_stream(&json!({})), None);
    }
}

//! Xinics 커먼즈(UniPlayer) 강의 영상 페이지 인식.
//!
//! 대학 LMS 에서 많이 쓰는 이 플레이어는 HTML 에 영상 주소가 없고, 재생할 때
//! `content.php?content_id=…` XML 을 불러와 MP4 주소를 얻는다. yt-dlp 가 인식하지 못하므로
//! 플레이어와 같은 순서(페이지 → 설정 XML → MP4)로 영상 정보를 읽는다.
//!
//! - 접근 제어를 우회하지 않는다: 페이지와 XML 을 사용자가 연 그대로 읽을 뿐이며,
//!   영상 서버에는 브라우저가 그 페이지에서 재생할 때와 같은 Referer(페이지 출처)만 보낸다.
//! - DRM/암호화 형식(progressive MP4 가 아닌 것)은 다루지 않는다.
//! - 페이지 주소의 사용자 토큰(userId 등)은 로그에 남기지 않는다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::Url;

use super::safety::{redact_url, validate_url};
use super::ytdlp::{DirectMedia, UrlError, UrlVideo};

const MARKER: &str = "content.php?content_id=";

/// 받아 둔 페이지가 Xinics 플레이어 페이지이면 영상 정보를 돌려준다. 아니면 `Ok(None)`.
/// `source_url` 은 다시 받을 때 쓰는 원래 주소 (사용자가 입력한 주소 또는 iframe 주소).
pub fn from_page(
    http: &reqwest::blocking::Client,
    page: &Url,
    html: &str,
    source_url: &str,
    cancel: &AtomicBool,
) -> Result<Option<Vec<UrlVideo>>, UrlError> {
    let Some(content_path) = find_content_uri(html) else {
        return Ok(None);
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(UrlError::Cancelled);
    }
    let xml_url = page.join(&content_path).map_err(|e| UrlError::Parse(e.to_string()))?;
    // 설정 XML 은 같은 사이트에서만 읽는다
    if xml_url.origin() != page.origin() {
        return Ok(None);
    }
    tracing::info!(page = %redact_url(page.as_str()), "Xinics 플레이어 페이지 - 설정 XML 읽기");
    let (_, xml) = super::page::fetch_text(http, xml_url, None)?;
    let Some(info) = parse_content_xml(&xml) else {
        return Err(UrlError::NoVideos);
    };
    let media = validate_url(&info.media_uri)?;
    let referer = super::page::origin_referer(page);
    let ext = media
        .rsplit('/')
        .next()
        .and_then(|name| name.split(['?', '#']).next())
        .and_then(|name| name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()))
        .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "mp4".to_owned());
    Ok(Some(vec![UrlVideo {
        source_url: source_url.to_owned(),
        playlist_index: None,
        id: info.content_id,
        title: if info.title.trim().is_empty() { "강의 영상".to_owned() } else { info.title },
        duration: info.duration,
        ext,
        resolution: None,
        has_video: info.has_video,
        is_live: false,
        drm: false,
        extractor: "Xinics".to_owned(),
        media_url: Some(media.clone()),
        direct: Some(DirectMedia { url: media, referer }),
        request: Default::default(),
    }]))
}

/// 페이지의 `contentUri = '…content.php?content_id=…'` 를 찾는다.
fn find_content_uri(html: &str) -> Option<String> {
    let at = html.find(MARKER)?;
    let start = html[..at].rfind(['\'', '"'])? + 1;
    let end = at + html[at..].find(['\'', '"'])?;
    let uri = &html[start..end];
    // 짧고 단순한 경로만 (스크립트 조각이나 다른 사이트 주소가 섞이지 않게)
    let ok = uri.len() < 512
        && uri.chars().all(|c| c.is_ascii_alphanumeric() || "/._-?=&:".contains(c))
        && uri[uri.find(MARKER)? + MARKER.len()..].chars().all(|c| c.is_ascii_alphanumeric());
    ok.then(|| uri.to_owned())
}

#[derive(Debug, PartialEq)]
struct ContentInfo {
    content_id: String,
    title: String,
    duration: Option<Duration>,
    media_uri: String,
    has_video: bool,
}

fn parse_content_xml(xml: &str) -> Option<ContentInfo> {
    let main = section(xml, "main_media")?;
    // 데스크톱 HTML5(progressive) → 그 밖의 첫 미디어 주소
    let media_uri = section(main, "desktop")
        .and_then(|d| section(d, "html5"))
        .and_then(|h| tag(h, "media_uri"))
        .or_else(|| tag(main, "media_uri"))?;
    let method = section(main, "html5").and_then(|h| tag(h, "method")).unwrap_or_default();
    if !method.is_empty() && method != "progressive" {
        return None; // 스트리밍 보호 형식 등은 다루지 않는다
    }
    let meta = section(xml, "content_metadata").unwrap_or("");
    Some(ContentInfo {
        content_id: tag(xml, "content_id").unwrap_or_default(),
        title: tag(meta, "title").unwrap_or_default(),
        duration: tag(xml, "content_duration")
            .and_then(|d| d.parse::<f64>().ok())
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(Duration::from_secs_f64),
        media_uri,
        has_video: tag(xml, "content_type").is_none_or(|t| !t.starts_with("audio")),
    })
}

/// `<name>…</name>` 안쪽 (첫 번째)
fn section<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let start = xml.find(&open)?;
    let body_start = start + xml[start..].find('>')? + 1;
    let end = body_start + xml[body_start..].find(&close)?;
    Some(&xml[body_start..end])
}

/// 태그 값 (CDATA·공백 제거, 기본 XML 엔티티 해제)
fn tag(xml: &str, name: &str) -> Option<String> {
    let raw = section(xml, name)?.trim();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    let value = raw
        .trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0"?>
<content version="1.0"><content_playing_info version="1.0"><content_id>691c78806e785</content_id><content_type>video1</content_type>
<content_duration>2327.43</content_duration>
<main_media><desktop><html5><method>progressive</method><media_uri>https://cdn.example.ac.kr/contents/media_files/mobile/ssmovie.mp4</media_uri></html5>
<flash_fallback><method>pseudo</method><media_uri>https://other/x.mp4</media_uri></flash_fallback></desktop></main_media>
</content_playing_info><content_metadata version="1.0"><title><![CDATA[2025-11-18 강의 & 실습]]></title></content_metadata></content>"#;

    #[test]
    fn parses_player_xml() {
        let info = parse_content_xml(XML).expect("parse");
        assert_eq!(info.content_id, "691c78806e785");
        assert_eq!(info.title, "2025-11-18 강의 & 실습");
        assert_eq!(info.media_uri, "https://cdn.example.ac.kr/contents/media_files/mobile/ssmovie.mp4");
        assert_eq!(info.duration, Some(Duration::from_secs_f64(2327.43)));
        assert!(info.has_video);
    }

    #[test]
    fn rejects_non_progressive() {
        let xml = XML.replace("<method>progressive</method>", "<method>drm_stream</method>");
        assert!(parse_content_xml(&xml).is_none());
    }

    #[test]
    fn finds_content_uri_only_in_simple_form() {
        let page = "var playerType = 'Uniplayer';\nvar contentUri = '/viewer/ssplayer/uniplayer_support/content.php?content_id=691c78806e785';";
        assert_eq!(
            find_content_uri(page).as_deref(),
            Some("/viewer/ssplayer/uniplayer_support/content.php?content_id=691c78806e785")
        );
        assert_eq!(find_content_uri("<html>nothing</html>"), None);
        assert_eq!(find_content_uri("x = 'content.php?content_id=a b<script>'"), None);
    }
}

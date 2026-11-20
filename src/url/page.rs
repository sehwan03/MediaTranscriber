//! yt-dlp 가 영상을 찾지 못한 페이지를 직접 살펴보는 도구.
//!
//! - 페이지 HTML 을 한 번 받아 (로그인 쿠키 없이) 그 안의 iframe 과 미디어 주소(.mp4 · .m3u8)를 찾는다.
//! - 페이지에서 얻은 주소는 신뢰하지 않는다: http/https 만, 길이·문자 검증, 내부망 주소는
//!   페이지 자체가 내부망일 때만 허용한다. 외부 프로그램에는 항상 인자로만 넘긴다.

use std::time::Duration;

use reqwest::Url;

use super::safety::validate_url;
use super::ytdlp::UrlError;

/// 응답 크기 상한 (페이지·설정 파일은 작다. 더 크면 영상 파일 등이므로 읽지 않는다)
const MAX_BODY: usize = 4 * 1024 * 1024;
/// 따라갈 iframe 최대 개수
pub const MAX_IFRAMES: usize = 6;
/// 페이지에서 직접 찾은 미디어 주소 최대 개수
pub const MAX_MEDIA: usize = 20;

/// 강의가 아닌 플레이어 부속 영상 (커먼즈 플레이어 인트로 등)
const IGNORED_MEDIA: &[&str] = &["/uniplayer/intro.mp4", "/uniplayer/preloader.mp4"];

/// DRM·보안 플레이어 표시 (이런 페이지는 우회하지 않고 안내만 한다)
const DRM_MARKERS: &[(&str, &str)] = &[
    ("kollus.com", "Kollus"),
    ("com.widevine.alpha", "Widevine"),
    ("pallycon", "PallyCon"),
    ("inkaentworks", "INKA"),
    ("com.microsoft.playready", "PlayReady"),
    ("com.apple.fps", "FairPlay"),
];

/// 페이지에 DRM·보안 플레이어 흔적이 있으면 그 이름
pub fn drm_marker(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    DRM_MARKERS.iter().find(|(m, _)| lower.contains(m)).map(|(_, name)| *name)
}

/// 영상과 관계없는 흔한 iframe (광고·분석·로그인 위젯 등)
const IGNORED_HOSTS: &[&str] = &[
    "googletagmanager.com",
    "doubleclick.net",
    "google-analytics.com",
    "google.com/recaptcha",
    "facebook.com/plugins",
    "platform.twitter.com",
    "accounts.google.com",
    "googlesyndication.com",
];

pub fn client() -> Result<reqwest::blocking::Client, UrlError> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        // 일부 사이트는 브라우저가 아닌 요청에 다른 페이지를 준다
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36")
        .build()
        .map_err(|e| UrlError::Failed(e.to_string()))
}

/// 페이지를 받아 (리다이렉트 후 최종 주소, 본문) 을 돌려준다. 본문이 너무 크면 빈 문자열.
pub fn fetch_text(http: &reqwest::blocking::Client, url: Url, referer: Option<&str>) -> Result<(Url, String), UrlError> {
    use std::io::Read;
    let mut req = http.get(url);
    if let Some(r) = referer {
        req = req.header(reqwest::header::REFERER, r);
    }
    let resp = req.send().map_err(|e| {
        if e.is_timeout() { UrlError::Timeout } else { UrlError::Failed(super::safety::redact_text(&e.to_string())) }
    })?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(UrlError::AuthRequired(format!("HTTP {}", status.as_u16())));
    }
    if !status.is_success() {
        return Err(UrlError::Failed(format!("HTTP {}", status.as_u16())));
    }
    let final_url = resp.url().clone();
    // 영상·음성·이미지 응답은 읽지 않는다 (설정 파일은 서버가 종류를 잘못 붙이는 경우가 있어 그 밖은 읽음)
    let is_text = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|t| !["video/", "audio/", "image/", "font/"].iter().any(|p| t.trim_start().starts_with(p)));
    if !is_text {
        return Ok((final_url, String::new()));
    }
    let mut body = Vec::new();
    resp.take(MAX_BODY as u64 + 1).read_to_end(&mut body)?;
    if body.len() > MAX_BODY {
        return Ok((final_url, String::new()));
    }
    Ok((final_url, String::from_utf8_lossy(&body).into_owned()))
}

/// 페이지 출처 + "/" (브라우저가 다른 사이트의 iframe·영상 요청에 보내는 Referer 와 같은 형태)
pub fn origin_referer(page: &Url) -> String {
    format!("{}/", page.origin().ascii_serialization())
}

/// `<title>` 내용
pub fn title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let body = start + lower[start..].find('>')? + 1;
    let end = body + lower[body..].find("</title>")?;
    let t = decode_entities(html[body..end].trim());
    (!t.is_empty()).then_some(t)
}

/// `<iframe src="…">` (와 지연 로딩용 `data-src`) 주소들. 중복·무관한 주소는 뺀다.
pub fn find_iframes(html: &str, base: &Url) -> Vec<Url> {
    let lower = html.to_ascii_lowercase();
    let mut out: Vec<Url> = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<iframe") {
        let start = pos + i;
        let end = start + lower[start..].find('>').unwrap_or(lower.len() - start);
        let tag = &html[start..end];
        for attr in ["src", "data-src"] {
            if let Some(v) = attr_value(tag, attr)
                && let Some(u) = resolve(&decode_entities(&v), base)
                && !IGNORED_HOSTS.iter().any(|h| u.as_str().contains(h))
                && !out.contains(&u)
            {
                out.push(u);
            }
        }
        pos = end;
        if out.len() >= MAX_IFRAMES {
            break;
        }
    }
    out
}

/// HTML · 스크립트 · JSON 안의 `.mp4` / `.m3u8` 주소들 (`\/` · `&` · `&amp;` 이스케이프 해제)
pub fn find_media_urls(html: &str, base: &Url) -> Vec<Url> {
    let text = html.replace("\\/", "/").replace("\\u0026", "&").replace("\\u002F", "/").replace("&amp;", "&");
    let lower = text.to_ascii_lowercase();
    let mut out: Vec<Url> = Vec::new();
    for ext in [".m3u8", ".mp4"] {
        let mut pos = 0;
        while let Some(i) = lower[pos..].find(ext) {
            let hit = pos + i;
            pos = hit + ext.len();
            // 확장자 바로 뒤가 문자/숫자면 (.mp4a 등) 주소가 아니다
            if lower[pos..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
            let start = text[..hit].rfind(is_boundary).map_or(0, |b| b + 1);
            let end = pos + text[pos..].find(is_boundary).unwrap_or(text.len() - pos);
            if let Some(u) = resolve(&text[start..end], base)
                && !IGNORED_MEDIA.iter().any(|m| u.path().to_ascii_lowercase().ends_with(m))
                && !out.contains(&u)
            {
                out.push(u);
                if out.len() >= MAX_MEDIA {
                    return out;
                }
            }
        }
    }
    out
}

fn is_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '\\')
}

/// 태그 문자열에서 속성 값 (따옴표 유무 모두)
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find(name) {
        let at = pos + i;
        pos = at + name.len();
        // 앞 글자가 공백이어야 정확히 그 속성 (예: data-src 안의 src 와 구분)
        if !lower[..at].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = tag[pos..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        return Some(value.to_owned());
    }
    None
}

/// 페이지 기준으로 주소를 풀고 검증한다. 내부망 주소는 페이지도 내부망일 때만 허용.
pub fn resolve(raw: &str, base: &Url) -> Option<Url> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2048 || raw.starts_with("data:") || raw.starts_with("blob:") || raw.starts_with("javascript:") {
        return None;
    }
    let url = base.join(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    validate_url(url.as_str()).ok()?;
    if is_private_host(&url) && !is_private_host(base) {
        return None;
    }
    Some(url)
}

fn is_private_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else { return true };
    match host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Err(_) => host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost"),
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://lms.example.ac.kr/course/view?id=1").expect("url")
    }

    #[test]
    fn finds_iframes_and_skips_noise() {
        let html = r#"<iframe width=640 src="https://player.example.com/em/abc?x=1&amp;y=2"></iframe>
            <IFRAME data-src='/viewer/play.php?id=7' src="about:blank"></IFRAME>
            <iframe src="https://www.googletagmanager.com/ns.html?id=GTM"></iframe>
            <iframe src="javascript:void(0)"></iframe>"#;
        let found: Vec<String> = find_iframes(html, &base()).into_iter().map(String::from).collect();
        assert_eq!(found, vec!["https://player.example.com/em/abc?x=1&y=2", "https://lms.example.ac.kr/viewer/play.php?id=7"]);
    }

    #[test]
    fn finds_media_urls_in_scripts_and_json() {
        let html = r#"<script>var cfg = {"file":"https:\/\/cdn.example.com\/v\/lec01.mp4?sig=a&e=1","poster":"x.jpg"};
            player.setup({ sources: [{ file: '/hls/lec02/index.m3u8' }] });
            var codec = "audio/mp4a.40.2"; </script>
            <video><source src="//cdn2.example.com/lec03.mp4"></video>"#;
        let found: Vec<String> = find_media_urls(html, &base()).into_iter().map(String::from).collect();
        assert_eq!(
            found,
            vec![
                "https://lms.example.ac.kr/hls/lec02/index.m3u8",
                "https://cdn.example.com/v/lec01.mp4?sig=a&e=1",
                "https://cdn2.example.com/lec03.mp4",
            ]
        );
    }

    #[test]
    fn skips_player_intro_and_detects_drm() {
        let html = "<script>a='/uniplayer/intro.mp4'; b='/contents/media_files/screen.mp4';</script>";
        let found: Vec<String> = find_media_urls(html, &base()).into_iter().map(String::from).collect();
        assert_eq!(found, vec!["https://lms.example.ac.kr/contents/media_files/screen.mp4"]);
        assert_eq!(drm_marker("<iframe src='https://v.kr.kollus.com/s?jwt=x'>"), Some("Kollus"));
        assert_eq!(drm_marker("keySystems: {'com.widevine.alpha': {}}"), Some("Widevine"));
        assert_eq!(drm_marker("<video src='a.mp4'>"), None);
    }

    #[test]
    fn blocks_internal_hosts_from_public_pages() {
        assert!(resolve("http://127.0.0.1:8080/a.mp4", &base()).is_none());
        assert!(resolve("http://192.168.0.5/a.mp4", &base()).is_none());
        let local = Url::parse("http://127.0.0.1:9000/page.html").expect("url");
        assert!(resolve("/a.mp4", &local).is_some());
        assert!(resolve("ftp://x/a.mp4", &base()).is_none());
    }

    #[test]
    fn reads_title_and_referer() {
        assert_eq!(title("<html><head><TITLE> 3주차 &amp; 실습 </TITLE>").as_deref(), Some("3주차 & 실습"));
        assert_eq!(origin_referer(&base()), "https://lms.example.ac.kr/");
    }
}

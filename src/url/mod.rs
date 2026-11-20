//! URL 변환: 웹페이지·스트리밍 주소에서 영상을 찾고, 선택한 영상의 오디오만 추출한다.
//!
//! 추출한 임시 WAV 는 기존 변환 과정(Local / Cloud Whisper, 언어 설정, TXT 저장)에 그대로 넘긴다.
//! 별도의 음성 인식 엔진을 만들지 않는다.
//!
//! 영상 찾기 순서 ([`discover`]):
//! 1. yt-dlp (1,700여 개 사이트 + 일반 페이지의 `<video>`·알려진 플레이어 임베드)
//! 2. 브라우저가 아닌 요청을 막는 사이트(403·Cloudflare)면 브라우저 요청 흉내로 한 번 더
//! 3. 그래도 없으면 페이지를 직접 살펴본다 ([`page`]):
//!    강의 플레이어(Xinics) 설정 → 페이지 안의 .mp4/.m3u8 주소 → iframe 안의 페이지(위 과정을 한 단계 더)

pub mod download;
pub mod extract;
pub mod installer;
pub mod page;
pub mod safety;
pub mod xinics;
pub mod ytdlp;

use std::sync::atomic::{AtomicBool, Ordering};

use reqwest::Url;

pub use ytdlp::{Browser, DirectMedia, RequestOpts, UrlAuth, UrlError, UrlVideo, YtDlp, fill_missing_durations};

/// 주소에서 영상을 찾는다.
pub fn discover(ytdlp: &YtDlp, url: &str, auth: &UrlAuth, cancel: &AtomicBool) -> Result<Vec<UrlVideo>, UrlError> {
    let first = match ytdlp.discover(url, auth, cancel) {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };
    let first = if is_blocked(&first) {
        tracing::info!("요청이 거부됨 - 브라우저 요청 흉내로 다시 시도");
        let opts = RequestOpts { referer: None, impersonate: true };
        match ytdlp.discover_with(url, auth, &opts, cancel) {
            Ok(v) => return Ok(v),
            Err(UrlError::Cancelled) => return Err(UrlError::Cancelled),
            Err(_) => first,
        }
    } else {
        first
    };
    if !matches!(first, UrlError::Unsupported | UrlError::NoVideos) {
        return Err(first);
    }
    match inspect_page(ytdlp, url, auth, cancel) {
        Ok(videos) if !videos.is_empty() => {
            tracing::info!(count = videos.len(), "페이지를 직접 살펴 영상 찾음");
            Ok(videos)
        }
        Ok(_) => Err(first),
        Err(UrlError::Cancelled) => Err(UrlError::Cancelled),
        // 플레이어는 찾았지만 영상 정보를 못 읽은 경우 등은 그 오류가 더 정확하다
        Err(err @ (UrlError::AuthRequired(_) | UrlError::Drm)) => Err(err),
        Err(err) => {
            tracing::warn!(error = %err, "페이지 직접 확인 실패");
            Err(first)
        }
    }
}

/// 403 · Cloudflare 등 '브라우저가 아니어서' 거부된 것으로 보이는 오류
fn is_blocked(err: &UrlError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("403") || text.contains("cloudflare") || text.contains("captcha") || text.contains("429")
}

fn inspect_page(ytdlp: &YtDlp, url: &str, auth: &UrlAuth, cancel: &AtomicBool) -> Result<Vec<UrlVideo>, UrlError> {
    let url = safety::validate_url(url)?;
    let http = page::client()?;
    let start = Url::parse(&url).map_err(|e| UrlError::Parse(e.to_string()))?;
    let (page_url, html) = page::fetch_text(&http, start, None)?;
    let mut found = from_html(&http, &page_url, &html, &url, cancel)?;
    if !found.is_empty() {
        return Ok(found);
    }
    // 영상을 못 찾았을 때 DRM·보안 플레이어 흔적이 있으면 그것이 원인이라고 알린다 (우회하지 않음)
    let mut drm = page::drm_marker(&html);

    // iframe 안의 페이지: 각각 yt-dlp → 직접 확인. 브라우저처럼 바깥 페이지를 Referer 로 보낸다.
    let referer = page::origin_referer(&page_url);
    for frame in page::find_iframes(&html, &page_url) {
        if cancel.load(Ordering::Relaxed) {
            return Err(UrlError::Cancelled);
        }
        if let Some(name) = page::drm_marker(frame.as_str()) {
            drm = drm.or(Some(name));
            continue; // 보안 플레이어는 열어 보지 않는다
        }
        tracing::info!(frame = %safety::redact_url(frame.as_str()), "iframe 안의 페이지 확인");
        let opts = RequestOpts { referer: Some(referer.clone()), impersonate: false };
        match ytdlp.discover_with(frame.as_str(), auth, &opts, cancel) {
            Ok(v) => {
                found.extend(v);
                continue;
            }
            Err(UrlError::Cancelled) => return Err(UrlError::Cancelled),
            Err(_) => {}
        }
        match page::fetch_text(&http, frame.clone(), Some(&referer)) {
            Ok((frame_url, frame_html)) => {
                let videos = from_html(&http, &frame_url, &frame_html, frame.as_str(), cancel)?;
                if videos.is_empty() {
                    drm = drm.or(page::drm_marker(&frame_html));
                }
                found.extend(videos);
            }
            Err(e) => tracing::debug!(error = %e, "iframe 페이지를 읽지 못함"),
        }
    }
    if found.is_empty()
        && let Some(name) = drm
    {
        tracing::info!(player = name, "DRM·보안 플레이어 페이지");
        return Err(UrlError::Drm);
    }
    Ok(found)
}

/// 받아 둔 HTML 에서: 강의 플레이어(Xinics) → 직접 미디어 주소
fn from_html(
    http: &reqwest::blocking::Client,
    page_url: &Url,
    html: &str,
    source_url: &str,
    cancel: &AtomicBool,
) -> Result<Vec<UrlVideo>, UrlError> {
    if let Some(v) = xinics::from_page(http, page_url, html, source_url, cancel)? {
        return Ok(v);
    }
    let media = page::find_media_urls(html, page_url);
    let title = page::title(html).unwrap_or_else(|| "영상".to_owned());
    let referer = page::origin_referer(page_url);
    let many = media.len() > 1;
    Ok(media
        .into_iter()
        .enumerate()
        .map(|(i, m)| {
            let path = m.path().to_ascii_lowercase();
            let hls = path.ends_with(".m3u8");
            let name = m.path_segments().and_then(|mut s| s.next_back()).unwrap_or("").to_owned();
            UrlVideo {
                // 다시 받을 때는 미디어 주소를 직접 쓰므로 원래 페이지 주소 + 순번으로 구분
                source_url: source_url.to_owned(),
                playlist_index: Some(i as u32 + 1),
                id: name.clone(),
                title: if many { format!("{title} ({})", i + 1) } else { title.clone() },
                duration: None,
                ext: if hls { "HLS".to_owned() } else { "mp4".to_owned() },
                resolution: None,
                has_video: true,
                is_live: false,
                drm: false,
                extractor: "page".to_owned(),
                media_url: Some(m.to_string()),
                direct: Some(DirectMedia { url: m.to_string(), referer: referer.clone() }),
                request: RequestOpts::default(),
            }
        })
        .collect())
}

#[cfg(test)]
pub(crate) mod tests;

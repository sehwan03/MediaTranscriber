//! URL 변환 통합 테스트.
//!
//! 외부 사이트 대신 테스트 안에서 띄운 로컬 HTTP 서버로 MP4 · HLS(m3u8) · 영상 여러 개가 있는 페이지를 제공한다.
//! yt-dlp 는 앱이 설치하는 위치(`%LOCALAPPDATA%\MediaTranscriber\data\yt-dlp`)에서 찾고, 없으면 건너뛴다.
//! 설치 자체는 `cargo test ytdlp_real_install -- --ignored` 로 검증한다.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::extract::extract_audio;
use super::ytdlp::{UrlAuth, UrlError, YtDlp};
use crate::config::AppPaths;
use crate::media::FfmpegTools;

pub(crate) fn ytdlp() -> Option<YtDlp> {
    let dir = AppPaths::resolve()?.ytdlp_dir;
    match YtDlp::locate(Some(&dir)) {
        Ok(y) => Some(y),
        Err(_) => {
            eprintln!("yt-dlp 없음 - 건너뜀 (cargo test ytdlp_real_install -- --ignored)");
            None
        }
    }
}

pub(crate) fn ffmpeg_exe() -> PathBuf {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\WinGet\Links"));
    }
    dirs.into_iter().map(|d| d.join("ffmpeg.exe")).find(|p| p.is_file()).expect("ffmpeg.exe")
}

fn run_ffmpeg(args: &[&str], out: &Path) {
    let ok = Command::new(ffmpeg_exe())
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "테스트 미디어 생성 실패: {}", out.display());
}

/// 사인파 소리가 있는 짧은 영상 (seconds 초)
pub(crate) fn make_video(out: &Path, seconds: u32) {
    let d = seconds.to_string();
    run_ffmpeg(
        &[
            "-f", "lavfi", "-i", &format!("testsrc=duration={d}:size=160x120:rate=10"),
            "-f", "lavfi", "-i", &format!("sine=frequency=440:duration={d}"),
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", "-movflags", "+faststart",
        ],
        out,
    );
}

/// 색인(moov)이 파일 끝에 있는 MP4 (faststart 아님 — 인코더 기본값이라 실제로 흔함)
/// 아주 작은 파일은 ffmpeg 가 버퍼에 다 담아 파이프로도 읽히므로 충분히 크게 만든다.
pub(crate) fn make_video_index_at_end(out: &Path, seconds: u32) {
    let d = seconds.to_string();
    run_ffmpeg(
        &[
            "-f", "lavfi", "-i", &format!("testsrc2=duration={d}:size=640x360:rate=25"),
            "-f", "lavfi", "-i", &format!("sine=frequency=440:duration={d}"),
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest",
        ],
        out,
    );
}

/// 디렉터리의 파일을 내려주는 최소 HTTP 서버. 반환: http://127.0.0.1:포트
pub(crate) fn serve_dir(dir: PathBuf) -> String {
    serve_dir_with(dir, None)
}

/// 본문을 16KB 마다 `delay` 씩 쉬면서 보내는 느린 서버 (취소 테스트용)
pub(crate) fn serve_dir_slow(dir: PathBuf, delay: Duration) -> String {
    serve_dir_with(dir, Some(delay))
}

fn serve_dir_with(dir: PathBuf, delay: Option<Duration>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    return;
                }
                loop {
                    let mut l = String::new();
                    if reader.read_line(&mut l).unwrap_or(0) == 0 || l.trim().is_empty() {
                        break;
                    }
                }
                let mut parts = request.split_whitespace();
                let method = parts.next().unwrap_or("GET").to_owned();
                let path = parts.next().unwrap_or("/").split('?').next().unwrap_or("/").trim_start_matches('/').to_owned();
                let file = dir.join(&path);
                let (status, body) = match std::fs::read(&file) {
                    Ok(b) if !path.contains("..") => ("200 OK", b),
                    _ => ("404 Not Found", b"not found".to_vec()),
                };
                let ctype = match file.extension().and_then(|e| e.to_str()) {
                    Some("mp4") => "video/mp4",
                    Some("m3u8") => "application/vnd.apple.mpegurl",
                    Some("ts") => "video/mp2t",
                    Some("html") => "text/html; charset=utf-8",
                    _ => "application/octet-stream",
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nAccept-Ranges: none\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                if method != "HEAD" {
                    match delay {
                        None => {
                            let _ = stream.write_all(&body);
                        }
                        Some(d) => {
                            for chunk in body.chunks(16 * 1024) {
                                if stream.write_all(chunk).is_err() {
                                    break; // 상대가 끊음 (취소)
                                }
                                std::thread::sleep(d);
                            }
                        }
                    }
                }
            });
        }
    });
    base
}

pub(crate) fn fixture_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("media_transcriber_url_{name}_{}", std::process::id())).join("영상 서버");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn wav_seconds(tools: &FfmpegTools, wav: &Path) -> f64 {
    tools.probe(wav).expect("probe wav").duration.expect("duration").as_secs_f64()
}

#[test]
#[ignore = "네트워크로 약 23MB 를 내려받는다"]
fn ytdlp_real_install() {
    let dir = AppPaths::resolve().expect("paths").ytdlp_dir;
    let (path, version) = super::installer::install(&dir, &AtomicBool::new(false), |_, _| {}).expect("install");
    eprintln!("설치: {} ({version})", path.display());
    assert!(path.is_file());
    assert!(!dir.join("yt-dlp.exe.part").exists());
}

#[test]
fn direct_mp4_is_discovered_and_audio_extracted() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("mp4");
    make_video(&dir.join("lecture.mp4"), 3);
    let base = serve_dir(dir.clone());
    let url = format!("{base}/lecture.mp4");

    let mut videos = y.discover(&url, &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    assert_eq!(videos.len(), 1);
    super::fill_missing_durations(&mut videos, &tools, &AtomicBool::new(false));
    let v = &videos[0];
    assert_eq!(v.playlist_index, None);
    let d = v.duration.expect("ffprobe 로 길이를 보완해야 함").as_secs_f64();
    assert!((d - 3.0).abs() < 0.3, "{d}");
    assert!(v.unsupported_reason().is_none());
    eprintln!("[mp4] {v:?}");

    let out = dir.join("out.wav");
    let mut progress = Vec::new();
    extract_audio(&y, &tools, v, &UrlAuth::None, &out, &AtomicBool::new(false), |p| progress.push(p)).expect("extract");
    let secs = wav_seconds(&tools, &out);
    assert!((secs - 3.0).abs() < 0.3, "wav {secs}s");
    assert!(progress.windows(2).all(|w| w[1] >= w[0]) && progress.last().copied() == Some(1.0), "{progress:?}");
    let info = tools.probe(&out).expect("probe");
    assert_eq!(info.audio.sample_rate, Some(16000));
    assert_eq!(info.audio.channels, Some(1));
    assert_eq!(info.audio.codec, "pcm_s16le");
    assert!(!info.has_video);
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// 색인(moov)이 파일 끝에 있는 MP4 (faststart 아님): 파이프로는 못 읽으므로 임시 파일로 받아 추출해야 한다.
/// 받은 영상 임시 파일은 추출 후 남지 않아야 한다.
#[test]
fn mp4_with_index_at_end_falls_back_to_temp_file() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("moov_end");
    make_video_index_at_end(&dir.join("slow_index.mp4"), 5);
    let base = serve_dir(dir.clone());
    let mut videos = y.discover(&format!("{base}/slow_index.mp4"), &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    super::fill_missing_durations(&mut videos, &tools, &AtomicBool::new(false));

    let work = dir.parent().expect("parent").join("work");
    std::fs::create_dir_all(&work).expect("mkdir");
    let out = work.join("out.wav");
    // 파이프만으로는 실패해야 이 테스트가 대체 경로를 실제로 검사하는 것
    let url = videos[0].source_url.clone();
    let pipe = super::extract::pipe_extract(&y, &tools, &videos[0], &UrlAuth::None, &url, &out, &AtomicBool::new(false), &mut |_| {});
    assert!(matches!(pipe, Ok(super::extract::PipeOutcome::NeedsFile(_))), "파이프 경로는 이 형식을 읽지 못해야 함");
    let _ = std::fs::remove_file(&out);
    let mut progress = Vec::new();
    extract_audio(&y, &tools, &videos[0], &UrlAuth::None, &out, &AtomicBool::new(false), |p| progress.push(p)).expect("extract");
    let secs = wav_seconds(&tools, &out);
    assert!((secs - 5.0).abs() < 0.3, "wav {secs}s");
    assert!(progress.windows(2).all(|w| w[1] >= w[0]) && progress.last().copied() == Some(1.0), "{progress:?}");
    let left: Vec<_> = std::fs::read_dir(&work).expect("dir").filter_map(Result::ok).map(|e| e.file_name()).collect();
    assert_eq!(left, vec![std::ffi::OsString::from("out.wav")], "받은 영상 임시 파일이 남으면 안 됨");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn hls_m3u8_is_discovered_and_audio_extracted() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("hls");
    let src = dir.join("src.mp4");
    make_video(&src, 6);
    let hls = dir.join("hls");
    std::fs::create_dir_all(&hls).expect("mkdir");
    run_ffmpeg(
        &["-i", src.to_str().expect("utf8"), "-c", "copy", "-hls_time", "2", "-hls_playlist_type", "vod", "-f", "hls"],
        &hls.join("index.m3u8"),
    );
    let base = serve_dir(dir.clone());
    let url = format!("{base}/hls/index.m3u8?token=SECRET123");

    let videos = y.discover(&url, &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    assert_eq!(videos.len(), 1);
    eprintln!("[hls] {:?}", videos[0]);
    assert!(videos[0].ext.contains("HLS"), "ext {}", videos[0].ext);

    let out = dir.join("out.wav");
    extract_audio(&y, &tools, &videos[0], &UrlAuth::None, &out, &AtomicBool::new(false), |_| {}).expect("extract");
    let secs = wav_seconds(&tools, &out);
    assert!((secs - 6.0).abs() < 0.5, "wav {secs}s");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn page_with_several_videos_lists_all_and_extracts_the_chosen_one() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("page");
    make_video(&dir.join("week1.mp4"), 2);
    // 고른 영상은 색인이 끝에 있는 MP4 → 재생목록 페이지에서 임시 파일 대체 경로도 확인
    make_video_index_at_end(&dir.join("week2.mp4"), 4);
    make_video(&dir.join("week3.mp4"), 6);
    std::fs::write(
        dir.join("course.html"),
        r#"<!doctype html><html><head><title>강의 페이지</title></head><body>
<h1>1주차</h1><video controls><source src="week1.mp4" type="video/mp4"></video>
<h1>2주차</h1><video controls><source src="week2.mp4" type="video/mp4"></video>
<h1>3주차</h1><video controls><source src="week3.mp4" type="video/mp4"></video>
</body></html>"#,
    )
    .expect("html");
    let base = serve_dir(dir.clone());
    let url = format!("{base}/course.html");

    let videos = y.discover(&url, &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    for v in &videos {
        eprintln!("[page] #{:?} {} {:?}", v.playlist_index, v.title, v.duration);
    }
    assert_eq!(videos.len(), 3, "페이지의 영상 3개를 모두 찾아야 함");
    assert!(videos.iter().all(|v| v.playlist_index.is_some()));

    // 사용자가 두 번째(4초) 영상만 골랐다고 가정
    let chosen = &videos[1];
    let work = dir.parent().expect("parent").join("work");
    std::fs::create_dir_all(&work).expect("mkdir");
    let out = work.join("chosen.wav");
    extract_audio(&y, &tools, chosen, &UrlAuth::None, &out, &AtomicBool::new(false), |_| {}).expect("extract");
    let secs = wav_seconds(&tools, &out);
    assert!((secs - 4.0).abs() < 0.4, "선택한 영상(4초)만 추출되어야 함: {secs}s");
    assert_eq!(std::fs::read_dir(&work).expect("dir").count(), 1, "받은 영상 임시 폴더가 남으면 안 됨");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// 강의 플레이어(Xinics) 페이지: yt-dlp 가 모르는 형식 → 플레이어 설정 XML 에서 MP4 를 찾아 직접 추출
#[test]
fn lecture_player_page_is_discovered_and_extracted() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("xinics");
    let media_dir = dir.join("contents").join("media_files");
    std::fs::create_dir_all(&media_dir).expect("mkdir");
    make_video(&media_dir.join("ssmovie.mp4"), 4);
    let base = serve_dir(dir.clone());
    std::fs::write(
        dir.join("em.html"),
        "<html><head><title>viewer</title></head><body><div id='viewer-root'></div><script>\n\
         var playerType = 'Uniplayer';\n\
         var contentUri = '/viewer/ssplayer/uniplayer_support/content.php?content_id=abc123';\n\
         </script></body></html>",
    )
    .expect("html");
    let xml_dir = dir.join("viewer").join("ssplayer").join("uniplayer_support");
    std::fs::create_dir_all(&xml_dir).expect("mkdir");
    std::fs::write(
        xml_dir.join("content.php"),
        format!(
            "<?xml version=\"1.0\"?><content><content_playing_info><content_id>abc123</content_id><content_type>video1</content_type>\
             <content_duration>4.0</content_duration><main_media><desktop><html5><method>progressive</method>\
             <media_uri>{base}/contents/media_files/ssmovie.mp4</media_uri></html5></desktop></main_media></content_playing_info>\
             <content_metadata><title><![CDATA[3주차 강의]]></title></content_metadata></content>"
        ),
    )
    .expect("xml");

    let page = format!("{base}/em.html?contentId=SECRETTOKEN&userId=SECRETUSER");
    let videos = super::discover(&y, &page, &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    assert_eq!(videos.len(), 1);
    let v = &videos[0];
    assert_eq!(v.title, "3주차 강의");
    assert_eq!(v.duration, Some(Duration::from_secs(4)));
    assert_eq!(v.extractor, "Xinics");
    assert_eq!(v.direct.as_ref().map(|d| d.referer.clone()), Some(format!("{base}/")));

    let work = dir.parent().expect("parent").join("work");
    std::fs::create_dir_all(&work).expect("mkdir");
    let out = work.join("lecture.wav");
    let mut progress = Vec::new();
    extract_audio(&y, &tools, v, &UrlAuth::None, &out, &AtomicBool::new(false), |p| progress.push(p)).expect("extract");
    let secs = wav_seconds(&tools, &out);
    assert!((secs - 4.0).abs() < 0.3, "wav {secs}s");
    assert_eq!(progress.last().copied(), Some(1.0));
    assert_eq!(std::fs::read_dir(&work).expect("dir").count(), 1, "영상 파일을 저장하지 않아야 함");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// LMS 페이지 안의 iframe 에 강의 플레이어가 들어 있는 경우 (예: 강의실 게시물 → 커먼즈 플레이어)
#[test]
fn lecture_player_inside_iframe_is_found() {
    let Some(y) = ytdlp() else { return };
    let dir = fixture_dir("iframe_xinics");
    let media_dir = dir.join("contents");
    std::fs::create_dir_all(&media_dir).expect("mkdir");
    make_video(&media_dir.join("ssmovie.mp4"), 3);
    let base = serve_dir(dir.clone());
    std::fs::write(
        dir.join("course.html"),
        "<html><head><title>강의실</title></head><body><h1>3주차</h1>\
         <iframe width='800' src='/em/abc123?userId=SECRETUSER'></iframe></body></html>",
    )
    .expect("html");
    std::fs::create_dir_all(dir.join("em")).expect("mkdir");
    std::fs::write(
        dir.join("em").join("abc123"),
        "<script>var contentUri = '/viewer/content.php?content_id=abc123';</script>",
    )
    .expect("em");
    std::fs::create_dir_all(dir.join("viewer")).expect("mkdir");
    std::fs::write(
        dir.join("viewer").join("content.php"),
        format!(
            "<content><content_playing_info><content_id>abc123</content_id><content_duration>3</content_duration>\
             <main_media><desktop><html5><method>progressive</method><media_uri>{base}/contents/ssmovie.mp4</media_uri>\
             </html5></desktop></main_media></content_playing_info><content_metadata><title>3주차 강의</title></content_metadata></content>"
        ),
    )
    .expect("xml");

    let videos = super::discover(&y, &format!("{base}/course.html"), &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    assert_eq!(videos.len(), 1);
    assert_eq!(videos[0].title, "3주차 강의");
    assert_eq!(videos[0].extractor, "Xinics");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// 플레이어 스크립트·JSON 설정 안에만 미디어 주소가 있는 페이지 (yt-dlp 일반 추출기가 못 찾는 형태)
#[test]
fn media_urls_in_player_config_are_found_and_extracted() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("sniff");
    let src = dir.join("src.mp4");
    make_video(&src, 5);
    let hls = dir.join("vod").join("lec02");
    std::fs::create_dir_all(&hls).expect("mkdir");
    run_ffmpeg(
        &["-i", src.to_str().expect("utf8"), "-c", "copy", "-hls_time", "2", "-hls_playlist_type", "vod", "-f", "hls"],
        &hls.join("playlist.m3u8"),
    );
    make_video(&dir.join("vod").join("lec01.mp4"), 2);
    let base = serve_dir(dir.clone());
    std::fs::write(
        dir.join("player.html"),
        "<html><head><title>온라인 강의</title></head><body><div id='p'></div><script>\n\
         var conf = {\"items\":[{\"src\":\"\\/vod\\/lec01.mp4\"},{\"src\":\"\\/vod\\/lec02\\/playlist.m3u8\"}]};\n\
         myPlayer.init('p', conf);</script></body></html>",
    )
    .expect("html");

    let videos = super::discover(&y, &format!("{base}/player.html?sid=SECRET"), &UrlAuth::None, &AtomicBool::new(false))
        .expect("discover");
    assert_eq!(videos.len(), 2, "{videos:?}");
    assert!(videos.iter().all(|v| v.direct.is_some()));
    assert_eq!(videos[0].title, "온라인 강의 (1)");
    let hls_video = videos.iter().find(|v| v.ext == "HLS").expect("hls");
    assert_ne!(videos[0].key(), videos[1].key());

    let mut videos = videos.clone();
    super::fill_missing_durations(&mut videos, &tools, &AtomicBool::new(false));
    assert!(videos.iter().all(|v| v.duration.is_some()), "길이를 보완해야 함");

    let out = dir.join("hls.wav");
    extract_audio(&y, &tools, hls_video, &UrlAuth::None, &out, &AtomicBool::new(false), |_| {}).expect("extract");
    let secs = wav_seconds(&tools, &out);
    assert!((secs - 5.0).abs() < 0.5, "wav {secs}s");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// iframe 안의 일반 영상 페이지: yt-dlp 가 바깥 페이지를 Referer 로 보내며 찾고, 받을 때도 같은 옵션을 쓴다
#[test]
fn plain_video_page_inside_iframe_is_found_with_referer() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("iframe_plain");
    make_video(&dir.join("clip.mp4"), 3);
    let base = serve_dir(dir.clone());
    std::fs::write(dir.join("inner.html"), "<html><head><title>inner</title></head><body><video src='clip.mp4'></video></body></html>")
        .expect("inner");
    std::fs::write(dir.join("outer.html"), "<html><body><p>강의 안내</p><iframe src='inner.html'></iframe></body></html>")
        .expect("outer");

    let videos = super::discover(&y, &format!("{base}/outer.html"), &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    assert_eq!(videos.len(), 1, "{videos:?}");
    let v = &videos[0];
    assert!(v.source_url.ends_with("/inner.html"), "{}", v.source_url);
    assert_eq!(v.request.referer.as_deref(), Some(format!("{base}/").as_str()));
    let out = dir.join("clip.wav");
    extract_audio(&y, &tools, v, &UrlAuth::None, &out, &AtomicBool::new(false), |_| {}).expect("extract");
    assert!((wav_seconds(&tools, &out) - 3.0).abs() < 0.4);
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// 보안 플레이어(Kollus) 페이지: 열어 보지 않고 DRM 으로 안내한다
#[test]
fn secure_player_page_is_reported_as_drm() {
    let Some(y) = ytdlp() else { return };
    let dir = fixture_dir("kollus");
    let base = serve_dir(dir.clone());
    std::fs::write(
        dir.join("week5.html"),
        "<html><body><iframe src='https://v.kr.kollus.com/s?jwt=SECRETJWT&custom_key=abc'></iframe></body></html>",
    )
    .expect("html");
    let err = super::discover(&y, &format!("{base}/week5.html"), &UrlAuth::None, &AtomicBool::new(false)).expect_err("drm");
    assert!(matches!(err, UrlError::Drm), "{err:?}");
    assert!(!err.to_string().contains("SECRET"));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

fn dirs_for_download(dir: &Path) -> (PathBuf, PathBuf) {
    let out = dir.parent().expect("parent").join("저장 폴더");
    let temp = dir.parent().expect("parent").join("임시");
    std::fs::create_dir_all(&temp).expect("mkdir");
    (out, temp)
}

fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(true)
}

/// 영상 저장: 여러 영상 페이지에서 고른 것만, 제목으로 저장, 같은 이름은 덮어쓰지 않음, 임시 폴더 정리
#[test]
fn chosen_video_is_saved_with_title_and_unique_name() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("dl_page");
    make_video(&dir.join("a.mp4"), 2);
    make_video(&dir.join("b.mp4"), 3);
    std::fs::write(dir.join("p.html"), "<html><head><title>강의</title></head><body><video src='a.mp4'></video><video src='b.mp4'></video></body></html>")
        .expect("html");
    let base = serve_dir(dir.clone());
    let mut videos = y.discover(&format!("{base}/p.html"), &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    assert_eq!(videos.len(), 2);
    let mut v = videos.remove(1);
    v.title = "2주차: 영상".to_owned();
    let (out, temp) = dirs_for_download(&dir);

    let mut progress = Vec::new();
    let first = super::download::download_video(&y, &tools, &v, &UrlAuth::None, &out, &temp, &AtomicBool::new(false), |_, p| progress.push(p))
        .expect("download");
    assert_eq!(first.file_name().and_then(|n| n.to_str()), Some("2주차_ 영상.mp4"));
    let info = tools.probe(&first).expect("probe");
    assert!(info.has_video);
    let secs = info.duration.expect("duration").as_secs_f64();
    assert!((secs - 3.0).abs() < 0.4, "고른 영상(3초)이어야 함: {secs}");
    assert_eq!(progress.last().copied(), Some(1.0));
    assert!(progress.windows(2).all(|w| w[1] >= w[0]), "{progress:?}");

    let second = super::download::download_video(&y, &tools, &v, &UrlAuth::None, &out, &temp, &AtomicBool::new(false), |_, _| {})
        .expect("download 2");
    assert_eq!(second.file_name().and_then(|n| n.to_str()), Some("2주차_ 영상 (1).mp4"));
    assert!(first.is_file(), "기존 파일은 그대로");
    assert!(is_empty_dir(&temp), "임시 폴더가 비어야 함");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// 최고 화질: 영상과 소리가 따로 있는 스트림은 둘 다 받아 하나로 합친다
#[test]
fn best_quality_merges_separate_video_and_audio() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("dl_merge");
    let src = dir.join("src.mp4");
    make_video(&src, 4);
    // 영상 스트림 2개(저화질·고화질) + 오디오 스트림 1개인 HLS (오디오는 별도 그룹)
    let hls = dir.join("hls");
    std::fs::create_dir_all(&hls).expect("mkdir");
    let ok = Command::new(super::tests::ffmpeg_exe())
        .current_dir(&hls)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&src)
        .args(["-map", "0:v", "-map", "0:v", "-map", "0:a"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .args(["-s:v:0", "160x120", "-b:v:0", "100k", "-s:v:1", "320x240", "-b:v:1", "400k"])
        .args(["-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod", "-master_pl_name", "master.m3u8"])
        .args(["-var_stream_map", "v:0,agroup:aud v:1,agroup:aud a:0,agroup:aud,default:yes"])
        .arg("stream_%v.m3u8")
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "HLS 만들기");
    let base = serve_dir(dir.clone());
    let videos = y.discover(&format!("{base}/hls/master.m3u8"), &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    let (out, temp) = dirs_for_download(&dir);
    let mut merging = false;
    let saved = super::download::download_video(&y, &tools, &videos[0], &UrlAuth::None, &out, &temp, &AtomicBool::new(false), |stage, _| {
        merging |= stage == super::download::DownloadStage::Merging;
    })
    .expect("download");
    eprintln!("[merge] {}", saved.display());
    let probe = Command::new(super::tests::ffmpeg_exe().with_file_name("ffprobe.exe"))
        .args(["-v", "error", "-show_entries", "stream=codec_type,width", "-of", "csv=p=0"])
        .arg(&saved)
        .output()
        .expect("ffprobe");
    let streams = String::from_utf8_lossy(&probe.stdout).to_string();
    eprintln!("[merge] streams: {streams}");
    assert!(streams.contains("video,320"), "가장 좋은(320x240) 영상이어야 함: {streams}");
    assert!(streams.contains("audio"), "소리도 합쳐져야 함: {streams}");
    assert!(merging, "합치기 단계 보고");
    assert!(is_empty_dir(&temp));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// 플레이어 설정에서 찾은 직접 주소도 저장되고, 취소하면 저장 폴더·임시 폴더에 아무것도 남지 않는다
#[test]
fn direct_media_is_saved_and_cancel_leaves_nothing() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("dl_direct");
    make_video(&dir.join("lec.mp4"), 2);
    make_video(&dir.join("long.mp4"), 60);
    let base = serve_dir(dir.clone());
    std::fs::write(dir.join("player.html"), "<html><head><title>직접 강의</title></head><script>var c={\"file\":\"/lec.mp4\"};</script></html>")
        .expect("html");
    let videos = super::discover(&y, &format!("{base}/player.html"), &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    assert!(videos[0].direct.is_some());
    let (out, temp) = dirs_for_download(&dir);
    let saved = super::download::download_video(&y, &tools, &videos[0], &UrlAuth::None, &out, &temp, &AtomicBool::new(false), |_, _| {})
        .expect("download");
    assert_eq!(saved.file_name().and_then(|n| n.to_str()), Some("직접 강의.mp4"));

    // 취소: 느린 서버에서 받는 도중
    let slow = serve_dir_slow(dir.clone(), Duration::from_millis(100));
    let mut v = videos[0].clone();
    v.title = "취소될 영상".to_owned();
    if let Some(d) = v.direct.as_mut() {
        d.url = format!("{slow}/long.mp4");
    }
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let c = std::sync::Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        c.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let err = super::download::download_video(&y, &tools, &v, &UrlAuth::None, &out, &temp, &cancel, |_, _| {}).expect_err("cancel");
    assert!(matches!(err, UrlError::Cancelled), "{err:?}");
    std::thread::sleep(Duration::from_millis(500));
    assert!(is_empty_dir(&temp), "취소 후 임시 폴더가 비어야 함");
    assert!(!out.join("취소될 영상.mp4").exists());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn errors_are_reported_without_tokens() {
    let Some(y) = ytdlp() else { return };
    let dir = fixture_dir("err");
    let base = serve_dir(dir.clone());
    let err = y
        .discover(&format!("{base}/missing.mp4?token=SECRET"), &UrlAuth::None, &AtomicBool::new(false))
        .expect_err("404");
    let msg = err.to_string();
    eprintln!("[err] {msg}");
    assert!(!msg.contains("SECRET"), "{msg}");
    // 옵션처럼 보이는 입력은 실행 전에 거부
    assert!(matches!(
        y.discover("--exec calc", &UrlAuth::None, &AtomicBool::new(false)),
        Err(UrlError::InvalidUrl(_))
    ));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn extraction_can_be_cancelled() {
    let Some(y) = ytdlp() else { return };
    let tools = FfmpegTools::locate(None).expect("ffmpeg");
    let dir = fixture_dir("cancel");
    make_video(&dir.join("long.mp4"), 60);
    // 16KB 마다 0.1초 쉬는 서버 → 전송에 수 초가 걸린다
    let base = serve_dir_slow(dir.clone(), Duration::from_millis(100));
    let videos = y.discover(&format!("{base}/long.mp4"), &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let timer = std::sync::Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        timer.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let out = dir.join("out.wav");
    let started = std::time::Instant::now();
    let result = extract_audio(&y, &tools, &videos[0], &UrlAuth::None, &out, &cancel, |_| {});
    assert!(matches!(result, Err(UrlError::Cancelled)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(10), "취소 후 바로 끝나야 함: {:?}", started.elapsed());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}


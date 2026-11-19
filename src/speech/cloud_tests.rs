//! Cloud 클라이언트 통합 테스트 (로컬 모의 HTTP 서버 사용, 실제 OpenAI 호출 없음).
//!
//! 검증 항목: 인증 헤더, multipart 필드(model/response_format/file), 구간 분할 업로드,
//! 429 재시도(Retry-After), 할당량 부족 시 재시도하지 않음, 401 처리, 연결 실패, 취소.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;

use super::cloud::{CloudClient, CloudError};
use crate::config::{CloudProvider, Language};
use crate::config::secret::ApiKey;
use crate::config::settings::{CloudSettings, ProviderSettings};
use crate::media::FfmpegTools;

const TEST_KEY: &str = "fake-mock-key-1234567890";

struct Recorded {
    request_line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// (상태 코드, 추가 헤더, JSON 본문)
type MockResponse = (u16, Vec<(&'static str, String)>, String);

/// 미리 정한 응답을 순서대로 돌려주는 모의 서버. 반환: (base_url, 기록 수신 채널)
fn mock_server(responses: Vec<MockResponse>) -> (String, mpsc::Receiver<Recorded>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for (status, headers, body) in responses {
            let Ok((stream, _)) = listener.accept() else { return };
            let Some(recorded) = read_request(&stream) else { return };
            let _ = tx.send(recorded);
            let mut stream = stream;
            let mut response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (k, v) in headers {
                response.push_str(&format!("{k}: {v}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&body);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{addr}/v1"), rx)
}

fn read_request(stream: &TcpStream) -> Option<Recorded> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    Some(Recorded { request_line: request_line.trim_end().to_owned(), headers, body })
}

fn key() -> ApiKey {
    ApiKey::new(TEST_KEY).expect("key")
}

fn settings(base_url: &str, chunk_seconds: u32) -> CloudSettings {
    CloudSettings {
        provider: CloudProvider::OpenAi,
        openai: ProviderSettings { base_url: base_url.to_owned(), model: "gpt-transcribe".to_owned() },
        chunk_seconds,
        ..CloudSettings::default()
    }
}

fn gemini_settings(base_url: &str) -> CloudSettings {
    CloudSettings {
        provider: CloudProvider::Gemini,
        gemini: ProviderSettings { base_url: base_url.to_owned(), model: "gemini-3.5-flash-lite".to_owned() },
        chunk_seconds: 600,
        ..CloudSettings::default()
    }
}

fn ffmpeg_exe() -> PathBuf {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\WinGet\Links"));
    }
    dirs.into_iter().map(|d| d.join("ffmpeg.exe")).find(|p| p.is_file()).expect("ffmpeg.exe")
}

fn sine_file(dir: &Path, name: &str, seconds: u32) -> PathBuf {
    let out = dir.join(name);
    let status = Command::new(ffmpeg_exe())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={seconds}"))
        .args(["-c:a", "libmp3lame"])
        .arg(&out)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    out
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("media_transcriber_cloud_{name}_{}", std::process::id()))
        .join("클라우드 테스트");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[test]
fn validate_key_sends_bearer_and_maps_401() {
    let (url, rx) = mock_server(vec![
        (200, vec![], r#"{"object":"list","data":[]}"#.to_owned()),
        (401, vec![], r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#.to_owned()),
    ]);
    let client = CloudClient::new(CloudProvider::OpenAi, &url).expect("client");
    client.validate_key(&key()).expect("valid");
    let req = rx.recv().expect("request");
    assert_eq!(req.request_line, "GET /v1/models HTTP/1.1");
    assert_eq!(req.header("authorization"), Some(format!("Bearer {TEST_KEY}").as_str()));

    let err = client.validate_key(&key()).expect_err("401");
    assert!(matches!(err, CloudError::InvalidApiKey));
    // 오류 메시지에 키가 노출되지 않는다
    assert!(!err.to_string().contains(TEST_KEY));
}

#[test]
fn long_file_is_uploaded_in_chunks_and_joined() {
    let Ok(tools) = FfmpegTools::locate(None) else { return };
    let dir = temp_dir("chunks");
    let input = sine_file(&dir, "긴 녹음 파일.mp3", 70);
    let info = tools.probe(&input).expect("probe");

    let (url, rx) = mock_server(vec![
        (200, vec![], r#"{"text":"첫 번째 구간"}"#.to_owned()),
        (200, vec![], r#"{"text":"두 번째 구간"}"#.to_owned()),
        (200, vec![], r#"{"text":"세 번째 구간"}"#.to_owned()),
    ]);
    let client = CloudClient::new(CloudProvider::OpenAi, &url).expect("client");
    let mut progress = Vec::new();
    let transcript = client
        .transcribe_file(
            &tools,
            &input,
            info.duration,
            &key(),
            &settings(&url, 30), // 70초 → 30 + 30 + 10
            Language::Korean,
            &AtomicBool::new(false),
            |p| progress.push(p),
        )
        .expect("transcribe");

    assert_eq!(transcript.plain_text(), "첫 번째 구간\n두 번째 구간\n세 번째 구간\n");
    assert_eq!(transcript.segments[1].start, Duration::from_secs(30));
    assert_eq!(transcript.segments[2].start, Duration::from_secs(60));
    assert!((progress.last().copied().unwrap_or(0.0) - 1.0).abs() < 1e-6);

    for i in 0..3 {
        let req = rx.recv().expect("request");
        assert_eq!(req.request_line, "POST /v1/audio/transcriptions HTTP/1.1");
        assert_eq!(req.header("authorization"), Some(format!("Bearer {TEST_KEY}").as_str()));
        assert!(req.header("content-type").unwrap_or_default().starts_with("multipart/form-data"));
        let body = req.body_text();
        assert!(body.contains("name=\"model\"\r\n\r\ngpt-transcribe"), "chunk {i}");
        assert!(body.contains("name=\"response_format\"\r\n\r\njson"));
        assert!(body.contains("name=\"file\"; filename=\"audio.mp3\""));
        assert!(body.contains("Content-Type: audio/mpeg"));
        // gpt-transcribe 에는 language 를 보내지 않는다 (문서상 languages 배열 사용)
        assert!(!body.contains("name=\"language\""));
        // 구간 MP3 는 25MB 제한보다 훨씬 작아야 한다 (30초 ≈ 180KB)
        assert!(req.body.len() < 1024 * 1024, "chunk {i} size {}", req.body.len());
    }
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn language_is_sent_for_whisper_model() {
    let Ok(tools) = FfmpegTools::locate(None) else { return };
    let dir = temp_dir("lang");
    let input = sine_file(&dir, "a.mp3", 3);
    let (url, rx) = mock_server(vec![(200, vec![], r#"{"text":"hello"}"#.to_owned())]);
    let mut s = settings(&url, 600);
    s.openai.model = "whisper-1".to_owned();
    CloudClient::new(CloudProvider::OpenAi, &url)
        .expect("client")
        .transcribe_file(&tools, &input, None, &key(), &s, Language::English, &AtomicBool::new(false), |_| {})
        .expect("transcribe");
    let body = rx.recv().expect("request").body_text();
    assert!(body.contains("name=\"language\"\r\n\r\nen"));
    assert!(body.contains("name=\"model\"\r\n\r\nwhisper-1"));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn rate_limit_is_retried_but_quota_is_not() {
    let Ok(tools) = FfmpegTools::locate(None) else { return };
    let dir = temp_dir("retry");
    let input = sine_file(&dir, "a.mp3", 3);

    // 429 (Retry-After: 0) → 재시도 → 성공
    let (url, rx) = mock_server(vec![
        (429, vec![("Retry-After", "0".to_owned())], r#"{"error":{"message":"Rate limit reached","type":"requests"}}"#.to_owned()),
        (200, vec![], r#"{"text":"ok"}"#.to_owned()),
    ]);
    let t = CloudClient::new(CloudProvider::OpenAi, &url)
        .expect("client")
        .transcribe_file(&tools, &input, None, &key(), &settings(&url, 600), Language::Auto, &AtomicBool::new(false), |_| {})
        .expect("retried");
    assert_eq!(t.plain_text(), "ok\n");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rx.try_iter().count(), 2, "429 후 한 번 재시도");

    // 429 할당량 부족 → 재시도하지 않고 즉시 실패
    let (url, rx) = mock_server(vec![
        (429, vec![], r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#.to_owned()),
        (200, vec![], r#"{"text":"should not be used"}"#.to_owned()),
    ]);
    let err = CloudClient::new(CloudProvider::OpenAi, &url)
        .expect("client")
        .transcribe_file(&tools, &input, None, &key(), &settings(&url, 600), Language::Auto, &AtomicBool::new(false), |_| {})
        .expect_err("quota");
    assert!(matches!(err, CloudError::QuotaExceeded(_)));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rx.try_iter().count(), 1, "할당량 오류는 재시도하지 않아야 함");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn connection_failure_is_network_error() {
    // 바인드 후 바로 닫아 아무도 듣지 않는 포트
    let port = TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let client = CloudClient::new(CloudProvider::OpenAi, &format!("http://127.0.0.1:{port}/v1")).expect("client");
    let err = client.validate_key(&key()).expect_err("no server");
    assert!(matches!(err, CloudError::Network(_)), "{err:?}");
}

#[test]
fn cancelled_before_upload() {
    let Ok(tools) = FfmpegTools::locate(None) else { return };
    let dir = temp_dir("cancel");
    let input = sine_file(&dir, "a.mp3", 3);
    let (url, rx) = mock_server(vec![(200, vec![], r#"{"text":"x"}"#.to_owned())]);
    let err = CloudClient::new(CloudProvider::OpenAi, &url)
        .expect("client")
        .transcribe_file(&tools, &input, None, &key(), &settings(&url, 600), Language::Auto, &AtomicBool::new(true), |_| {})
        .expect_err("cancelled");
    assert!(matches!(err, CloudError::Cancelled));
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "취소 시 아무것도 전송하지 않음");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

// ---------------- Gemini ----------------

#[test]
fn gemini_validate_key_uses_header_not_url() {
    let (url, rx) = mock_server(vec![
        (200, vec![], r#"{"models":[]}"#.to_owned()),
        (
            400,
            vec![],
            r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#.to_owned(),
        ),
    ]);
    let client = CloudClient::new(CloudProvider::Gemini, &url).expect("client");
    client.validate_key(&key()).expect("valid");
    let req = rx.recv().expect("request");
    assert_eq!(req.request_line, "GET /v1/models HTTP/1.1");
    assert_eq!(req.header("x-goog-api-key"), Some(TEST_KEY));
    // 키가 URL 쿼리에 들어가지 않는다
    assert!(!req.request_line.contains(TEST_KEY));
    assert!(req.header("authorization").is_none());

    let err = client.validate_key(&key()).expect_err("invalid");
    assert!(matches!(err, CloudError::InvalidApiKey), "{err:?}");
}

#[test]
fn gemini_generate_content_with_inline_audio() {
    let Ok(tools) = FfmpegTools::locate(None) else { return };
    let dir = temp_dir("gemini");
    let input = sine_file(&dir, "제미나이 테스트.mp3", 3);
    let (url, rx) = mock_server(vec![(
        200,
        vec![],
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"안녕하세요.\n테스트입니다."}]},"finishReason":"STOP"}]}"#.to_owned(),
    )]);
    let transcript = CloudClient::new(CloudProvider::Gemini, &url)
        .expect("client")
        .transcribe_file(&tools, &input, None, &key(), &gemini_settings(&url), Language::Korean, &AtomicBool::new(false), |_| {})
        .expect("transcribe");
    assert_eq!(transcript.plain_text(), "안녕하세요.\n테스트입니다.\n");

    let req = rx.recv().expect("request");
    assert_eq!(req.request_line, "POST /v1/models/gemini-3.5-flash-lite:generateContent HTTP/1.1");
    assert_eq!(req.header("x-goog-api-key"), Some(TEST_KEY));
    assert!(req.header("content-type").unwrap_or_default().starts_with("application/json"));
    let body: serde_json::Value = serde_json::from_slice(&req.body).expect("json body");
    let parts = body.pointer("/contents/0/parts").and_then(|v| v.as_array()).expect("parts");
    let prompt = parts[0].get("text").and_then(|v| v.as_str()).expect("prompt");
    assert!(prompt.contains("Korean"));
    assert_eq!(parts[1].pointer("/inlineData/mimeType").and_then(|v| v.as_str()), Some("audio/mpeg"));
    let data = parts[1].pointer("/inlineData/data").and_then(|v| v.as_str()).expect("data");
    use base64::Engine as _;
    let audio = base64::engine::general_purpose::STANDARD.decode(data).expect("base64");
    // MP3 프레임 동기 또는 ID3 헤더로 시작해야 한다
    assert!(audio.starts_with(b"ID3") || (audio[0] == 0xFF && audio[1] & 0xE0 == 0xE0));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn gemini_safety_block_is_reported() {
    let Ok(tools) = FfmpegTools::locate(None) else { return };
    let dir = temp_dir("gemini_block");
    let input = sine_file(&dir, "a.mp3", 3);
    let (url, _rx) = mock_server(vec![(200, vec![], r#"{"promptFeedback":{"blockReason":"PROHIBITED_CONTENT"}}"#.to_owned())]);
    let err = CloudClient::new(CloudProvider::Gemini, &url)
        .expect("client")
        .transcribe_file(&tools, &input, None, &key(), &gemini_settings(&url), Language::Auto, &AtomicBool::new(false), |_| {})
        .expect_err("blocked");
    assert!(matches!(err, CloudError::Blocked(ref r) if r == "PROHIBITED_CONTENT"), "{err:?}");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

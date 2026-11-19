//! Cloud 음성 인식 클라이언트 (OpenAI / Google Gemini).
//!
//! 공통
//! - 입력을 일정 길이 구간으로 잘라 MP3(16 kHz mono 48 kbps)로 인코딩해 순서대로 올린다 (임시 파일 없음).
//! - 429(속도 제한)/5xx/네트워크 오류는 제한 횟수만큼 재시도한다. 인증 오류·할당량 소진은 재시도하지 않는다.
//! - 네트워크가 끊겨도 Local 로 몰래 전환하지 않는다. 실패로 보고한다.
//! - API Key 는 HTTP 헤더로만 전송하며 URL·로그·오류 메시지에 포함하지 않는다.
//!
//! OpenAI: `POST {base}/audio/transcriptions` (multipart, 파일당 25 MB)
//! Gemini: `POST {base}/models/{model}:generateContent` (오디오를 base64 inlineData 로, 요청당 20 MB)
//!
//! 모델명·주소·구간 길이는 [`CloudSettings`] 로 관리한다 (코드에 흩어 두지 않는다).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

use super::engine::{Segment, Transcript};
use crate::config::secret::ApiKey;
use crate::config::settings::{CloudSettings, ProviderSettings};
use crate::config::{CloudProvider, Language};
use crate::media::{FfmpegTools, MediaError, audio};

/// OpenAI 공식 업로드 제한 25 MB → 여유를 두고 검사.
const OPENAI_MAX_UPLOAD_BYTES: usize = 24 * 1024 * 1024;
/// Gemini 인라인 요청 제한 20 MB (base64 로 4/3 배 커짐 + JSON) → 원본 기준 14 MB.
const GEMINI_MAX_AUDIO_BYTES: usize = 14 * 1024 * 1024;
const MAX_ATTEMPTS: u32 = 4;
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("API Key 가 올바르지 않거나 만료되었습니다. 설정에서 키를 확인하세요.")]
    InvalidApiKey,

    #[error("API 사용 권한이 없습니다 (HTTP 403): {0}")]
    Forbidden(String),

    #[error("API 사용 한도/잔액이 부족합니다: {0}. 계정의 결제·사용 한도를 확인하세요.")]
    QuotaExceeded(String),

    #[error("요청 속도 제한에 걸렸습니다 (HTTP 429). 무료 등급이라면 한도가 낮을 수 있습니다. 잠시 후 다시 시도하세요.")]
    RateLimited,

    #[error("요청이 거부되었습니다 (HTTP {status}): {message}")]
    BadRequest { status: u16, message: String },

    #[error("서버 오류 (HTTP {0}). 잠시 후 다시 시도하세요.")]
    Server(u16),

    #[error("네트워크 오류: {0}. 인터넷 연결을 확인하세요.")]
    Network(String),

    #[error("업로드 구간이 너무 큽니다 ({0} MB). 설정에서 구간 길이를 줄이세요.")]
    ChunkTooLarge(usize),

    #[error("응답이 차단되었습니다 (안전 정책: {0})")]
    Blocked(String),

    #[error("응답 형식 오류: {0}")]
    InvalidResponse(String),

    #[error("오디오 준비 실패: {0}")]
    Media(String),

    #[error("작업이 취소되었습니다")]
    Cancelled,
}

impl From<MediaError> for CloudError {
    fn from(e: MediaError) -> Self {
        match e {
            MediaError::Cancelled => Self::Cancelled,
            other => Self::Media(other.to_string()),
        }
    }
}

impl CloudError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::Server(_) | Self::Network(_))
    }
}

/// 재시도 대기 시간을 계산하기 위해 오류와 함께 Retry-After 를 전달한다.
struct Attempt {
    error: CloudError,
    retry_after: Option<Duration>,
}

impl From<CloudError> for Attempt {
    fn from(error: CloudError) -> Self {
        Self { error, retry_after: None }
    }
}

#[derive(Clone)]
pub struct CloudClient {
    http: reqwest::blocking::Client,
    provider: CloudProvider,
    base_url: String,
}

impl CloudClient {
    pub fn new(provider: CloudProvider, base_url: &str) -> Result<Self, CloudError> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            // 긴 구간은 서버 처리 시간이 길 수 있어 응답 대기를 넉넉히 둔다.
            .timeout(Duration::from_secs(300))
            .user_agent(concat!("MediaTranscriber/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| CloudError::Network(e.to_string()))?;
        Ok(Self { http, provider, base_url: base_url.trim_end_matches('/').to_owned() })
    }

    fn authed(&self, req: reqwest::blocking::RequestBuilder, key: &ApiKey) -> reqwest::blocking::RequestBuilder {
        match self.provider {
            CloudProvider::OpenAi => req.bearer_auth(key.expose()),
            // URL 쿼리(?key=) 대신 헤더로 보내 키가 URL·오류 메시지에 남지 않게 한다.
            CloudProvider::Gemini => req.header("x-goog-api-key", key.expose()),
        }
    }

    /// API Key 확인: 모델 목록 조회 (과금 없음).
    /// Gemini 는 목록 조회 성공이 추론 권한까지 보장하지는 않는다 (문서 명시).
    pub fn validate_key(&self, key: &ApiKey) -> Result<(), CloudError> {
        let response = self
            .authed(self.http.get(format!("{}/models", self.base_url)), key)
            .send()
            .map_err(network_error)?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let retry_after = retry_after(&response);
        let body = response.text().unwrap_or_default();
        Err(self.classify(status.as_u16(), &body, retry_after).error)
    }

    /// 파일 전체를 구간별로 업로드해 인식한다. `on_progress` 는 0.0~1.0.
    #[allow(clippy::too_many_arguments)]
    pub fn transcribe_file(
        &self,
        tools: &FfmpegTools,
        input: &Path,
        duration: Option<Duration>,
        key: &ApiKey,
        settings: &CloudSettings,
        language: Language,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(f32),
    ) -> Result<Transcript, CloudError> {
        let provider_settings = settings.provider_settings(self.provider);
        let chunk_len = Duration::from_secs(u64::from(settings.chunk_seconds.max(30)));
        let ranges = chunk_ranges(duration, chunk_len);
        let total = ranges.len() as f32;
        let limit = match self.provider {
            CloudProvider::OpenAi => OPENAI_MAX_UPLOAD_BYTES,
            CloudProvider::Gemini => GEMINI_MAX_AUDIO_BYTES,
        };
        let mut segments = Vec::with_capacity(ranges.len());

        for (index, (start, length)) in ranges.iter().enumerate() {
            check_cancel(cancel)?;
            let audio = audio::encode_segment_mp3(tools, input, *start, *length, cancel)?;
            if audio.len() > limit {
                return Err(CloudError::ChunkTooLarge(audio.len() / (1024 * 1024)));
            }
            on_progress((index as f32 + 0.2) / total);

            let text = self.upload_with_retry(&audio, key, provider_settings, language, cancel)?;
            let end = length.map_or(duration.unwrap_or(*start), |l| *start + l);
            segments.push(Segment { start: *start, end, text });
            tracing::info!(
                provider = self.provider.label(),
                chunk = index + 1,
                total = ranges.len(),
                "Cloud 구간 인식 완료"
            );
            on_progress((index as f32 + 1.0) / total);
        }
        Ok(Transcript { segments, language: language.code().map(str::to_owned) })
    }

    fn upload_with_retry(
        &self,
        audio: &[u8],
        key: &ApiKey,
        settings: &ProviderSettings,
        language: Language,
        cancel: &AtomicBool,
    ) -> Result<String, CloudError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            check_cancel(cancel)?;
            let result = match self.provider {
                CloudProvider::OpenAi => self.openai_once(audio, key, settings, language),
                CloudProvider::Gemini => self.gemini_once(audio, key, settings, language),
            };
            match result {
                Ok(text) => return Ok(text),
                Err(Attempt { error, retry_after }) if error.is_retryable() && attempt < MAX_ATTEMPTS => {
                    let backoff = Duration::from_secs(2u64.pow(attempt)).min(MAX_BACKOFF);
                    let wait = retry_after.unwrap_or(backoff).min(MAX_BACKOFF * 2);
                    tracing::warn!(attempt, wait_secs = wait.as_secs(), error = %error, "Cloud 요청 재시도");
                    sleep_cancellable(wait, cancel)?;
                }
                Err(Attempt { error, .. }) => return Err(error),
            }
        }
    }

    fn openai_once(
        &self,
        audio: &[u8],
        key: &ApiKey,
        settings: &ProviderSettings,
        language: Language,
    ) -> Result<String, Attempt> {
        let part = reqwest::blocking::multipart::Part::bytes(audio.to_vec())
            .file_name("audio.mp3")
            .mime_str("audio/mpeg")
            .map_err(|e| CloudError::InvalidResponse(e.to_string()))?;
        let mut form = reqwest::blocking::multipart::Form::new()
            .part("file", part)
            .text("model", settings.model.clone())
            .text("response_format", "json");
        // gpt-transcribe 는 문서상 `language` 대신 `languages` 배열을 쓴다. 배열 전송 형식이
        // 문서에 명시되어 있지 않아, 해당 모델에서는 언어 힌트를 보내지 않고 자동 감지에 맡긴다.
        if let Some(code) = language.code()
            && !settings.model.starts_with("gpt-transcribe")
        {
            form = form.text("language", code);
        }
        let request = self.http.post(format!("{}/audio/transcriptions", self.base_url)).multipart(form);
        let body = self.send(request, key)?;

        #[derive(Deserialize)]
        struct Response {
            text: String,
        }
        serde_json::from_str::<Response>(&body)
            .map(|r| r.text)
            .map_err(|e| CloudError::InvalidResponse(e.to_string()).into())
    }

    fn gemini_once(
        &self,
        audio: &[u8],
        key: &ApiKey,
        settings: &ProviderSettings,
        language: Language,
    ) -> Result<String, Attempt> {
        let body = json!({
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": gemini_prompt(language) },
                    { "inlineData": {
                        "mimeType": "audio/mpeg",
                        "data": base64::engine::general_purpose::STANDARD.encode(audio),
                    }},
                ],
            }],
        });
        let url = format!("{}/models/{}:generateContent", self.base_url, settings.model.trim());
        let request = self.http.post(url).json(&body);
        let response = self.send(request, key)?;
        parse_gemini_response(&response).map_err(Attempt::from)
    }

    /// 인증 헤더를 붙여 보내고, 성공이면 본문 문자열을 돌려준다.
    fn send(&self, request: reqwest::blocking::RequestBuilder, key: &ApiKey) -> Result<String, Attempt> {
        let response = self.authed(request, key).send().map_err(|e| Attempt::from(network_error(e)))?;
        let status = response.status();
        let retry_after = retry_after(&response);
        let body = response.text().map_err(|e| Attempt::from(network_error(e)))?;
        if status.is_success() { Ok(body) } else { Err(self.classify(status.as_u16(), &body, retry_after)) }
    }

    fn classify(&self, status: u16, body: &str, retry_after: Option<Duration>) -> Attempt {
        let error = match self.provider {
            CloudProvider::OpenAi => classify_openai(status, body),
            CloudProvider::Gemini => classify_gemini(status, body),
        };
        Attempt { error, retry_after }
    }
}

/// Gemini 에 보내는 전사 지시문.
fn gemini_prompt(language: Language) -> String {
    let lang = match language {
        Language::Korean => " The audio is in Korean; write the transcript in Korean.",
        Language::English => " The audio is in English; write the transcript in English.",
        Language::Auto => " Keep the original spoken language; do not translate.",
    };
    format!(
        "Transcribe the speech in this audio verbatim.{lang} \
         Output only the transcript text, with one sentence or utterance per line. \
         Do not add timestamps, speaker labels, summaries, translations or any commentary. \
         If there is no speech, output nothing."
    )
}

fn parse_gemini_response(body: &str) -> Result<String, CloudError> {
    let v: Value = serde_json::from_str(body).map_err(|e| CloudError::InvalidResponse(e.to_string()))?;
    if let Some(reason) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
        return Err(CloudError::Blocked(reason.to_owned()));
    }
    let candidate = v
        .pointer("/candidates/0")
        .ok_or_else(|| CloudError::InvalidResponse("candidates 가 비어 있습니다".to_owned()))?;
    let text: String = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect())
        .unwrap_or_default();
    let finish = candidate.get("finishReason").and_then(Value::as_str).unwrap_or("STOP");
    match finish {
        "STOP" | "FINISH_REASON_UNSPECIFIED" => Ok(text),
        "MAX_TOKENS" => {
            tracing::warn!("Gemini 응답이 최대 토큰에서 잘렸습니다. 구간 길이를 줄이는 것을 권장합니다.");
            Ok(text)
        }
        other if text.trim().is_empty() => Err(CloudError::Blocked(other.to_owned())),
        other => {
            tracing::warn!(finish_reason = other, "Gemini 응답이 정상 종료되지 않았습니다");
            Ok(text)
        }
    }
}

/// 길이를 모르면 파일 전체를 한 번에 올린다 (크기 초과 시 ChunkTooLarge).
fn chunk_ranges(duration: Option<Duration>, chunk: Duration) -> Vec<(Duration, Option<Duration>)> {
    let Some(total) = duration else { return vec![(Duration::ZERO, None)] };
    let mut ranges = Vec::new();
    let mut start = Duration::ZERO;
    while start < total {
        let len = chunk.min(total - start);
        // 끝에 1초 미만 자투리만 남으면 앞 구간에 포함시킨다.
        if total - (start + len) < Duration::from_secs(1) {
            ranges.push((start, Some(total - start)));
            break;
        }
        ranges.push((start, Some(len)));
        start += len;
    }
    if ranges.is_empty() {
        ranges.push((Duration::ZERO, None));
    }
    ranges
}

/// 오류 본문에서 (메시지, 소문자 전체 본문) 을 꺼낸다. 형식이 달라도 동작하도록 느슨하게 읽는다.
fn error_message(body: &str, status: u16) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| format!("HTTP {status}"))
}

fn classify_openai(status: u16, body: &str) -> CloudError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = error_message(body, status);
    let kind = v.pointer("/error/type").and_then(Value::as_str).unwrap_or_default();
    let code = v.pointer("/error/code").and_then(Value::as_str).unwrap_or_default();
    const QUOTA_CODES: [&str; 5] = [
        "insufficient_quota",
        "credit_balance_exhausted",
        "organization_spend_limit_exceeded",
        "project_spend_limit_exceeded",
        "organization_usage_limit_exceeded",
    ];
    match status {
        401 => CloudError::InvalidApiKey,
        403 => CloudError::Forbidden(message),
        429 if kind == "insufficient_quota" || QUOTA_CODES.contains(&code) => CloudError::QuotaExceeded(message),
        429 => CloudError::RateLimited,
        500..=599 => CloudError::Server(status),
        _ => CloudError::BadRequest { status, message },
    }
}

fn classify_gemini(status: u16, body: &str) -> CloudError {
    let message = error_message(body, status);
    let lower = body.to_ascii_lowercase();
    match status {
        401 => CloudError::InvalidApiKey,
        // Google API 는 잘못된 키에 400 INVALID_ARGUMENT + reason API_KEY_INVALID 를 주기도 한다.
        400 if lower.contains("api_key_invalid") || lower.contains("api key not valid") => CloudError::InvalidApiKey,
        403 => CloudError::Forbidden(message),
        // 일일 할당량 소진은 재시도해도 소용없다 (태평양 시간 자정에 초기화).
        429 if lower.contains("quota_exceeded") || lower.contains("perday") || lower.contains("per day") => {
            CloudError::QuotaExceeded(message)
        }
        429 => CloudError::RateLimited,
        408 => CloudError::Network("요청 시간 초과".to_owned()),
        500..=599 => CloudError::Server(status),
        _ => CloudError::BadRequest { status, message },
    }
}

fn retry_after(response: &reqwest::blocking::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
}

fn network_error(e: reqwest::Error) -> CloudError {
    // reqwest 오류 문자열은 URL 을 포함할 수 있어 그대로 쓰지 않는다 (키는 헤더로만 보낸다).
    let kind = if e.is_timeout() {
        "응답 시간 초과"
    } else if e.is_connect() {
        "서버에 연결할 수 없음"
    } else {
        "전송 실패"
    };
    CloudError::Network(kind.to_owned())
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), CloudError> {
    if cancel.load(Ordering::Relaxed) { Err(CloudError::Cancelled) } else { Ok(()) }
}

fn sleep_cancellable(total: Duration, cancel: &AtomicBool) -> Result<(), CloudError> {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < total {
        check_cancel(cancel)?;
        std::thread::sleep(step);
        waited += step;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_ranges_split_and_merge_tail() {
        let r = chunk_ranges(Some(Duration::from_secs(1250)), Duration::from_secs(600));
        assert_eq!(
            r,
            vec![
                (Duration::ZERO, Some(Duration::from_secs(600))),
                (Duration::from_secs(600), Some(Duration::from_secs(600))),
                (Duration::from_secs(1200), Some(Duration::from_secs(50))),
            ]
        );
        let r = chunk_ranges(Some(Duration::from_millis(600_500)), Duration::from_secs(600));
        assert_eq!(r, vec![(Duration::ZERO, Some(Duration::from_millis(600_500)))]);
        assert_eq!(chunk_ranges(None, Duration::from_secs(600)), vec![(Duration::ZERO, None)]);
        assert_eq!(chunk_ranges(Some(Duration::from_secs(5)), Duration::from_secs(600)).len(), 1);
    }

    #[test]
    fn classifies_openai_errors() {
        assert!(matches!(classify_openai(401, ""), CloudError::InvalidApiKey));
        let quota = r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#;
        assert!(matches!(classify_openai(429, quota), CloudError::QuotaExceeded(_)));
        let spend = r#"{"error":{"message":"limit","type":"invalid_request_error","code":"project_spend_limit_exceeded"}}"#;
        assert!(matches!(classify_openai(429, spend), CloudError::QuotaExceeded(_)));
        let rate = r#"{"error":{"message":"Rate limit reached","type":"requests","code":"rate_limit_exceeded"}}"#;
        assert!(classify_openai(429, rate).is_retryable());
        assert!(matches!(classify_openai(503, ""), CloudError::Server(503)));
        match classify_openai(400, r#"{"error":{"message":"Invalid file format.","type":"invalid_request_error"}}"#) {
            CloudError::BadRequest { status, message } => {
                assert_eq!(status, 400);
                assert_eq!(message, "Invalid file format.");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(!CloudError::InvalidApiKey.is_retryable());
    }

    #[test]
    fn classifies_gemini_errors() {
        let bad_key = r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#;
        assert!(matches!(classify_gemini(400, bad_key), CloudError::InvalidApiKey));
        assert!(matches!(classify_gemini(401, r#"{"error":{"code":"authentication","message":"x"}}"#), CloudError::InvalidApiKey));
        let rate = r#"{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED"}}"#;
        assert!(matches!(classify_gemini(429, rate), CloudError::RateLimited));
        let daily = r#"{"error":{"code":"quota_exceeded","message":"Daily quota exceeded"}}"#;
        assert!(matches!(classify_gemini(429, daily), CloudError::QuotaExceeded(_)));
        let region = r#"{"error":{"code":403,"message":"User location is not supported","status":"PERMISSION_DENIED"}}"#;
        assert!(matches!(classify_gemini(403, region), CloudError::Forbidden(_)));
        assert!(matches!(classify_gemini(404, r#"{"error":{"message":"model not found"}}"#), CloudError::BadRequest { status: 404, .. }));
        assert!(classify_gemini(503, "").is_retryable());
    }

    #[test]
    fn parses_gemini_responses() {
        let ok = r#"{"candidates":[{"content":{"parts":[{"text":"안녕하세요.\n"},{"text":"반갑습니다."}],"role":"model"},"finishReason":"STOP"}]}"#;
        assert_eq!(parse_gemini_response(ok).expect("ok"), "안녕하세요.\n반갑습니다.");
        let blocked = r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#;
        assert!(matches!(parse_gemini_response(blocked), Err(CloudError::Blocked(r)) if r == "SAFETY"));
        let safety = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"SAFETY"}]}"#;
        assert!(matches!(parse_gemini_response(safety), Err(CloudError::Blocked(_))));
        let empty_ok = r#"{"candidates":[{"content":{"parts":[{"text":""}]},"finishReason":"STOP"}]}"#;
        assert_eq!(parse_gemini_response(empty_ok).expect("ok"), "");
        assert!(parse_gemini_response("not json").is_err());
    }

    #[test]
    fn prompt_mentions_language() {
        assert!(gemini_prompt(Language::Korean).contains("Korean"));
        assert!(gemini_prompt(Language::Auto).contains("do not translate"));
    }
}

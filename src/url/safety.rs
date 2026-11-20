//! URL 입력 검증과 로그용 가리기.
//!
//! - 외부 프로세스에는 항상 인자 배열로 넘기고, URL 앞에 `--` 를 붙여 옵션으로 해석되지 않게 한다.
//! - 서명된 스트리밍 주소(m3u8 등)는 쿼리 문자열에 인증 토큰이 들어 있는 경우가 많으므로
//!   로그에는 쿼리·조각을 지운 주소만 남긴다.

/// 허용하는 URL 최대 길이
const MAX_URL_LEN: usize = 4096;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UrlInputError {
    #[error("URL 을 입력하세요")]
    Empty,
    #[error("http:// 또는 https:// 로 시작하는 주소만 지원합니다")]
    Scheme,
    #[error("URL 에 공백이나 제어 문자가 들어 있습니다")]
    InvalidChars,
    #[error("URL 이 너무 깁니다")]
    TooLong,
    #[error("주소에 호스트가 없습니다")]
    NoHost,
}

/// 사용자가 입력한 URL 을 검증하고 앞뒤 공백을 지운 값을 돌려준다.
pub fn validate_url(input: &str) -> Result<String, UrlInputError> {
    let url = input.trim();
    if url.is_empty() {
        return Err(UrlInputError::Empty);
    }
    if url.len() > MAX_URL_LEN {
        return Err(UrlInputError::TooLong);
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(UrlInputError::InvalidChars);
    }
    let lower = url.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if let Some(r) = lower.strip_prefix("http://") {
        r
    } else {
        return Err(UrlInputError::Scheme);
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // 사용자 정보(user:pass@)는 로그·프로세스 인자에 노출되므로 허용하지 않는다.
    if host.is_empty() || host.contains('@') {
        return Err(UrlInputError::NoHost);
    }
    Ok(url.to_owned())
}

/// 로그용: `https://host/path?token=…#…` → `https://host/path`
pub fn redact_url(url: &str) -> String {
    let cut = url.find(['?', '#']).unwrap_or(url.len());
    url[..cut].to_owned()
}

/// 외부 도구 오류 메시지 안의 URL 에서 쿼리 문자열을 지운다 (토큰 노출 방지).
pub fn redact_text(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            if word.contains("://") {
                let start = word.find("http").unwrap_or(0);
                format!("{}{}", &word[..start], redact_url(&word[start..]))
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 결과 파일 이름으로 쓸 수 있게 영상 제목을 정리한다 (Windows 금지 문자 제거, 길이 제한).
pub fn safe_file_stem(title: &str) -> String {
    const FORBIDDEN: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let cleaned: String = title
        .chars()
        .map(|c| if FORBIDDEN.contains(&c) || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).trim();
    let limited: String = trimmed.chars().take(120).collect();
    // 예약 이름(CON, NUL 등) 회피
    let upper = limited.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "LPT1", "LPT2", "LPT3"];
    if limited.is_empty() {
        "video".to_owned()
    } else if reserved.contains(&upper.as_str()) {
        format!("{limited}_")
    } else {
        limited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_urls() {
        assert_eq!(validate_url("  https://example.com/a.mp4 ").as_deref(), Ok("https://example.com/a.mp4"));
        assert!(validate_url("http://127.0.0.1:8080/x.m3u8").is_ok());
        assert_eq!(validate_url(""), Err(UrlInputError::Empty));
        assert_eq!(validate_url("ftp://x"), Err(UrlInputError::Scheme));
        assert_eq!(validate_url("--exec calc"), Err(UrlInputError::InvalidChars));
        assert_eq!(validate_url("-o evil"), Err(UrlInputError::InvalidChars));
        assert_eq!(validate_url("--config-locations=x"), Err(UrlInputError::Scheme));
        assert_eq!(validate_url("https://a.com/x\ny"), Err(UrlInputError::InvalidChars));
        assert_eq!(validate_url("https://user:pw@a.com/"), Err(UrlInputError::NoHost));
        assert_eq!(validate_url("https:///path"), Err(UrlInputError::NoHost));
        assert_eq!(validate_url(&format!("https://a.com/{}", "x".repeat(5000))), Err(UrlInputError::TooLong));
    }

    #[test]
    fn redacts_tokens() {
        assert_eq!(redact_url("https://cdn.x/v/a.m3u8?token=SECRET#t=1"), "https://cdn.x/v/a.m3u8");
        let msg = "ERROR: HTTP 403 for https://cdn.x/a.m3u8?sig=SECRET (forbidden)";
        let out = redact_text(msg);
        assert!(!out.contains("SECRET"));
        assert!(out.contains("https://cdn.x/a.m3u8"));
    }

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(safe_file_stem("1주차: 강의 소개 / Part 1?"), "1주차_ 강의 소개 _ Part 1_");
        assert_eq!(safe_file_stem("   ...  "), "video");
        assert_eq!(safe_file_stem("con"), "con_");
        assert_eq!(safe_file_stem(&"가".repeat(300)).chars().count(), 120);
    }
}

//! 엔진 공통 결과 타입.
//!
//! TXT 외에 SRT/VTT/JSON 출력을 추가할 수 있도록 구간(segment) 단위로 보관한다.

use std::time::Duration;

/// 인식된 음성 구간.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

/// 한 파일의 인식 결과.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    pub segments: Vec<Segment>,
    /// 인식 언어 코드 (예: "ko"). 자동 감지 결과를 알 수 없으면 None.
    pub language: Option<String>,
}

impl Transcript {
    /// 공백뿐인 구간을 제외하고 실제 텍스트가 있는지.
    pub fn is_empty(&self) -> bool {
        self.segments.iter().all(|s| s.text.trim().is_empty())
    }

    /// TXT 출력용 본문: 구간마다 한 줄.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            let line = seg.text.trim();
            if line.is_empty() {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    #[error("Whisper 모델 파일이 없습니다: {0}. 설정에서 모델을 다운로드하세요.")]
    ModelMissing(String),

    #[error("Whisper 모델을 불러오지 못했습니다 ({model}): {reason}. 모델 파일이 손상되었거나 메모리가 부족할 수 있습니다.")]
    ModelLoad { model: String, reason: String },

    #[error("음성 인식 실패: {0}")]
    Inference(String),

    #[error("작업이 취소되었습니다")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str) -> Segment {
        Segment { start: Duration::ZERO, end: Duration::ZERO, text: text.to_owned() }
    }

    #[test]
    fn plain_text_skips_blank_segments() {
        let t = Transcript { segments: vec![seg(" 안녕하세요."), seg("  "), seg("Hello ")], language: None };
        assert_eq!(t.plain_text(), "안녕하세요.\nHello\n");
        assert!(!t.is_empty());
        assert!(Transcript { segments: vec![seg(" ")], language: None }.is_empty());
    }
}

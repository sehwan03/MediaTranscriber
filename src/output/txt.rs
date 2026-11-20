use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::OutputError;
use crate::speech::Transcript;

/// 같은 이름 파일이 있을 때 시도할 최대 번호.
const MAX_NAME_SUFFIX: u32 = 9_999;

/// 결과를 UTF-8 TXT 로 저장한다.
///
/// - 파일명: 원본 파일명 기반 (`강의.mp4` → `강의.txt`)
/// - 기존 파일은 덮어쓰지 않는다: `강의 (1).txt`, `강의 (2).txt` … 로 저장
/// - 빈 결과는 저장하지 않고 오류를 반환한다.
#[cfg(test)]
pub fn save_txt(output_dir: &Path, input: &Path, transcript: &Transcript) -> Result<PathBuf, OutputError> {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "transcript".to_owned());
    save_txt_named(output_dir, &stem, transcript)
}

/// 결과 이름(확장자 제외)을 직접 지정해 저장한다 (URL 영상은 제목 기반).
pub fn save_txt_named(output_dir: &Path, stem: &str, transcript: &Transcript) -> Result<PathBuf, OutputError> {
    if transcript.is_empty() {
        return Err(OutputError::EmptyResult);
    }
    let text = transcript.plain_text();
    std::fs::create_dir_all(output_dir).map_err(|e| OutputError::io(output_dir, e))?;
    let stem = if stem.trim().is_empty() { "transcript".to_owned() } else { stem.to_owned() };

    for n in 0..=MAX_NAME_SUFFIX {
        let name = if n == 0 { format!("{stem}.txt") } else { format!("{stem} ({n}).txt") };
        let path = output_dir.join(name);
        // create_new: 이미 있으면 실패 → 다음 번호. 확인과 생성 사이 경쟁 조건도 없다.
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(OutputError::io(&path, e)),
        };
        let written = file.write_all(text.as_bytes()).and_then(|()| file.sync_all());
        if let Err(e) = written {
            drop(file);
            // 반쯤 쓰인 파일을 남기지 않는다.
            let _ = std::fs::remove_file(&path);
            return Err(OutputError::io(&path, e));
        }
        return Ok(path);
    }
    Err(OutputError::TooManyDuplicates(stem))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::speech::Transcript;
    use crate::speech::engine::Segment;

    fn transcript(text: &str) -> Transcript {
        Transcript {
            segments: vec![Segment { start: Duration::ZERO, end: Duration::from_secs(1), text: text.to_owned() }],
            language: Some("ko".to_owned()),
        }
    }

    #[test]
    fn saves_utf8_without_overwriting() {
        let dir = std::env::temp_dir()
            .join(format!("media_transcriber_txt_{}", std::process::id()))
            .join("결과 폴더");
        let _ = std::fs::remove_dir_all(&dir);
        let input = Path::new(r"C:\영상\강의 1.mp4");

        let first = save_txt(&dir, input, &transcript(" 안녕하세요. Hello")).expect("save");
        assert_eq!(first.file_name().and_then(|n| n.to_str()), Some("강의 1.txt"));
        assert_eq!(std::fs::read_to_string(&first).expect("read"), "안녕하세요. Hello\n");

        let second = save_txt(&dir, input, &transcript("두번째")).expect("save");
        assert_eq!(second.file_name().and_then(|n| n.to_str()), Some("강의 1 (1).txt"));
        // 첫 파일은 그대로
        assert_eq!(std::fs::read_to_string(&first).expect("read"), "안녕하세요. Hello\n");

        let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
    }

    #[test]
    fn empty_result_is_not_saved() {
        let dir = std::env::temp_dir().join(format!("media_transcriber_txt_empty_{}", std::process::id()));
        let result = save_txt(&dir, Path::new("a.mp3"), &transcript("   "));
        assert!(matches!(result, Err(OutputError::EmptyResult)));
        assert!(!dir.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

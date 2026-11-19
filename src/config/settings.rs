use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine_selector::BenchmarkResult;
use crate::utils::error::AppError;

/// 음성 인식 실행 모드.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EngineMode {
    #[default]
    Auto,
    Local,
    Cloud,
}


/// 로컬 Whisper 모델 (whisper.cpp ggml 형식).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum WhisperModel {
    Tiny,
    #[default]
    Base,
    Small,
    Medium,
}

impl WhisperModel {
    pub const ALL: [Self; 4] = [Self::Tiny, Self::Base, Self::Small, Self::Medium];

    pub fn label(self) -> &'static str {
        match self {
            Self::Tiny => "Tiny",
            Self::Base => "Base",
            Self::Small => "Small",
            Self::Medium => "Medium",
        }
    }

    /// 추론 시 예상 메모리 사용량 (MiB, whisper.cpp README 기준 근사값).
    pub fn approx_memory_mb(self) -> u32 {
        match self {
            Self::Tiny => 273,
            Self::Base => 388,
            Self::Small => 852,
            Self::Medium => 2100,
        }
    }
}

/// 인식 언어.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Language {
    #[default]
    Auto,
    Korean,
    English,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Korean, Self::English];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "자동 감지",
            Self::Korean => "한국어",
            Self::English => "English",
        }
    }

    /// Whisper / OpenAI API 에 전달할 ISO-639-1 코드. 자동 감지는 None.
    pub fn code(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Korean => Some("ko"),
            Self::English => Some("en"),
        }
    }
}

/// Cloud 음성 인식 공급자.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CloudProvider {
    #[default]
    OpenAi,
    Gemini,
}

impl CloudProvider {
    pub const ALL: [Self; 2] = [Self::OpenAi, Self::Gemini];

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Google Gemini",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::OpenAi => 0,
            Self::Gemini => 1,
        }
    }

    /// API Key 입력란 힌트
    pub fn key_hint(self) -> &'static str {
        match self {
            Self::OpenAi => "sk-...",
            Self::Gemini => "AIza...",
        }
    }
}

/// 공급자별 기본값.
/// - OpenAI: 2026-11 문서 기준 신규 권장 모델 `gpt-transcribe`
///   (whisper-1 / gpt-4o-*-transcribe 는 2027-02-26 종료 예정)
/// - Gemini: 2026-11 문서 기준 무료 등급·오디오 입력이 확인된 가장 저렴/빠른 안정 모델
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-transcribe";
pub const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.5-flash-lite";

/// 공급자 하나의 접속 설정.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderSettings {
    pub base_url: String,
    pub model: String,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self::defaults(CloudProvider::OpenAi)
    }
}

impl ProviderSettings {
    pub fn defaults(provider: CloudProvider) -> Self {
        let (base_url, model) = match provider {
            CloudProvider::OpenAi => (DEFAULT_OPENAI_BASE_URL, DEFAULT_OPENAI_MODEL),
            CloudProvider::Gemini => (DEFAULT_GEMINI_BASE_URL, DEFAULT_GEMINI_MODEL),
        };
        Self { base_url: base_url.to_owned(), model: model.to_owned() }
    }
}

fn default_gemini() -> ProviderSettings {
    ProviderSettings::defaults(CloudProvider::Gemini)
}

/// Cloud API 관련 설정. 모델명/주소/제한값은 코드에 흩어 두지 않고 여기서 관리한다.
/// API Key 는 이 파일에 절대 저장하지 않는다 (config::secret 참고).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    pub provider: CloudProvider,
    pub openai: ProviderSettings,
    #[serde(default = "default_gemini")]
    pub gemini: ProviderSettings,
    pub max_concurrent_requests: u32,
    /// 업로드 구간 길이(초). 16 kHz mono 48 kbps MP3 기준 600초 ≈ 3.6 MB
    /// (OpenAI 파일당 25 MB, Gemini 요청당 20 MB 제한).
    pub chunk_seconds: u32,
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            provider: CloudProvider::OpenAi,
            openai: ProviderSettings::defaults(CloudProvider::OpenAi),
            gemini: ProviderSettings::defaults(CloudProvider::Gemini),
            max_concurrent_requests: 2,
            chunk_seconds: 600,
        }
    }
}

impl CloudSettings {
    pub fn provider_settings(&self, provider: CloudProvider) -> &ProviderSettings {
        match provider {
            CloudProvider::OpenAi => &self.openai,
            CloudProvider::Gemini => &self.gemini,
        }
    }

    pub fn provider_settings_mut(&mut self, provider: CloudProvider) -> &mut ProviderSettings {
        match provider {
            CloudProvider::OpenAi => &mut self.openai,
            CloudProvider::Gemini => &mut self.gemini,
        }
    }

    pub fn active(&self) -> &ProviderSettings {
        self.provider_settings(self.provider)
    }
}

/// 사용자 설정. 누락된 필드는 기본값으로 채워 이전 버전 설정 파일과 호환된다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub engine_mode: EngineMode,
    pub whisper_model: WhisperModel,
    pub language: Language,
    /// None 이면 기본 출력 경로 사용.
    pub output_dir: Option<PathBuf>,
    /// FFmpeg 폴더 (ffmpeg.exe / ffprobe.exe). None 이면 자동 탐색.
    pub ffmpeg_dir: Option<PathBuf>,
    pub cloud: CloudSettings,
    /// 로컬 성능 측정 결과 (모델별 최신 1개)
    pub benchmarks: Vec<BenchmarkResult>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine_mode: EngineMode::Auto,
            whisper_model: WhisperModel::Base,
            language: Language::Auto,
            output_dir: None,
            ffmpeg_dir: None,
            cloud: CloudSettings::default(),
            benchmarks: Vec::new(),
        }
    }
}

impl Settings {
    /// 설정 파일을 읽는다. 파일이 없거나 손상된 경우 기본값을 사용하고 경고를 남긴다.
    pub fn load_or_default(path: &Path) -> Self {
        match Self::load(path) {
            Ok(Some(settings)) => {
                tracing::info!(path = %path.display(), "설정 파일 로드");
                settings
            }
            Ok(None) => {
                tracing::info!("설정 파일이 없어 기본 설정 사용");
                Self::default()
            }
            Err(err) => {
                tracing::warn!(error = %err, "설정 파일 로드 실패, 기본 설정 사용");
                Self::default()
            }
        }
    }

    fn load(path: &Path) -> Result<Option<Self>, AppError> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AppError::io(path, e)),
        };
        // 메모장/PowerShell 5.1 등은 UTF-8 BOM 을 붙여 저장하므로 허용한다.
        let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        serde_json::from_slice(json)
            .map(Some)
            .map_err(|source| AppError::ConfigParse { path: path.to_path_buf(), source })
    }

    /// 임시 파일에 쓴 뒤 교체하여, 저장 중 종료되어도 설정 파일이 깨지지 않도록 한다.
    pub fn save(&self, path: &Path) -> Result<(), AppError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        let json = serde_json::to_vec_pretty(self)
            .map_err(|source| AppError::ConfigParse { path: path.to_path_buf(), source })?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| AppError::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| AppError::io(path, e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("media_transcriber_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_and_load_roundtrip_with_korean_path() {
        let root = temp_dir("roundtrip");
        let path = root.join("한글 경로").join("settings.json");
        let settings = Settings {
            engine_mode: EngineMode::Local,
            whisper_model: WhisperModel::Small,
            language: Language::Korean,
            output_dir: Some(PathBuf::from(r"C:\결과 폴더")),
            ffmpeg_dir: Some(PathBuf::from(r"D:\도구\ff mpeg")),
            cloud: CloudSettings::default(),
            benchmarks: Vec::new(),
        };
        settings.save(&path).expect("save");
        let loaded = Settings::load(&path).expect("load").expect("exists");
        assert_eq!(settings, loaded);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_file_gives_none_and_partial_json_uses_defaults() {
        let dir = temp_dir("partial");
        assert!(Settings::load(&dir.join("nope.json")).expect("ok").is_none());

        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"engine_mode":"Cloud"}"#).expect("write");
        let loaded = Settings::load(&path).expect("load").expect("exists");
        assert_eq!(loaded.engine_mode, EngineMode::Cloud);
        assert_eq!(loaded.whisper_model, WhisperModel::Base);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utf8_bom_is_accepted() {
        let dir = temp_dir("bom");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        std::fs::write(&path, b"\xEF\xBB\xBF{\"language\":\"Korean\"}").expect("write");
        let loaded = Settings::load(&path).expect("load").expect("exists");
        assert_eq!(loaded.language, Language::Korean);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_file_is_error() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        std::fs::write(&path, "{not json").expect("write");
        assert!(Settings::load(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

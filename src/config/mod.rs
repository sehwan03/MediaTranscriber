pub mod secret;
pub mod settings;

use std::path::PathBuf;

use directories::ProjectDirs;

pub use settings::{CloudProvider, EngineMode, Language, Settings, WhisperModel};

/// 애플리케이션이 사용하는 사용자별 경로 모음.
///
/// - 설정: `%APPDATA%\MediaTranscriber\config\settings.json`
/// - 로그: `%LOCALAPPDATA%\MediaTranscriber\data\logs`
/// - 모델: `%LOCALAPPDATA%\MediaTranscriber\data\models`
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub settings_file: PathBuf,
    pub log_dir: PathBuf,
    pub models_dir: PathBuf,
    /// 앱이 자동 설치한 FFmpeg 폴더
    pub ffmpeg_dir: PathBuf,
    /// 출력 경로를 지정하지 않았을 때 사용할 기본 위치 (문서\MediaTranscriber)
    pub default_output_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Option<Self> {
        let dirs = ProjectDirs::from("", "", crate::APP_NAME)?;
        let default_output_dir = directories::UserDirs::new()
            .and_then(|u| u.document_dir().map(|d| d.join(crate::APP_NAME)))
            .unwrap_or_else(|| dirs.data_local_dir().join("output"));

        Some(Self {
            settings_file: dirs.config_dir().join("settings.json"),
            log_dir: dirs.data_local_dir().join("logs"),
            models_dir: dirs.data_local_dir().join("models"),
            ffmpeg_dir: dirs.data_local_dir().join("ffmpeg"),
            default_output_dir,
        })
    }
}

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// 콘솔(stderr)과 일별 로그 파일에 동시에 기록하는 로거를 초기화한다.
///
/// 로그 디렉터리를 만들 수 없으면 콘솔 로그만 사용한다.
/// 주의: API Key, 음성 데이터, 인식 결과 텍스트는 절대 로그에 남기지 않는다.
pub fn init(log_dir: &Path) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_env("MEDIATRANSCRIBER_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,eframe=warn,egui_glow=warn,whisper_rs=warn,reqwest=warn,hyper_util=warn"));

    let console_layer = fmt::layer().with_writer(std::io::stderr).with_target(false);

    match std::fs::create_dir_all(log_dir) {
        Ok(()) => {
            // 일별 로그, 최근 14개 파일만 보관 (오래된 로그는 자동 삭제)
            let file_appender = match tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("media_transcriber")
                .filename_suffix("log")
                .max_log_files(14)
                .build(log_dir)
            {
                Ok(a) => a,
                Err(err) => {
                    eprintln!("로그 파일 생성 실패: {err}");
                    let init = tracing_subscriber::registry().with(filter).with(console_layer).try_init();
                    if let Err(init_err) = init {
                        eprintln!("로거 초기화 실패: {init_err}");
                    }
                    return None;
                }
            };
            let (writer, guard) = tracing_appender::non_blocking(file_appender);
            let file_layer = fmt::layer().with_writer(writer).with_ansi(false).with_target(true);
            let init = tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .with(file_layer)
                .try_init();
            if let Err(err) = init {
                eprintln!("로거 초기화 실패: {err}");
            }
            Some(guard)
        }
        Err(err) => {
            let init = tracing_subscriber::registry().with(filter).with(console_layer).try_init();
            if let Err(init_err) = init {
                eprintln!("로거 초기화 실패: {init_err}");
            }
            tracing::warn!(dir = %log_dir.display(), error = %err, "로그 디렉터리 생성 실패, 콘솔 로그만 사용");
            None
        }
    }
}

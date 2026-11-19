// Release 빌드에서는 콘솔 창을 띄우지 않는다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod engine_selector;
mod media;
mod output;
mod speech;
mod system;
mod task;
mod ui;
mod utils;

use anyhow::Context as _;

use crate::app::MediaTranscriberApp;
use crate::config::{AppPaths, Settings};

pub const APP_NAME: &str = "MediaTranscriber";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> anyhow::Result<()> {
    let paths = AppPaths::resolve().context("애플리케이션 데이터 경로를 결정할 수 없습니다")?;
    // guard 가 drop 되면 파일 로그가 flush 되므로 main 종료 시점까지 유지한다.
    let _log_guard = utils::logger::init(&paths.log_dir);

    tracing::info!(version = APP_VERSION, "MediaTranscriber 시작");
    install_panic_hook(paths.log_dir.clone());
    // whisper.cpp / ggml 의 stderr 로그를 tracing 으로 돌린다.
    whisper_rs::install_logging_hooks();

    let settings = Settings::load_or_default(&paths.settings_file);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{APP_NAME} {APP_VERSION}"))
            .with_icon(window_icon())
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([800.0, 520.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(MediaTranscriberApp::new(cc, paths, settings)))),
    );

    if let Err(err) = &result {
        tracing::error!(error = %err, "GUI 실행 실패");
    }
    tracing::info!("MediaTranscriber 종료");
    result.map_err(|e| anyhow::anyhow!("GUI 실행 실패: {e}"))
}

/// 창 아이콘 (scripts/make-icon.ps1 이 만든 64x64 RGBA).
fn window_icon() -> egui::IconData {
    const RGBA: &[u8] = include_bytes!("../assets/icon-64.rgba");
    egui::IconData { rgba: RGBA.to_vec(), width: 64, height: 64 }
}

/// 예기치 않은 오류(panic)를 로그 파일에 남기고, 사용자에게 로그 위치를 알린다.
fn install_panic_hook(log_dir: std::path::PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "알 수 없는 오류".to_owned());
        tracing::error!(%location, %message, "예기치 않은 오류 (panic)");
        // 작업 스레드의 panic 은 해당 작업만 실패하므로 알림창은 GUI(main) 스레드일 때만 띄운다.
        if std::thread::current().name() == Some("main") {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("MediaTranscriber 오류")
                .set_description(format!(
                    "예기치 않은 오류로 프로그램을 종료합니다.\n\n{message}\n\n로그 폴더: {}",
                    log_dir.display()
                ))
                .show();
        }
        default_hook(info);
    }));
}

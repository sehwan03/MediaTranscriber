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
    // whisper.cpp / ggml 의 stderr 로그를 tracing 으로 돌린다.
    whisper_rs::install_logging_hooks();

    let settings = Settings::load_or_default(&paths.settings_file);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{APP_NAME} {APP_VERSION}"))
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

//! 화면 구성 요소. 각 패널은 `AppState` 를 받아 그리기만 하며,
//! 무거운 작업(FFmpeg, 추론, HTTP)은 절대 여기서 수행하지 않는다.

pub mod file_panel;
pub mod result_panel;
pub mod settings_panel;
pub mod task_panel;
pub mod theme;

use egui::RichText;

use crate::app::{AppState, FfmpegStatus};
use crate::utils::format_bytes;

/// FFmpeg 가 없을 때 보여주는 자동 설치 안내 (작업 목록·설정 화면 공용).
pub fn ffmpeg_install_banner(ui: &mut egui::Ui, state: &mut AppState) {
    if !matches!(state.ffmpeg, FfmpegStatus::Missing(_)) && state.ffmpeg_install.is_none() {
        return;
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        match &state.ffmpeg_install {
            Some(p) => {
                ui.horizontal(|ui| {
                    ui.label("FFmpeg 내려받는 중…");
                    let frac = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
                    let text = if p.total > 0 {
                        format!("{} / {}", format_bytes(p.done), format_bytes(p.total))
                    } else {
                        format_bytes(p.done)
                    };
                    ui.add(egui::ProgressBar::new(frac).desired_width(240.0).text(text));
                    if ui.button("취소").clicked() {
                        p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
            None => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "음성·동영상 처리에 필요한 FFmpeg 가 없습니다.",
                );
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(RichText::new(format!(
                            "FFmpeg 자동 설치 (약 {} MB 다운로드)",
                            crate::media::installer::APPROX_DOWNLOAD_MB
                        )).strong())
                        .clicked()
                    {
                        state.ffmpeg_install_requested = true;
                    }
                    ui.label(RichText::new("또는 설정에서 FFmpeg 폴더를 직접 지정할 수 있습니다.").weak());
                });
                ui.label(
                    RichText::new(format!(
                        "출처: {} — 내려받은 뒤 SHA-256 을 확인하고 앱 데이터 폴더에만 설치합니다.",
                        crate::media::installer::source_description()
                    ))
                    .weak()
                    .small(),
                );
            }
        }
    });
}

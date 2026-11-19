use std::path::Path;

use egui::{FontId, RichText, Sense, pos2, vec2};

use super::theme::{self, Icon};
use crate::app::{AppState, StatusLevel};

/// 결과 카드: 최근 완료 결과, TXT 열기 · 폴더에서 보기, 결과 폴더 열기.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    let mut error = None;
    let mut clear = false;
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("결과").font(theme::bold(15.5)).color(theme::TEXT));
            if !state.results.is_empty() {
                ui.label(RichText::new(state.results.len().to_string()).font(theme::bold(15.5)).color(theme::TEXT_FAINT));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button_inline(ui, Icon::Folder, "결과 폴더 열기") {
                    let dir = state.output_dir().to_path_buf();
                    if let Err(err) = open_folder(&dir) {
                        tracing::warn!(dir = %dir.display(), error = %err, "결과 폴더 열기 실패");
                        error = Some(format!("결과 폴더를 열 수 없습니다: {err}"));
                    }
                }
                if !state.results.is_empty() && theme::icon_button_inline(ui, Icon::Trash, "결과 목록 비우기 (TXT 파일은 삭제하지 않음)") {
                    clear = true;
                }
            });
        });

        if state.results.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("완료된 결과가 여기에 표시됩니다").size(12.5).color(theme::TEXT_FAINT));
            return;
        }
        ui.add_space(2.0);
        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, entry) in state.results.iter().enumerate().rev() {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
                let painter = ui.painter().clone();
                if response.hovered() {
                    painter.rect_filled(rect, egui::CornerRadius::same(theme::RADIUS_SM), theme::CARD_HOVER);
                }
                let icon = egui::Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(20.0, 20.0));
                theme::paint_icon(&painter, icon, Icon::FileText, theme::SUCCESS);

                let open_btn = egui::Rect::from_center_size(pos2(rect.right() - 46.0, rect.center().y), vec2(26.0, 26.0));
                let reveal_btn = egui::Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(26.0, 26.0));
                let text_left = icon.right() + 10.0;
                let width = open_btn.left() - 8.0 - text_left;
                let name = entry.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let g = theme::truncated(&painter, &name, theme::bold(13.0), theme::TEXT, width);
                painter.galley(pos2(text_left, rect.top() + 5.0), g, theme::TEXT);
                let sub = format!("{} · {:.1}초", entry.engine, entry.elapsed.as_secs_f32());
                let g = theme::truncated(&painter, &sub, FontId::proportional(11.5), theme::TEXT_FAINT, width);
                painter.galley(pos2(text_left, rect.top() + 25.0), g, theme::TEXT_FAINT);

                if theme::icon_button(ui, open_btn, ui.scope_id().with(("result-open", i)), Icon::FileText, "TXT 열기", theme::TEXT_DIM)
                    && let Err(err) = open_file(&entry.output)
                {
                    error = Some(format!("파일을 열 수 없습니다: {err}"));
                }
                if theme::icon_button(ui, reveal_btn, ui.scope_id().with(("result-reveal", i)), Icon::Folder, "폴더에서 보기", theme::TEXT_DIM)
                    && let Err(err) = reveal_in_folder(&entry.output)
                {
                    error = Some(format!("폴더를 열 수 없습니다: {err}"));
                }
                if response.hovered() {
                    response.on_hover_text(format!("{}\n원본: {}", entry.output.display(), entry.source_name));
                }
            }
        });
    });
    if clear {
        state.results.clear();
    }
    if let Some(msg) = error {
        state.set_status(StatusLevel::Error, msg);
    }
}

/// 폴더가 없으면 만든 뒤 Windows 탐색기로 연다.
/// 경로는 인자 배열로 전달하므로 공백/한글이 포함되어도 안전하다.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // explorer.exe 는 성공해도 종료 코드 1 을 반환하는 경우가 있어 spawn 결과만 확인한다.
    std::process::Command::new("explorer").arg(dir).spawn().map(|_| ())
}

/// 기본 연결 프로그램(메모장 등)으로 연다. 셸(cmd /c start)을 거치지 않는다.
pub fn open_file(path: &Path) -> std::io::Result<()> {
    if !path.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "결과 파일이 삭제되었거나 이동되었습니다"));
    }
    std::process::Command::new("explorer").arg(path).spawn().map(|_| ())
}

/// 탐색기에서 파일을 선택한 상태로 연다.
fn reveal_in_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer 는 "/select,<경로>" 를 하나의 인자로 받는다. Windows 경로에는 '"' 가 올 수 없으므로 안전하다.
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(windows))]
    {
        open_folder(path.parent().unwrap_or(path))
    }
}

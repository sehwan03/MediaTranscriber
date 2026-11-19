use egui::RichText;

use crate::app::{AppState, StatusLevel};
use crate::utils::format_bytes;

const ROW_HEIGHT: f32 = 22.0;

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    ui.add_space(6.0);
    ui.heading("파일 등록");
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        if ui.button("파일 선택…").clicked() {
            pick_files(state);
        }
        if ui.button("폴더 선택…").clicked() {
            pick_folder(state);
        }
        if ui
            .add_enabled(!state.files.is_empty(), egui::Button::new("목록 비우기"))
            .clicked()
        {
            state.clear_files();
        }
    });

    let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let hint = if hovering {
        RichText::new("여기에 놓으면 추가됩니다").strong()
    } else {
        RichText::new("파일이나 폴더를 창으로 끌어다 놓을 수 있습니다").weak()
    };
    ui.label(hint);
    ui.add_space(4.0);
    ui.label(format!("등록된 파일: {}개", state.files.len()));
    ui.separator();

    let mut remove_index = None;
    // show_rows 로 화면에 보이는 행만 그려 대량 목록에서도 가볍게 유지한다.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, state.files.len(), |ui, range| {
            for index in range {
                let file = &state.files[index];
                ui.horizontal(|ui| {
                    ui.set_height(ROW_HEIGHT);
                    if ui.small_button("제거").on_hover_text("목록에서 제거").clicked() {
                        remove_index = Some(index);
                    }
                    ui.label(RichText::new(format_bytes(file.size_bytes)).weak().monospace());
                    ui.add(egui::Label::new(&file.display_name).truncate())
                        .on_hover_text(file.path.display().to_string());
                });
            }
        });

    if let Some(index) = remove_index {
        state.remove_file(index);
    }
}

fn media_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new()
        .set_title("변환할 파일 선택")
        .add_filter("미디어 파일", crate::app::SUPPORTED_EXTENSIONS)
        .add_filter("모든 파일", &["*"])
}

fn pick_files(state: &mut AppState) {
    if let Some(paths) = media_dialog().pick_files() {
        let (added, skipped) = state.add_paths(paths);
        report_added(state, added, skipped);
    }
}

fn pick_folder(state: &mut AppState) {
    if let Some(dir) = rfd::FileDialog::new().set_title("폴더 선택").pick_folder() {
        let (added, skipped) = state.add_paths([dir]);
        report_added(state, added, skipped);
    }
}

pub fn report_added(state: &mut AppState, added: usize, skipped: usize) {
    let text = match (added, skipped) {
        (0, 0) => return,
        (a, 0) => format!("{a}개 파일을 추가했습니다."),
        (0, s) => format!("추가된 파일이 없습니다. ({s}개는 지원하지 않는 형식이거나 이미 등록됨)"),
        (a, s) => format!("{a}개 파일을 추가했습니다. ({s}개 건너뜀)"),
    };
    let level = if added == 0 { StatusLevel::Warning } else { StatusLevel::Info };
    tracing::info!(added, skipped, "파일 등록");
    state.set_status(level, text);
}

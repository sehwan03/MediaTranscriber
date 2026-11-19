use std::path::Path;

use egui::RichText;

use crate::app::{AppState, StatusLevel};

/// 결과 영역: 완료된 파일 목록, TXT 열기, 결과 폴더 열기.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.heading("결과");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("결과 폴더 열기").clicked() {
                let dir = state.output_dir().to_path_buf();
                if let Err(err) = open_folder(&dir) {
                    tracing::warn!(dir = %dir.display(), error = %err, "결과 폴더 열기 실패");
                    state.set_status(StatusLevel::Error, format!("결과 폴더를 열 수 없습니다: {err}"));
                }
            }
            // 경로가 길면 제목을 덮지 않도록 남은 폭에서 잘라 표시한다.
            let path = state.output_dir().display().to_string();
            ui.add(egui::Label::new(RichText::new(&path).weak().monospace()).truncate())
                .on_hover_text(path);
        });
    });
    ui.separator();

    let mut error = None;
    let mut clear = false;
    if state.results.is_empty() {
        ui.label(RichText::new("완료된 파일이 없습니다.").weak());
    } else {
        ui.horizontal(|ui| {
            ui.label(format!("완료 {}개 (최근 순)", state.results.len()));
            if ui.small_button("결과 목록 비우기").on_hover_text("목록만 비웁니다. 저장된 TXT 파일은 삭제하지 않습니다.").clicked() {
                clear = true;
            }
        });
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for entry in state.results.iter().rev() {
            let output = &entry.output;
            ui.horizontal(|ui| {
                if ui.small_button("열기").clicked()
                    && let Err(err) = open_file(output)
                {
                    error = Some(format!("파일을 열 수 없습니다: {err}"));
                }
                if ui.small_button("폴더에서 보기").clicked()
                    && let Err(err) = reveal_in_folder(output)
                {
                    error = Some(format!("폴더를 열 수 없습니다: {err}"));
                }
                let name = output.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                ui.label(RichText::new(name).strong()).on_hover_text(output.display().to_string());
                ui.label(
                    RichText::new(format!(
                        "← {} · {} · {:.1}초",
                        entry.source_name,
                        entry.engine,
                        entry.elapsed.as_secs_f32()
                    ))
                    .weak(),
                );
            });
        }
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
fn open_file(path: &Path) -> std::io::Result<()> {
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

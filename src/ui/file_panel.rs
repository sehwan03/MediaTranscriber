//! 메인 화면 왼쪽: 파일 끌어다 놓기 영역 + 파일(작업) 목록.
//!
//! 파일 목록과 작업 상태를 한 목록에 보여준다. 각 행은 아이콘 · 이름 · 정보(또는 진행률) · 상태 배지 · 동작 버튼.
//! 대량 목록에서도 가볍도록 화면에 보이는 행만 그린다.

use egui::{Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, pos2, vec2};

use super::theme::{self, Icon, Tone};
use crate::app::{AppState, InputMode, JobState, ProbeState, StatusLevel};
use crate::media::format_duration;
use crate::utils::format_bytes;

const ROW_H: f32 = 58.0;

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    mode_switch(ui, state);
    if state.input_mode == InputMode::Url {
        super::url_panel::show(ui, state);
        return;
    }
    drop_zone(ui, state);
    if state.files.is_empty() {
        return; // 비어 있으면 큰 끌어다 놓기 영역 자체가 안내 화면
    }
    ui.add_space(14.0);
    show_list(ui, state);
}

/// 작업 목록 (파일 · URL 영상 공통)
pub fn show_list(ui: &mut egui::Ui, state: &mut AppState) {
    list_header(ui, state);
    ui.add_space(6.0);
    file_list(ui, state);
}

fn mode_switch(ui: &mut egui::Ui, state: &mut AppState) {
    ui.allocate_ui(vec2(ui.available_width().min(300.0), 32.0), |ui| {
        theme::segmented(ui, &mut state.input_mode, &[(InputMode::Url, "URL 변환"), (InputMode::Files, "파일 변환")], true);
    });
    ui.add_space(12.0);
}

// ---------------------------------------------------------------- 끌어다 놓기 영역

fn drop_zone(ui: &mut egui::Ui, state: &mut AppState) {
    let hovering_files = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let compact = !state.files.is_empty();
    let height = if compact { 76.0 } else { (ui.available_height() - 4.0).clamp(220.0, 420.0) };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let painter = ui.painter();

    let active = hovering_files || response.hovered();
    let fill = if hovering_files {
        Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 26)
    } else if response.hovered() {
        theme::CARD_HOVER
    } else {
        theme::CARD
    };
    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
    // 점선 테두리
    let border = if active { theme::ACCENT } else { theme::BORDER };
    let r = rect.shrink(0.5);
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for pair in corners.windows(2) {
        for shape in egui::Shape::dashed_line(pair, Stroke::new(1.2, border), 6.0, 5.0) {
            painter.add(shape);
        }
    }

    let title = if hovering_files { "여기에 놓으면 추가됩니다" } else { "파일을 끌어다 놓거나 클릭해서 선택하세요" };
    let formats = "MP4 · MKV · AVI · MOV · WEBM · MP3 · WAV · M4A · FLAC · AAC · OGG";
    let icon_color = if active { theme::ACCENT_HOVER } else { theme::TEXT_DIM };
    if compact {
        let icon_rect = Rect::from_center_size(pos2(rect.left() + 44.0, rect.center().y), vec2(30.0, 30.0));
        theme::paint_icon(painter, icon_rect, Icon::Upload, icon_color);
        painter.text(pos2(rect.left() + 76.0, rect.center().y - 9.0), Align2::LEFT_CENTER, title, theme::bold(14.5), theme::TEXT);
        painter.text(pos2(rect.left() + 76.0, rect.center().y + 12.0), Align2::LEFT_CENTER, formats, FontId::proportional(12.0), theme::TEXT_FAINT);
    } else {
        let cy = rect.center().y - 40.0;
        let halo = Rect::from_center_size(pos2(rect.center().x, cy), vec2(64.0, 64.0));
        painter.rect_filled(halo, CornerRadius::same(255), Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 30));
        theme::paint_icon(painter, halo.shrink(16.0), Icon::Upload, theme::ACCENT_HOVER);
        painter.text(pos2(rect.center().x, cy + 56.0), Align2::CENTER_CENTER, title, theme::bold(17.0), theme::TEXT);
        painter.text(pos2(rect.center().x, cy + 84.0), Align2::CENTER_CENTER, formats, FontId::proportional(12.5), theme::TEXT_DIM);
        painter.text(
            pos2(rect.center().x, cy + 108.0),
            Align2::CENTER_CENTER,
            "폴더를 놓으면 그 안의 미디어 파일을 모두 추가합니다",
            FontId::proportional(12.0),
            theme::TEXT_FAINT,
        );
    }

    // 오른쪽 위: 폴더 추가 버튼 (영역 자체를 클릭하면 파일 선택)
    // ui.put 은 레이아웃 커서를 옮겨 아래 위젯과 겹치므로 직접 그린다.
    let btn = if compact {
        Rect::from_center_size(pos2(rect.right() - 64.0, rect.center().y), vec2(104.0, 32.0))
    } else {
        Rect::from_center_size(pos2(rect.center().x, rect.center().y + 120.0), vec2(120.0, 34.0))
    };
    let folder = ui.interact(btn, ui.scope_id().with("drop-folder"), Sense::click());
    let btn_fill = if folder.hovered() { Color32::from_rgb(0x33, 0x3A, 0x47) } else { Color32::from_rgb(0x2A, 0x30, 0x3B) };
    ui.painter().rect_filled(btn, CornerRadius::same(theme::RADIUS_SM), btn_fill);
    let folder_icon = Rect::from_center_size(pos2(btn.left() + 20.0, btn.center().y), vec2(16.0, 16.0));
    theme::paint_icon(ui.painter(), folder_icon, Icon::Folder, theme::TEXT_DIM);
    ui.painter().text(pos2(btn.left() + 34.0, btn.center().y), Align2::LEFT_CENTER, "폴더 추가", FontId::proportional(13.0), theme::TEXT);
    let folder_clicked = folder.clicked();
    if folder_clicked {
        pick_folder(state);
    } else if response.clicked() {
        pick_files(state);
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

// ---------------------------------------------------------------- 목록 머리

struct Counts {
    done: usize,
    failed: usize,
    active: usize,
    cancelled: usize,
    waiting: usize,
    progress: f32,
}

fn counts(state: &AppState) -> Counts {
    let mut c = Counts { done: 0, failed: 0, active: 0, cancelled: 0, waiting: 0, progress: 0.0 };
    for f in &state.files {
        match (&f.probe, &f.job) {
            (ProbeState::Failed(_), _) | (_, JobState::Failed(_)) => c.failed += 1,
            (_, JobState::Completed { .. }) => {
                c.done += 1;
                c.progress += 1.0;
            }
            (_, JobState::Running { progress, .. }) => {
                c.active += 1;
                c.progress += progress;
            }
            (_, JobState::Queued { .. }) => c.active += 1,
            (_, JobState::Cancelled) => c.cancelled += 1,
            _ => c.waiting += 1,
        }
    }
    c
}

fn list_header(ui: &mut egui::Ui, state: &mut AppState) {
    let c = counts(state);
    ui.horizontal(|ui| {
        ui.label(RichText::new("파일").font(theme::bold(16.0)).color(theme::TEXT));
        ui.label(RichText::new(state.files.len().to_string()).font(theme::bold(16.0)).color(theme::TEXT_FAINT));
        ui.add_space(8.0);
        if c.done > 0 {
            theme::pill(ui, &format!("완료 {}", c.done), Tone::Success);
        }
        if c.active > 0 {
            theme::pill(ui, &format!("진행 {}", c.active), Tone::Info);
        }
        if c.failed > 0 {
            theme::pill(ui, &format!("실패 {}", c.failed), Tone::Danger);
        }
        if c.cancelled > 0 {
            theme::pill(ui, &format!("취소 {}", c.cancelled), Tone::Neutral);
        }
        if c.waiting > 0 {
            theme::pill(ui, &format!("대기 {}", c.waiting), Tone::Neutral);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(!state.files.is_empty(), egui::Button::new(RichText::new("모두 지우기").size(13.0)))
                .on_hover_text("목록을 비웁니다. 진행 중인 작업은 취소되고, 저장된 TXT 는 남습니다.")
                .clicked()
            {
                state.clear_files();
            }
            if ui
                .add_enabled(c.done > 0 || c.failed > 0, egui::Button::new(RichText::new("완료·실패 지우기").size(13.0)))
                .on_hover_text("목록에서만 지웁니다. 저장된 TXT 파일과 결과 목록은 그대로 남습니다.")
                .clicked()
            {
                state.clear_finished();
            }
            if c.active > 0 {
                let total = (c.done + c.active).max(1) as f32;
                ui.label(RichText::new(format!("{:.0}%", c.progress / total * 100.0)).size(12.0).color(theme::TEXT_DIM));
                let (rect, _) = ui.allocate_exact_size(vec2(140.0, 6.0), Sense::hover());
                theme::paint_progress(ui.painter(), rect, c.progress / total, theme::ACCENT);
            }
        });
    });
}

// ---------------------------------------------------------------- 목록

enum RowAction {
    Cancel(u64),
    Remove(usize),
    Open(std::path::PathBuf),
}

fn file_list(ui: &mut egui::Ui, state: &mut AppState) {
    let frame = egui::Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(6, 6));

    if state.files.is_empty() {
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_space(28.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("아직 등록된 파일이 없습니다").color(theme::TEXT_DIM));
                ui.label(RichText::new("변환할 동영상이나 음성 파일을 추가하세요").size(12.0).color(theme::TEXT_FAINT));
            });
            ui.add_space(28.0);
        });
        return;
    }

    let current_engine = state.engine_label();
    let mut action = None;
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
            ui,
            ROW_H,
            state.files.len(),
            |ui, range| {
                for index in range {
                    if let Some(a) = file_row(ui, state, index, &current_engine) {
                        action = Some(a);
                    }
                }
            },
        );
    });

    match action {
        Some(RowAction::Cancel(id)) => state.cancel_job(id),
        Some(RowAction::Remove(index)) => state.remove_file(index),
        Some(RowAction::Open(path)) => {
            if let Err(err) = super::result_panel::open_file(&path) {
                state.set_status(StatusLevel::Error, format!("파일을 열 수 없습니다: {err}"));
            }
        }
        None => {}
    }
}

fn file_row(ui: &mut egui::Ui, state: &AppState, index: usize, current_engine: &str) -> Option<RowAction> {
    let file = &state.files[index];
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::CARD_HOVER);
    }

    // 상태별 표시
    let cancelling = match &file.job {
        JobState::Queued { cancel } | JobState::Running { cancel, .. } => cancel.load(std::sync::atomic::Ordering::Relaxed),
        _ => false,
    };
    let (status, tone): (&str, Tone) = match (&file.probe, &file.job) {
        (ProbeState::Pending, _) => ("검사 중", Tone::Neutral),
        (ProbeState::Failed(_), _) => ("실패", Tone::Danger),
        (_, JobState::Queued { .. } | JobState::Running { .. }) if cancelling => ("취소 중", Tone::Warn),
        (_, JobState::Idle) => ("대기", Tone::Neutral),
        (_, JobState::Queued { .. }) => ("대기열", Tone::Info),
        (_, JobState::Running { .. }) => ("처리 중", Tone::Info),
        (_, JobState::Completed { .. }) => ("완료", Tone::Success),
        (_, JobState::Failed(_)) => ("실패", Tone::Danger),
        (_, JobState::Cancelled) => ("취소됨", Tone::Neutral),
    };

    // 아이콘 (동영상은 보라, 음성은 파랑)
    let is_video = match &file.probe {
        ProbeState::Ready(info) => info.has_video,
        _ => file
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["mp4", "mkv", "avi", "mov", "webm"].iter().any(|v| v.eq_ignore_ascii_case(e))),
    };
    let icon_box = Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(36.0, 36.0));
    let icon_color = if is_video { Color32::from_rgb(0xA7, 0x8B, 0xFA) } else { theme::ACCENT_HOVER };
    painter.rect_filled(
        icon_box,
        CornerRadius::same(10),
        Color32::from_rgba_unmultiplied(icon_color.r(), icon_color.g(), icon_color.b(), 30),
    );
    theme::paint_icon(&painter, icon_box.shrink(8.0), if is_video { Icon::Video } else { Icon::Audio }, icon_color);

    // 오른쪽: 동작 버튼 + 상태 배지
    let btn = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(28.0, 28.0));
    let btn_id = ui.scope_id().with(("row-action", file.id));
    let mut action = None;
    if file.job.is_active() {
        if theme::icon_button(ui, btn, btn_id, Icon::Close, "이 작업 취소", theme::TEXT_DIM) {
            action = Some(RowAction::Cancel(file.id));
        }
    } else if let JobState::Completed { output, .. } = &file.job {
        if theme::icon_button(ui, btn, btn_id, Icon::FileText, "결과 TXT 열기", theme::SUCCESS) {
            action = Some(RowAction::Open(output.clone()));
        }
    } else if theme::icon_button(ui, btn, btn_id, Icon::Trash, "목록에서 제거", theme::TEXT_FAINT) {
        action = Some(RowAction::Remove(index));
    }
    let pill_rect = theme::paint_pill_right(&painter, pos2(btn.left() - 8.0, rect.top() + 19.0), status, tone);

    // 이름 (배지 왼쪽까지)
    let text_left = icon_box.right() + 12.0;
    let name_width = pill_rect.left() - 12.0 - text_left;
    let name = theme::truncated(&painter, &file.display_name, theme::bold(14.0), theme::TEXT, name_width);
    painter.galley(pos2(text_left, rect.top() + 10.0), name, theme::TEXT);

    // 둘째 줄: 진행률 또는 정보
    let line2_y = rect.top() + 39.0;
    let line_width = btn.left() - 12.0 - text_left;
    match &file.job {
        JobState::Running { progress, stage, .. } if !cancelling => {
            let bar = Rect::from_min_size(pos2(text_left, line2_y - 3.0), vec2((line_width - 130.0).max(60.0), 6.0));
            theme::paint_progress(&painter, bar, *progress, theme::ACCENT);
            painter.text(
                pos2(bar.right() + 10.0, line2_y),
                Align2::LEFT_CENTER,
                format!("{:.0}% · {}", progress * 100.0, stage.label()),
                FontId::proportional(12.0),
                theme::TEXT_DIM,
            );
        }
        _ => {
            let mut parts: Vec<String> = Vec::new();
            match &file.probe {
                ProbeState::Ready(info) => {
                    parts.push(info.duration.map_or_else(|| "길이 알 수 없음".to_owned(), format_duration));
                }
                ProbeState::Pending => parts.push("미디어 정보 확인 중".to_owned()),
                ProbeState::Failed(_) => {}
            }
            if file.url.is_some() {
                parts.push("URL".to_owned());
            } else {
                parts.push(format_bytes(file.size_bytes));
            }
            let engine = match (&file.job, &file.engine_label) {
                (JobState::Idle, _) | (_, None) => current_engine.to_owned(),
                (_, Some(label)) => label.clone(),
            };
            let (text, color) = match (&file.probe, &file.job) {
                (ProbeState::Failed(msg), _) | (_, JobState::Failed(msg)) => (msg.clone(), theme::DANGER),
                (_, JobState::Completed { elapsed, .. }) => {
                    parts.push(engine);
                    parts.push(format!("{:.1}초 만에 완료", elapsed.as_secs_f32()));
                    (parts.join(" · "), theme::TEXT_DIM)
                }
                _ => {
                    parts.push(engine);
                    (parts.join(" · "), theme::TEXT_DIM)
                }
            };
            let galley = theme::truncated(&painter, &text, FontId::proportional(12.0), color, line_width);
            painter.galley(pos2(text_left, line2_y - galley.size().y / 2.0), galley, color);
        }
    }

    // 행 구분선
    if index + 1 < state.files.len() {
        painter.line_segment(
            [pos2(text_left, rect.bottom() + 1.0), pos2(rect.right() - 8.0, rect.bottom() + 1.0)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 8)),
        );
    }

    // 마우스를 올리면 전체 경로와 오류 내용을 보여준다
    if response.hovered() {
        // URL 은 토큰이 붙어 있을 수 있으므로 쿼리를 지운 주소만 보여준다
        let source = match &file.url {
            Some(v) => format!("{}\n{}", file.display_name, crate::url::safety::redact_url(&v.source_url)),
            None => file.path.display().to_string(),
        };
        let detail = match (&file.probe, &file.job) {
            (ProbeState::Failed(msg), _) | (_, JobState::Failed(msg)) => format!("{source}\n\n{msg}"),
            (_, JobState::Completed { output, .. }) => format!("{source}\n→ {}", output.display()),
            (ProbeState::Ready(info), _) => {
                let kind = if info.has_video { "동영상" } else { "음성" };
                format!("{source}\n{kind} · {} · 오디오 {}", info.format_name, info.audio.codec)
            }
            _ => source,
        };
        response.on_hover_text(detail);
    }
    action
}

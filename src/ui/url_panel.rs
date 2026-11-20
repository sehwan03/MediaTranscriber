//! URL 변환 화면 (메인 왼쪽, URL 모드).
//!
//! 1. 주소 입력 + 로그인 세션 선택  2. 영상 찾기  3. 찾은 영상 목록에서 직접 선택
//!
//! 선택한 영상은 오른쪽 「선택한 영상 변환」을 누를 때 기존 작업 목록에 들어가 같은 과정으로 변환된다.
//! (여러 개가 나와도 자동으로 고르거나 변환하지 않는다)

use egui::{Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};

use super::theme::{self, Icon, Tone};
use crate::app::{AppState, Discovery, ToolStatus};
use crate::media::format_duration;
use crate::url::{Browser, UrlAuth, UrlVideo};

const ROW_H: f32 = 56.0;

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    ytdlp_banner(ui, state);
    input_card(ui, state);
    ui.add_space(12.0);
    results(ui, state);
    if !state.downloads.is_empty() {
        ui.add_space(14.0);
        downloads(ui, state);
    }
    if !state.files.is_empty() {
        ui.add_space(14.0);
        super::file_panel::show_list(ui, state);
    }
}

fn ytdlp_banner(ui: &mut egui::Ui, state: &mut AppState) {
    let u = &mut state.url_ui;
    let missing = matches!(u.ytdlp, ToolStatus::Missing(_));
    if !missing && u.ytdlp_install.is_none() {
        return;
    }
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(0xFB, 0xBF, 0x24, 18))
        .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xFB, 0xBF, 0x24, 70)))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match &u.ytdlp_install {
                Some(p) => {
                    ui.horizontal(|ui| {
                        ui.label("구성 요소 설치 중…");
                        let frac = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
                        let (rect, _) = ui.allocate_exact_size(vec2(200.0, 6.0), Sense::hover());
                        theme::paint_progress(ui.painter(), rect, frac, theme::ACCENT);
                        if ui.small_button("취소").clicked() {
                            p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    });
                }
                None => {
                    ui.label(RichText::new("URL 변환을 쓰려면 구성 요소를 한 번 설치해야 합니다.").color(theme::WARN));
                    ui.horizontal_wrapped(|ui| {
                        let label = format!("설치 (약 {} MB)", crate::url::installer::APPROX_DOWNLOAD_MB);
                        if ui.button(RichText::new(label).strong()).clicked() {
                            u.ytdlp_install_requested = true;
                        }
                    });
                }
            }
        });
    ui.add_space(12.0);
}

fn input_card(ui: &mut egui::Ui, state: &mut AppState) {
    let running = matches!(state.url_ui.discovery, Discovery::Running { .. });
    let ready = matches!(state.url_ui.ytdlp, ToolStatus::Found { .. });
    theme::card(ui, |ui| {
        theme::card_title(ui, "영상 주소", None);
        ui.horizontal(|ui| {
            let btn_w = 104.0;
            let edit = egui::TextEdit::singleline(&mut state.url_ui.input)
                .hint_text("https://…")
                .desired_width(ui.available_width() - btn_w - 8.0)
                .margin(vec2(10.0, 7.0));
            let resp = ui.add_enabled(!running, edit);
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if running {
                if ui.add_sized(vec2(btn_w, 32.0), egui::Button::new("탐색 취소")).clicked()
                    && let Discovery::Running { cancel } = &state.url_ui.discovery
                {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                let can = ready && !state.url_ui.input.trim().is_empty();
                let clicked = ui
                    .add_enabled(can, theme::primary_button("영상 찾기").min_size(vec2(btn_w, 32.0)))
                    .on_disabled_hover_text(if ready { "주소를 입력하세요" } else { "구성 요소를 먼저 설치하세요" })
                    .clicked();
                if (clicked || enter) && can {
                    state.url_ui.discover_requested = true;
                }
            }
        });
        ui.add_space(8.0);
        auth_row(ui, state, !running);
    });
}

fn auth_row(ui: &mut egui::Ui, state: &mut AppState, enabled: bool) {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        None,
        Browser,
        File,
    }
    let auth = &mut state.settings.url_auth;
    let mut kind = match auth {
        UrlAuth::None => Kind::None,
        UrlAuth::Browser(_) => Kind::Browser,
        UrlAuth::CookieFile(_) => Kind::File,
    };
    theme::field_label(ui, "로그인");
    ui.allocate_ui(vec2(ui.available_width().min(420.0), 30.0), |ui| {
        if theme::segmented(ui, &mut kind, &[(Kind::None, "필요 없음"), (Kind::Browser, "브라우저"), (Kind::File, "쿠키 파일")], enabled) {
            *auth = match kind {
                Kind::None => UrlAuth::None,
                Kind::Browser => UrlAuth::Browser(Browser::Edge),
                Kind::File => UrlAuth::CookieFile(std::path::PathBuf::new()),
            };
        }
    });
    match auth {
        UrlAuth::None => {}
        UrlAuth::Browser(b) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("브라우저").color(theme::TEXT_DIM));
                ui.add_enabled_ui(enabled, |ui| {
                    egui::ComboBox::from_id_salt("url_browser").selected_text(b.label()).width(140.0).show_ui(ui, |ui| {
                        for option in Browser::ALL {
                            ui.selectable_value(b, option, option.label());
                        }
                    });
                });
            });
            ui.label(
                RichText::new("이 브라우저에 로그인해 둔 상태로 영상을 받습니다. 잘 안 되면 브라우저를 닫고 다시 시도하세요.")
                .size(11.5)
                .color(theme::TEXT_FAINT),
            );
        }
        UrlAuth::CookieFile(path) => {
            ui.horizontal(|ui| {
                let text = if path.as_os_str().is_empty() { "파일을 선택하세요".to_owned() } else { path.display().to_string() };
                ui.add_sized(
                    vec2((ui.available_width() - 90.0).max(120.0), 22.0),
                    egui::Label::new(RichText::new(&text).monospace().size(12.0).color(theme::TEXT_DIM)).truncate(),
                );
                if ui.add_enabled(enabled, egui::Button::new("선택…")).clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .set_title("쿠키 파일 선택")
                        .add_filter("쿠키 파일", &["txt"])
                        .pick_file()
                {
                    *path = p;
                }
            });
        }
    }
}

fn results(ui: &mut egui::Ui, state: &mut AppState) {
    match &mut state.url_ui.discovery {
        Discovery::Idle => {
            theme::card(ui, |ui| {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("주소를 입력하고 「영상 찾기」를 누르세요").color(theme::TEXT_DIM));
                    ui.label(
                        RichText::new("페이지에서 찾은 영상이 여기에 나열됩니다. 변환할 영상을 직접 고를 수 있습니다.")
                            .size(12.0)
                            .color(theme::TEXT_FAINT),
                    );
                });
                ui.add_space(6.0);
            });
        }
        Discovery::Running { .. } => {
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("영상을 찾는 중… 페이지에 따라 수십 초 걸릴 수 있습니다").color(theme::TEXT_DIM));
                });
            });
        }
        Discovery::Failed(msg) => {
            theme::card(ui, |ui| {
                ui.label(RichText::new("영상을 찾지 못했습니다").font(theme::bold(14.0)).color(theme::DANGER));
                ui.label(RichText::new(msg.as_str()).size(12.5).color(theme::TEXT_DIM));
            });
        }
        Discovery::Done { videos } => {
            let selectable = videos.iter().filter(|(v, _)| v.unsupported_reason().is_none()).count();
            let selected = videos.iter().filter(|(_, s)| *s).count();
            ui.horizontal(|ui| {
                ui.label(RichText::new("찾은 영상").font(theme::bold(16.0)).color(theme::TEXT));
                ui.label(RichText::new(videos.len().to_string()).font(theme::bold(16.0)).color(theme::TEXT_FAINT));
                if selected > 0 {
                    theme::pill(ui, &format!("선택 {selected}"), Tone::Info);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(selected > 0, egui::Button::new(RichText::new("선택 해제").size(13.0))).clicked() {
                        videos.iter_mut().for_each(|(_, s)| *s = false);
                    }
                    if ui.add_enabled(selectable > selected, egui::Button::new(RichText::new("모두 선택").size(13.0))).clicked() {
                        videos.iter_mut().filter(|(v, _)| v.unsupported_reason().is_none()).for_each(|(_, s)| *s = true);
                    }
                });
            });
            ui.add_space(6.0);
            let max_h = if state.files.is_empty() && state.downloads.is_empty() { ui.available_height() } else { (ui.available_height() * 0.45).max(ROW_H * 2.5) };
            egui::Frame::new()
                .fill(theme::CARD)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.0;
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show_rows(
                        ui,
                        ROW_H,
                        videos.len(),
                        |ui, range| {
                            for i in range {
                                let (video, selected) = &mut videos[i];
                                video_row(ui, video, selected, i);
                            }
                        },
                    );
                });
            let mut save = false;
            ui.horizontal(|ui| {
                let hint = if selected == 0 {
                    "변환할 영상을 선택하세요 (여러 개 선택 가능)"
                } else {
                    "오른쪽 「선택한 영상 변환」을 누르면 아래 작업 목록에 추가되어 변환됩니다."
                };
                ui.label(RichText::new(hint).size(12.0).color(theme::TEXT_FAINT));
                if state.settings.video_download {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        save = ui
                            .add_enabled(selected > 0, egui::Button::new(RichText::new(format!("선택한 영상 저장 · {selected}개")).size(13.0)))
                            .on_hover_text("최고 화질 영상 파일로 저장합니다 (변환과 별개)")
                            .on_disabled_hover_text("저장할 영상을 선택하세요")
                            .clicked();
                    });
                }
            });
            if save {
                let n = state.queue_selected_downloads();
                state.set_status(crate::app::StatusLevel::Info, format!("영상 {n}개 저장을 시작합니다."));
            }
        }
    }
}

// ---------------------------------------------------------------- 영상 저장 목록

enum DownloadAction {
    Cancel(usize),
    Remove(usize),
    Open(std::path::PathBuf),
    Folder(std::path::PathBuf),
}

fn downloads(ui: &mut egui::Ui, state: &mut AppState) {
    use crate::app::DownloadState;
    let done = state.downloads.iter().filter(|d| !d.is_active()).count();
    ui.horizontal(|ui| {
        ui.label(RichText::new("영상 저장").font(theme::bold(16.0)).color(theme::TEXT));
        ui.label(RichText::new(state.downloads.len().to_string()).font(theme::bold(16.0)).color(theme::TEXT_FAINT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(done > 0, egui::Button::new(RichText::new("끝난 항목 지우기").size(13.0))).clicked() {
                state.downloads.retain(|d| d.is_active());
            }
        });
    });
    ui.add_space(6.0);
    let mut action = None;
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            egui::ScrollArea::vertical().id_salt("downloads").max_height(ROW_H * 3.2).auto_shrink([false, true]).show(ui, |ui| {
                for (i, d) in state.downloads.iter().enumerate() {
                    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    let painter = ui.painter().clone();
                    if response.hovered() {
                        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::CARD_HOVER);
                    }
                    let icon_box = Rect::from_center_size(pos2(rect.left() + 24.0, rect.center().y), vec2(34.0, 34.0));
                    let color = Color32::from_rgb(0xA7, 0x8B, 0xFA);
                    painter.rect_filled(icon_box, CornerRadius::same(10), Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 30));
                    theme::paint_icon(&painter, icon_box.shrink(8.0), Icon::Video, color);

                    // 오른쪽 동작 버튼
                    let btn = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(28.0, 28.0));
                    let id = ui.scope_id().with(("dl-action", d.id));
                    match &d.state {
                        _ if d.is_active() => {
                            if theme::icon_button(ui, btn, id, Icon::Close, "저장 취소", theme::TEXT_DIM) {
                                action = Some(DownloadAction::Cancel(i));
                            }
                        }
                        DownloadState::Done(path) => {
                            if theme::icon_button(ui, btn, id, Icon::Folder, "저장 폴더 열기", theme::SUCCESS) {
                                action = Some(DownloadAction::Folder(path.clone()));
                            }
                        }
                        _ => {
                            if theme::icon_button(ui, btn, id, Icon::Trash, "목록에서 제거", theme::TEXT_FAINT) {
                                action = Some(DownloadAction::Remove(i));
                            }
                        }
                    }
                    let cancelling = d.is_active() && d.cancel.load(std::sync::atomic::Ordering::Relaxed);
                    let (status, tone) = match &d.state {
                        _ if cancelling => ("취소 중", Tone::Warn),
                        DownloadState::Queued => ("대기", Tone::Neutral),
                        DownloadState::Running { .. } => ("저장 중", Tone::Info),
                        DownloadState::Done(_) => ("완료", Tone::Success),
                        DownloadState::Failed(_) => ("실패", Tone::Danger),
                        DownloadState::Cancelled => ("취소됨", Tone::Neutral),
                    };
                    let pill = theme::paint_pill_right(&painter, pos2(btn.left() - 8.0, rect.top() + 18.0), status, tone);
                    let left = icon_box.right() + 12.0;
                    let title = theme::truncated(&painter, &d.title, theme::bold(14.0), theme::TEXT, pill.left() - 12.0 - left);
                    painter.galley(pos2(left, rect.top() + 8.0), title, theme::TEXT);

                    let line_y = rect.top() + 38.0;
                    let width = btn.left() - 12.0 - left;
                    match &d.state {
                        DownloadState::Running { progress, merging } if !cancelling => {
                            let bar = Rect::from_min_size(pos2(left, line_y - 3.0), vec2((width - 130.0).max(60.0), 6.0));
                            theme::paint_progress(&painter, bar, *progress, theme::ACCENT);
                            let label = if *merging { "영상·소리 합치는 중".to_owned() } else { format!("{:.0}% · 받는 중", progress * 100.0) };
                            painter.text(pos2(bar.right() + 10.0, line_y), Align2::LEFT_CENTER, label, FontId::proportional(12.0), theme::TEXT_DIM);
                        }
                        other => {
                            let (text, color) = match other {
                                DownloadState::Done(p) => (
                                    p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                                    theme::TEXT_DIM,
                                ),
                                DownloadState::Failed(msg) => (msg.clone(), theme::DANGER),
                                DownloadState::Cancelled => ("저장을 취소했습니다".to_owned(), theme::TEXT_FAINT),
                                _ => ("최고 화질로 저장 대기 중".to_owned(), theme::TEXT_FAINT),
                            };
                            let g = theme::truncated(&painter, &text, FontId::proportional(12.0), color, width);
                            painter.galley(pos2(left, line_y - g.size().y / 2.0), g, color);
                        }
                    }
                    if let DownloadState::Done(p) = &d.state {
                        if response.double_clicked() {
                            action = Some(DownloadAction::Open(p.clone()));
                        }
                        response.on_hover_text(format!("{}\n두 번 클릭하면 재생", p.display()));
                    }
                }
            });
        });

    match action {
        Some(DownloadAction::Cancel(i)) => state.downloads[i].cancel.store(true, std::sync::atomic::Ordering::Relaxed),
        Some(DownloadAction::Remove(i)) => {
            state.downloads.remove(i);
        }
        Some(DownloadAction::Open(p)) => {
            if let Err(err) = super::result_panel::open_file(&p) {
                state.set_status(crate::app::StatusLevel::Error, format!("파일을 열 수 없습니다: {err}"));
            }
        }
        Some(DownloadAction::Folder(p)) => {
            let dir = p.parent().map_or_else(|| state.output_dir().to_path_buf(), std::path::Path::to_path_buf);
            if let Err(err) = super::result_panel::open_folder(&dir) {
                state.set_status(crate::app::StatusLevel::Error, format!("폴더를 열 수 없습니다: {err}"));
            }
        }
        None => {}
    }
}

fn video_row(ui: &mut egui::Ui, video: &UrlVideo, selected: &mut bool, index: usize) {
    let unsupported = video.unsupported_reason();
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), if unsupported.is_none() { Sense::click() } else { Sense::hover() });
    let painter = ui.painter().clone();
    if *selected {
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 34));
    } else if response.hovered() && unsupported.is_none() {
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::CARD_HOVER);
    }
    if response.clicked() && unsupported.is_none() {
        *selected = !*selected;
    }

    // 체크 상자
    let check = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(18.0, 18.0));
    if *selected {
        painter.rect_filled(check, CornerRadius::same(5), theme::ACCENT);
        let c = check.center();
        painter.line_segment([pos2(c.x - 4.5, c.y), pos2(c.x - 1.0, c.y + 3.5)], Stroke::new(2.0, Color32::WHITE));
        painter.line_segment([pos2(c.x - 1.0, c.y + 3.5), pos2(c.x + 5.0, c.y - 4.0)], Stroke::new(2.0, Color32::WHITE));
    } else {
        let color = if unsupported.is_some() { theme::TEXT_FAINT } else { theme::TEXT_DIM };
        painter.rect_stroke(check, CornerRadius::same(5), Stroke::new(1.5, color), StrokeKind::Inside);
    }

    // 아이콘
    let icon_box = Rect::from_center_size(pos2(rect.left() + 58.0, rect.center().y), vec2(34.0, 34.0));
    let color = if video.has_video { Color32::from_rgb(0xA7, 0x8B, 0xFA) } else { theme::ACCENT_HOVER };
    painter.rect_filled(icon_box, CornerRadius::same(10), Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 30));
    theme::paint_icon(&painter, icon_box.shrink(8.0), if video.has_video { Icon::Video } else { Icon::Audio }, color);

    // 제목 + 정보
    let left = icon_box.right() + 12.0;
    let width = rect.right() - 16.0 - left;
    let title_color = if unsupported.is_some() { theme::TEXT_FAINT } else { theme::TEXT };
    let index_label = video.playlist_index.map(|i| format!("{i}. ")).unwrap_or_default();
    let title = theme::truncated(&painter, &format!("{index_label}{}", video.title), theme::bold(14.0), title_color, width);
    painter.galley(pos2(left, rect.top() + 9.0), title, title_color);
    let mut meta = vec![
        video.duration.map_or_else(|| "길이 알 수 없음".to_owned(), format_duration),
        video.ext.clone(),
    ];
    if let Some(r) = &video.resolution {
        meta.push(r.clone());
    }
    if !video.has_video {
        meta.push("오디오".to_owned());
    }
    let (text, color) = match unsupported {
        Some(reason) => (reason.to_owned(), theme::WARN),
        None => (meta.join(" · "), theme::TEXT_DIM),
    };
    let g = theme::truncated(&painter, &text, FontId::proportional(12.0), color, width);
    painter.galley(pos2(left, rect.top() + 32.0), g, color);
    let _ = (Align2::LEFT_CENTER, index);
    if response.hovered() {
        response.on_hover_text(format!("{}\n{}", video.title, crate::url::safety::redact_url(&video.source_url)));
    }
}

use egui::RichText;

use crate::app::{AppState, JobState, ModelOpKind, ProbeState};
use crate::config::{CloudProvider, EngineMode, Language, WhisperModel};
use crate::engine_selector::Choice;
use crate::media::{MediaInfo, format_duration};
use crate::speech::model::ModelFileState;
use crate::utils::format_bytes;

const ROW_HEIGHT: f32 = 24.0;

fn media_summary(info: &MediaInfo) -> String {
    let kind = if info.has_video { "동영상" } else { "음성" };
    let rate = info.audio.sample_rate.map_or_else(String::new, |r| format!(", {r} Hz"));
    let ch = info.audio.channels.map_or_else(String::new, |c| format!(", {c}ch"));
    format!("{kind} · {} · 오디오 {}{rate}{ch}", info.format_name, info.audio.codec)
}

/// 작업 목록: 인식 설정, 시작/취소, 파일별 진행 상황.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.heading("작업 목록");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(state.has_active_jobs(), egui::Button::new("전체 취소"))
                .clicked()
            {
                state.cancel_all();
            }
            let blocker = state.start_blocker();
            let start = ui.add_enabled(blocker.is_none(), egui::Button::new("변환 시작"));
            if let Some(reason) = blocker {
                start.on_disabled_hover_text(reason);
            } else if start.clicked() {
                state.start_requested = true;
            }
        });
    });
    quick_settings(ui, state);
    auto_summary(ui, state);
    ui.separator();

    if state.files.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("등록된 파일이 없습니다.").weak());
        });
        return;
    }

    batch_summary(ui, state);

    // 열 폭: 파일명은 남은 폭을 모두 쓰고, 나머지는 고정 (창이 좁아도 취소 버튼이 밀려나지 않게)
    const FIXED: [f32; 6] = [64.0, 56.0, 150.0, 110.0, 84.0, 40.0];
    let spacing = ui.spacing().item_spacing.x;
    let scroll = &ui.spacing().scroll;
    // 세로 스크롤바가 차지하는 폭까지 빼야 마지막 열이 가려지지 않는다
    let scrollbar = scroll.bar_width + scroll.bar_inner_margin + scroll.bar_outer_margin;
    let fixed_total: f32 = FIXED.iter().sum::<f32>() + spacing * FIXED.len() as f32;
    let name_width = (ui.available_width() - fixed_total - scrollbar - 4.0).max(120.0);
    let widths = [name_width, FIXED[0], FIXED[1], FIXED[2], FIXED[3], FIXED[4], FIXED[5]];

    // 머리글 (스크롤 영역 밖에 고정)
    ui.horizontal(|ui| {
        for (title, w) in ["파일명", "크기", "길이", "엔진", "진행률", "상태", ""].iter().zip(widths) {
            cell(ui, w, |ui| {
                ui.label(RichText::new(*title).strong());
            });
        }
    });
    ui.separator();

    // 작업을 시작한 파일은 실제 사용 엔진, 아직 시작 전이면 현재 설정을 표시한다.
    let current_engine = state.engine_label();
    let mut cancel_id = None;
    let stripe = ui.visuals().faint_bg_color;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, state.files.len(), |ui, range| {
            for (offset, file) in state.files[range.clone()].iter().enumerate() {
                // 배경은 행 내용보다 먼저 그려져야 하므로 자리만 잡아 두고 나중에 채운다.
                let bg = ui.painter().add(egui::Shape::Noop);
                let row = ui.horizontal(|ui| {
                    ui.set_height(ROW_HEIGHT);
                    cell(ui, widths[0], |ui| {
                        ui.add(egui::Label::new(&file.display_name).truncate())
                            .on_hover_text(file.path.display().to_string());
                    });
                    cell(ui, widths[1], |ui| {
                        ui.label(format_bytes(file.size_bytes));
                    });
                    cell(ui, widths[2], |ui| match &file.probe {
                        ProbeState::Pending => {
                            ui.label(RichText::new("검사 중…").weak());
                        }
                        ProbeState::Ready(info) => {
                            let text = info.duration.map_or_else(|| "알 수 없음".to_owned(), format_duration);
                            ui.label(text).on_hover_text(media_summary(info));
                        }
                        ProbeState::Failed(_) => {
                            ui.label("-");
                        }
                    });
                    let engine = match (&file.job, &file.engine_label) {
                        (JobState::Idle, _) | (_, None) => &current_engine,
                        (_, Some(label)) => label,
                    };
                    cell(ui, widths[3], |ui| {
                        ui.add(egui::Label::new(engine.as_str()).truncate());
                    });
                    cell(ui, widths[4], |ui| progress_cell(ui, &file.job));
                    cell(ui, widths[5], |ui| status_cell(ui, &file.probe, &file.job));
                    cell(ui, widths[6], |ui| {
                        if file.job.is_active() && ui.small_button("취소").clicked() {
                            cancel_id = Some(file.id);
                        }
                    });
                });
                if (range.start + offset) % 2 == 1 {
                    ui.painter().set(bg, egui::epaint::RectShape::filled(row.response.rect, 0.0, stripe));
                }
            }
        });

    if let Some(id) = cancel_id {
        state.cancel_job(id);
    }
}

/// 고정 폭 셀 (왼쪽 정렬, 세로 가운데)
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(width);
            add(ui);
        },
    );
}

/// 전체 진행 요약: 완료/실패/진행/대기 수와 전체 진행률, 완료 항목 정리 버튼.
fn batch_summary(ui: &mut egui::Ui, state: &mut AppState) {
    let (mut done, mut failed, mut active, mut waiting, mut cancelled, mut progress_sum) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0.0f32);
    for f in &state.files {
        match (&f.probe, &f.job) {
            (ProbeState::Failed(_), _) | (_, JobState::Failed(_)) => failed += 1,
            (_, JobState::Completed { .. }) => {
                done += 1;
                progress_sum += 1.0;
            }
            (_, JobState::Running { progress, .. }) => {
                active += 1;
                progress_sum += progress;
            }
            (_, JobState::Queued { .. }) => active += 1,
            (_, JobState::Cancelled) => cancelled += 1,
            _ => waiting += 1,
        }
    }
    ui.horizontal(|ui| {
        ui.label(format!("전체 {}개", state.files.len()));
        ui.label(RichText::new(format!("완료 {done}")).color(egui::Color32::from_rgb(110, 200, 120)));
        if failed > 0 {
            ui.colored_label(ui.visuals().error_fg_color, format!("실패 {failed}"));
        }
        if active > 0 {
            ui.label(RichText::new(format!("진행 {active}")).strong());
        }
        if cancelled > 0 {
            ui.label(RichText::new(format!("취소 {cancelled}")).weak());
        }
        ui.label(RichText::new(format!("대기 {waiting}")).weak());
        let batch_total = done + active;
        if active > 0 && batch_total > 0 {
            ui.add(
                egui::ProgressBar::new(progress_sum / batch_total as f32)
                    .desired_width(160.0)
                    .show_percentage(),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(done > 0 || failed > 0, egui::Button::new("완료·실패 항목 지우기").small())
                .on_hover_text("목록에서만 지웁니다. 저장된 TXT 파일과 결과 목록은 그대로 남습니다.")
                .clicked()
            {
                state.clear_finished();
            }
        });
    });
}

fn progress_cell(ui: &mut egui::Ui, job: &JobState) {
    let progress = match job {
        JobState::Running { progress, .. } => *progress,
        JobState::Completed { .. } => 1.0,
        JobState::Queued { .. } => 0.0,
        _ => {
            ui.label(RichText::new("-").weak());
            return;
        }
    };
    ui.add(egui::ProgressBar::new(progress).desired_width(104.0).show_percentage());
}

fn status_cell(ui: &mut egui::Ui, probe: &ProbeState, job: &JobState) {
    let error = ui.visuals().error_fg_color;
    match (probe, job) {
        (ProbeState::Pending, _) => {
            ui.label(RichText::new("검사 중").weak());
        }
        (ProbeState::Failed(msg), _) => {
            ui.colored_label(error, "실패").on_hover_text(msg);
        }
        (ProbeState::Ready(_), job) => match job {
            JobState::Idle => {
                ui.label("대기");
            }
            JobState::Queued { cancel } | JobState::Running { cancel, .. }
                if cancel.load(std::sync::atomic::Ordering::Relaxed) =>
            {
                ui.label(RichText::new("취소 중…").weak());
            }
            JobState::Queued { .. } => {
                ui.label("대기열");
            }
            JobState::Running { stage, .. } => {
                ui.label(RichText::new(stage.label()).strong());
            }
            JobState::Completed { output, elapsed } => {
                ui.colored_label(egui::Color32::from_rgb(110, 200, 120), "완료").on_hover_text(format!(
                    "{}\n처리 시간 {:.1}초",
                    output.display(),
                    elapsed.as_secs_f32()
                ));
            }
            JobState::Failed(msg) => {
                ui.colored_label(error, "실패").on_hover_text(msg);
            }
            JobState::Cancelled => {
                ui.label(RichText::new("취소됨").weak());
            }
        },
    }
}

/// Auto 모드 추천 결과 한 줄 + 근거(마우스 오버) + 성능 측정 버튼.
fn auto_summary(ui: &mut egui::Ui, state: &mut AppState) {
    if state.settings.engine_mode != EngineMode::Auto {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        let Some(rec) = state.recommendation() else {
            ui.spinner();
            ui.label(RichText::new("PC 성능 확인 중…").weak());
            return;
        };
        let headline = match &rec.choice {
            Choice::Local { model } => format!("추천: Local · {}", model.label()),
            Choice::Cloud => format!("추천: Cloud · {}", state.settings.cloud.provider.label()),
            Choice::Unavailable => "추천 불가".to_owned(),
        };
        let color = match rec.choice {
            Choice::Unavailable => ui.visuals().error_fg_color,
            _ => ui.visuals().strong_text_color(),
        };
        ui.colored_label(color, RichText::new(headline).strong())
            .on_hover_text(rec.reasons.join("\n"));
        if let Some(local) = &rec.local {
            let time = local
                .total_time
                .map(|t| format!(" · 로컬 예상 {}", format_duration(t)))
                .unwrap_or_default();
            ui.label(
                RichText::new(format!("로컬 약 {:.1}배속 ({}){time}", local.rtf, local.source.label())).weak(),
            )
            .on_hover_text(rec.reasons.join("\n"));
        }
        if rec.needs_download {
            ui.colored_label(ui.visuals().warn_fg_color, "모델 다운로드 필요");
        }
        let model = rec.local.as_ref().map_or(state.settings.whisper_model, |l| l.model);
        let installed = state.model_file_state(model) == ModelFileState::Installed;
        let busy = state.benchmark_running.is_some() || state.has_active_jobs();
        if state.benchmark_running.is_some() {
            ui.spinner();
            ui.label("성능 측정 중…");
        } else if ui
            .add_enabled(installed && !busy, egui::Button::new("성능 측정").small())
            .on_hover_text("등록된 파일의 앞 30초(없으면 합성 신호)로 이 PC의 실제 처리 속도를 잽니다.")
            .on_disabled_hover_text("모델 설치 후, 변환 중이 아닐 때 측정할 수 있습니다.")
            .clicked()
        {
            state.benchmark_requested = Some(model);
        }
    });
}

/// 메인 화면의 인식 설정 (모드/모델/언어) 과 모델 설치 상태.
fn quick_settings(ui: &mut egui::Ui, state: &mut AppState) {
    let locked = state.has_active_jobs();
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!locked, |ui| {
            ui.label("모드");
            egui::ComboBox::from_id_salt("quick_mode")
                .selected_text(state.settings.engine_mode.label())
                .show_ui(ui, |ui| {
                    for mode in EngineMode::ALL {
                        ui.selectable_value(&mut state.settings.engine_mode, mode, mode.label());
                    }
                });
            if !state.uses_cloud() || state.settings.engine_mode == EngineMode::Auto {
                let label = if state.settings.engine_mode == EngineMode::Auto { "선호 모델" } else { "모델" };
                ui.label(label);
                egui::ComboBox::from_id_salt("quick_model")
                    .selected_text(state.settings.whisper_model.label())
                    .show_ui(ui, |ui| {
                        for model in WhisperModel::ALL {
                            ui.selectable_value(&mut state.settings.whisper_model, model, model.label());
                        }
                    });
            }
            ui.label("언어");
            egui::ComboBox::from_id_salt("quick_lang")
                .selected_text(state.settings.language.label())
                .show_ui(ui, |ui| {
                    for lang in Language::ALL {
                        ui.selectable_value(&mut state.settings.language, lang, lang.label());
                    }
                });
        });

        if state.uses_cloud() {
            ui.add_enabled_ui(!locked, |ui| {
                ui.label("공급자");
                egui::ComboBox::from_id_salt("quick_provider")
                    .selected_text(state.settings.cloud.provider.label())
                    .show_ui(ui, |ui| {
                        for provider in CloudProvider::ALL {
                            ui.selectable_value(&mut state.settings.cloud.provider, provider, provider.label());
                        }
                    });
            });
            ui.label(RichText::new(state.settings.cloud.active().model.clone()).weak());
            match &state.key_slot(state.settings.cloud.provider).key {
                Some(_) => {
                    ui.label(RichText::new("API Key 설정됨").weak());
                }
                None => {
                    ui.colored_label(ui.visuals().warn_fg_color, "API Key 없음");
                    if ui.button("설정 열기").clicked() {
                        state.view = crate::app::View::Settings;
                    }
                }
            }
            return;
        }
        let model = state.local_model();
        if let Some(op) = state.model_op(model) {
            let label = match op.kind {
                ModelOpKind::Download => "다운로드 중",
                ModelOpKind::Verify => "검증 중",
            };
            let p = if op.total > 0 { op.done as f32 / op.total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(p).desired_width(140.0).text(format!("{label} {:.0}%", p * 100.0)));
        } else {
            match state.model_file_state(model) {
                ModelFileState::Installed => {
                    ui.label(RichText::new("모델 설치됨").weak());
                }
                ModelFileState::Missing | ModelFileState::Corrupted => {
                    ui.colored_label(ui.visuals().warn_fg_color, "모델 없음");
                    if ui.button("다운로드").clicked() {
                        state.model_requests.push((model, ModelOpKind::Download));
                    }
                }
            }
        }
    });
}

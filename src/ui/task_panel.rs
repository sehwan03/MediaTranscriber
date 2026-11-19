//! 메인 화면 오른쪽: 인식 설정 · Auto 추천 · 변환 시작/취소 · 결과.

use egui::{RichText, vec2};

use super::theme::{self, Tone};
use crate::app::{AppState, ModelOpKind, View};
use crate::config::{CloudProvider, EngineMode, Language, WhisperModel};
use crate::engine_selector::Choice;
use crate::media::format_duration;
use crate::speech::model::ModelFileState;

/// 스크롤되는 부분 (설정 · 추천 · 결과)
pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    super::ffmpeg_install_banner(ui, state);
    settings_card(ui, state);
    ui.add_space(12.0);
    if state.settings.engine_mode == EngineMode::Auto {
        auto_card(ui, state);
        ui.add_space(12.0);
    }
    super::result_panel::show(ui, state);
}

/// 패널 아래에 고정되는 시작/취소 버튼 (스크롤과 관계없이 항상 보임)
pub fn show_footer(ui: &mut egui::Ui, state: &mut AppState) {
    start_controls(ui, state);
}

fn settings_card(ui: &mut egui::Ui, state: &mut AppState) {
    let locked = state.has_active_jobs();
    theme::card(ui, |ui| {
        theme::card_title(ui, "인식 설정", None);

        theme::field_label(ui, "엔진");
        theme::segmented(
            ui,
            &mut state.settings.engine_mode,
            &[(EngineMode::Auto, "Auto"), (EngineMode::Local, "Local"), (EngineMode::Cloud, "Cloud")],
            !locked,
        );
        let hint = match state.settings.engine_mode {
            EngineMode::Auto => "PC 성능을 분석해 자동으로 고릅니다",
            EngineMode::Local => "내 PC에서 처리 · 인터넷 불필요",
            EngineMode::Cloud => "인터넷 서비스로 처리 · 본인 API Key 사용",
        };
        ui.label(RichText::new(hint).size(12.0).color(theme::TEXT_FAINT));
        ui.add_space(6.0);

        let cloud_relevant = state.settings.engine_mode == EngineMode::Cloud
            || (state.settings.engine_mode == EngineMode::Auto && state.uses_cloud());
        if cloud_relevant {
            theme::field_label(ui, "Cloud 공급자");
            theme::segmented(
                ui,
                &mut state.settings.cloud.provider,
                &[(CloudProvider::OpenAi, "OpenAI"), (CloudProvider::Gemini, "Gemini")],
                !locked,
            );
            ui.horizontal(|ui| {
                ui.label(RichText::new(state.settings.cloud.active().model.clone()).size(12.5).color(theme::TEXT_DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match &state.key_slot(state.settings.cloud.provider).key {
                        Some(_) => {
                            theme::pill(ui, "API Key 설정됨", Tone::Success);
                        }
                        None => {
                            if ui.small_button("키 입력").clicked() {
                                state.view = View::Settings;
                            }
                            theme::pill(ui, "API Key 없음", Tone::Warn);
                        }
                    }
                });
            });
            ui.add_space(6.0);
        }

        if state.settings.engine_mode != EngineMode::Cloud {
            let label = if state.settings.engine_mode == EngineMode::Auto { "선호 모델 (Local)" } else { "모델" };
            theme::field_label(ui, label);
            ui.add_enabled_ui(!locked, |ui| {
                egui::ComboBox::from_id_salt("side_model")
                    .width(ui.available_width())
                    .selected_text(model_text(state.settings.whisper_model))
                    .show_ui(ui, |ui| {
                        for model in WhisperModel::ALL {
                            ui.selectable_value(&mut state.settings.whisper_model, model, model_text(model));
                        }
                    });
            });
            model_status(ui, state);
            ui.add_space(6.0);
        }

        theme::field_label(ui, "언어");
        ui.add_enabled_ui(!locked, |ui| {
            egui::ComboBox::from_id_salt("side_lang")
                .width(ui.available_width())
                .selected_text(state.settings.language.label())
                .show_ui(ui, |ui| {
                    for lang in Language::ALL {
                        ui.selectable_value(&mut state.settings.language, lang, lang.label());
                    }
                });
        });
        ui.add_space(6.0);

        theme::field_label(ui, "저장 위치 · 형식 TXT (UTF-8)");
        ui.horizontal(|ui| {
            let path = state.output_dir().display().to_string();
            let width = ui.available_width() - 64.0;
            ui.add_sized(
                vec2(width, 22.0),
                egui::Label::new(RichText::new(&path).size(12.5).color(theme::TEXT_DIM)).truncate(),
            )
            .on_hover_text(&path);
            if ui.small_button("변경").clicked() {
                let dialog = rfd::FileDialog::new().set_title("출력 폴더 선택").set_directory(state.output_dir());
                if let Some(dir) = dialog.pick_folder() {
                    state.settings.output_dir = Some(dir);
                }
            }
        });
    });
}

fn model_text(model: WhisperModel) -> String {
    let note = match model {
        WhisperModel::Tiny => "가장 빠름",
        WhisperModel::Base => "권장",
        WhisperModel::Small => "더 정확",
        WhisperModel::Medium => "가장 정확 · 느림",
    };
    format!("{} — {note}", model.label())
}

/// 선택한 로컬 모델의 설치 상태 한 줄
fn model_status(ui: &mut egui::Ui, state: &mut AppState) {
    let model = state.local_model();
    ui.horizontal(|ui| {
        if let Some(op) = state.model_op(model) {
            let label = match op.kind {
                ModelOpKind::Download => "다운로드",
                ModelOpKind::Verify => "검증",
            };
            let p = if op.total > 0 { op.done as f32 / op.total as f32 } else { 0.0 };
            ui.label(RichText::new(format!("{} {label} {:.0}%", model.label(), p * 100.0)).size(12.0).color(theme::TEXT_DIM));
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), egui::Sense::hover());
            theme::paint_progress(ui.painter(), rect, p, theme::ACCENT);
            return;
        }
        match state.model_file_state(model) {
            ModelFileState::Installed => {
                theme::pill(ui, &format!("{} 모델 설치됨", model.label()), Tone::Success);
            }
            ModelFileState::Missing | ModelFileState::Corrupted => {
                theme::pill(ui, &format!("{} 모델 없음", model.label()), Tone::Warn);
                let size = crate::utils::format_bytes(crate::speech::model::spec(model).size_bytes);
                if ui.small_button(format!("다운로드 ({size})")).clicked() {
                    state.model_requests.push((model, ModelOpKind::Download));
                }
            }
        }
    });
}

fn auto_card(ui: &mut egui::Ui, state: &mut AppState) {
    theme::card(ui, |ui| {
        theme::card_title(ui, "Auto 추천", Some("이 PC의 메모리·속도와 변환할 파일 길이로 판단합니다"));
        let Some(rec) = state.recommendation() else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("PC 성능 확인 중…").color(theme::TEXT_DIM));
            });
            return;
        };
        ui.horizontal(|ui| {
            let (text, tone) = match &rec.choice {
                Choice::Local { model } => (format!("Local · {}", model.label()), Tone::Success),
                Choice::Cloud => (format!("Cloud · {}", state.settings.cloud.provider.label()), Tone::Info),
                Choice::Unavailable => ("사용 불가".to_owned(), Tone::Danger),
            };
            theme::pill(ui, &text, tone);
            if rec.needs_download {
                theme::pill(ui, "모델 다운로드 필요", Tone::Warn);
            }
        });
        if let Some(local) = &rec.local {
            let time = local.total_time.map(|t| format!(" · 예상 {}", format_duration(t))).unwrap_or_default();
            ui.label(RichText::new(format!("로컬 약 {:.1}배속{time}", local.rtf)).size(13.0).color(theme::TEXT));
            ui.label(RichText::new(local.source.label()).size(12.0).color(theme::TEXT_FAINT));
        }
        egui::CollapsingHeader::new(RichText::new("판단 근거").size(12.5).color(theme::TEXT_DIM))
            .id_salt("auto-reasons")
            .default_open(false)
            .show(ui, |ui| {
                for reason in &rec.reasons {
                    ui.label(RichText::new(format!("• {reason}")).size(12.0).color(theme::TEXT_DIM));
                }
            });
        let model = rec.local.as_ref().map_or(state.settings.whisper_model, |l| l.model);
        let installed = state.model_file_state(model) == ModelFileState::Installed;
        let busy = state.benchmark_running.is_some() || state.has_active_jobs();
        if state.benchmark_running.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("성능 측정 중…").color(theme::TEXT_DIM));
            });
        } else if ui
            .add_enabled(installed && !busy, egui::Button::new("성능 측정 (30초 샘플)"))
            .on_hover_text("등록된 파일의 앞 30초(없으면 합성 신호)로 이 PC의 실제 처리 속도를 잽니다.")
            .on_disabled_hover_text("모델 설치 후, 변환 중이 아닐 때 측정할 수 있습니다.")
            .clicked()
        {
            state.benchmark_requested = Some(model);
        }
    });
}

fn start_controls(ui: &mut egui::Ui, state: &mut AppState) {
    let blocker = state.start_blocker();
    let ready = state.files.iter().filter(|f| matches!(f.probe, crate::app::ProbeState::Ready(_)) && !f.job.is_active() && !matches!(f.job, crate::app::JobState::Completed { .. })).count();
    let label = if ready > 0 && blocker.is_none() { format!("변환 시작 · {ready}개") } else { "변환 시작".to_owned() };
    let start = theme::big_button(ui, &label, 46.0, blocker.is_none(), true);
    if let Some(reason) = &blocker {
        ui.label(RichText::new(reason).size(12.0).color(theme::TEXT_FAINT));
    } else if start.clicked() {
        state.start_requested = true;
    }
    if state.has_active_jobs() {
        ui.add_space(4.0);
        if theme::big_button(ui, "전체 취소", 36.0, true, false).clicked() {
            state.cancel_all();
        }
    }
}

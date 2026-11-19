use egui::RichText;

use std::sync::atomic::Ordering;

use super::theme::{self, Tone};
use crate::app::{AppState, FfmpegStatus, KeyCheck, ModelOpKind, StatusLevel};
use crate::config::settings::ProviderSettings;
use crate::config::{CloudProvider, EngineMode, Language, WhisperModel};
use zeroize::Zeroize;
use crate::speech::model::{self, ModelFileState};
use crate::utils::format_bytes;

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // 읽기 편하도록 가운데 정렬된 최대 폭 열
        let width = ui.available_width().min(860.0);
        let side = (ui.available_width() - width) / 2.0;
        ui.horizontal(|ui| {
            ui.add_space(side);
            ui.vertical(|ui| {
                ui.set_width(width);
                sections(ui, state);
            });
        });
    });
}

fn label_col(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(140.0, 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(140.0);
            ui.label(RichText::new(text).color(theme::TEXT_DIM));
        },
    );
}

/// 긴 경로는 정해진 폭에서 … 로 자르고, 마우스를 올리면 전체 경로를 보여준다.
fn path_label(ui: &mut egui::Ui, path: &str, width: f32, color: egui::Color32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_max_width(width);
            ui.add(egui::Label::new(RichText::new(path).monospace().color(color)).truncate())
                .on_hover_text(path);
        },
    );
}

fn sections(ui: &mut egui::Ui, state: &mut AppState) {
    ui.label(RichText::new("설정").font(theme::bold(22.0)).color(theme::TEXT));
    ui.label(RichText::new("변경 사항은 바로 저장됩니다. API Key 는 설정 파일에 저장되지 않습니다.").color(theme::TEXT_DIM));
    ui.add_space(12.0);

    theme::card(ui, |ui| {
        theme::card_title(ui, "기본 설정", None);
        egui::Grid::new("settings_grid").num_columns(2).spacing([16.0, 12.0]).show(ui, |ui| {
            label_col(ui, "기본 엔진");
            let unlocked = !state.has_active_jobs();
            ui.allocate_ui(egui::vec2(300.0, 30.0), |ui| {
                theme::segmented(
                    ui,
                    &mut state.settings.engine_mode,
                    &[(EngineMode::Auto, "Auto"), (EngineMode::Local, "Local"), (EngineMode::Cloud, "Cloud")],
                    unlocked,
                );
            });
            ui.end_row();

            label_col(ui, "Whisper 모델");
            egui::ComboBox::from_id_salt("whisper_model")
                .selected_text(model_text(state.settings.whisper_model))
                .width(300.0)
                .show_ui(ui, |ui| {
                    for model in WhisperModel::ALL {
                        ui.selectable_value(&mut state.settings.whisper_model, model, model_text(model));
                    }
                });
            ui.end_row();

            label_col(ui, "언어");
            egui::ComboBox::from_id_salt("language")
                .selected_text(state.settings.language.label())
                .width(300.0)
                .show_ui(ui, |ui| {
                    for lang in Language::ALL {
                        ui.selectable_value(&mut state.settings.language, lang, lang.label());
                    }
                });
            ui.end_row();

            label_col(ui, "출력 폴더");
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 130.0).max(160.0);
                path_label(ui, &state.output_dir().display().to_string(), w, theme::TEXT);
                if ui.small_button("변경").clicked() {
                    let dialog = rfd::FileDialog::new().set_title("출력 폴더 선택").set_directory(state.output_dir());
                    if let Some(dir) = dialog.pick_folder() {
                        state.settings.output_dir = Some(dir);
                    }
                }
                if state.settings.output_dir.is_some() && ui.small_button("기본값").clicked() {
                    state.settings.output_dir = None;
                }
            });
            ui.end_row();
        });
    });
    ui.add_space(12.0);

    theme::card(ui, |ui| {
        theme::card_title(
            ui,
            "Cloud 음성 인식",
            Some("본인 API Key 사용 · '이 PC에 기억'을 선택한 경우에만 Windows 자격 증명 관리자에 암호화 저장"),
        );
        egui::Grid::new("cloud_grid").num_columns(2).spacing([16.0, 12.0]).show(ui, |ui| {
            label_col(ui, "공급자");
            ui.allocate_ui(egui::vec2(300.0, 30.0), |ui| {
                let before = state.settings.cloud.provider;
                theme::segmented(
                    ui,
                    &mut state.settings.cloud.provider,
                    &[(CloudProvider::OpenAi, "OpenAI"), (CloudProvider::Gemini, "Google Gemini")],
                    true,
                );
                if before != state.settings.cloud.provider {
                    // 다른 공급자로 바꾸면 입력 중이던 키를 지운다.
                    state.api_key_input.zeroize();
                    state.api_key_input.clear();
                }
            });
            ui.end_row();

            let provider = state.settings.cloud.provider;
            if provider == CloudProvider::Gemini {
                ui.label("");
                ui.label(
                    RichText::new(
                        "무료 등급(Unpaid)으로 보낸 음성·결과는 Google 제품 개선에 쓰이고 사람 검토자가 볼 수 있습니다. \
                         민감한 내용은 Local 모드를 사용하세요.",
                    )
                    .size(12.5)
                    .color(theme::WARN),
                );
                ui.end_row();
            }

            label_col(ui, "API Key");
            ui.vertical(|ui| api_key_editor(ui, state, provider));
            ui.end_row();

            let defaults = ProviderSettings::defaults(provider);
            let current = state.settings.cloud.provider_settings_mut(provider);
            label_col(ui, "모델");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut current.model).desired_width(240.0));
                if current.model != defaults.model && ui.small_button("기본값").clicked() {
                    current.model = defaults.model.clone();
                }
            });
            ui.end_row();

            label_col(ui, "API 주소");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut current.base_url).desired_width(360.0));
                if current.base_url != defaults.base_url && ui.small_button("기본값").clicked() {
                    current.base_url = defaults.base_url.clone();
                }
            });
            ui.end_row();

            label_col(ui, "업로드 구간 길이");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut state.settings.cloud.chunk_seconds)
                        .range(60..=1800)
                        .speed(10)
                        .suffix(" 초"),
                );
                ui.label(RichText::new("긴 파일은 이 길이로 나눠 업로드합니다").size(12.5).color(theme::TEXT_FAINT));
            });
            ui.end_row();
        });
    });
    ui.add_space(12.0);

    theme::card(ui, |ui| {
        theme::card_title(
            ui,
            "Whisper 모델",
            Some("다운로드한 모델은 인터넷 없이 사용할 수 있습니다 · 다운로드 후 SHA-256 으로 무결성 검증"),
        );
        model_manager(ui, state);
    });
    ui.add_space(12.0);

    theme::card(ui, |ui| {
        theme::card_title(ui, "FFmpeg", Some("동영상·음성 파일을 읽는 데 필요합니다"));
        crate::ui::ffmpeg_install_banner(ui, state);
        egui::Grid::new("ffmpeg_grid").num_columns(2).spacing([16.0, 12.0]).show(ui, |ui| {
            label_col(ui, "상태");
            match &state.ffmpeg {
                FfmpegStatus::Searching => {
                    theme::pill(ui, "확인 중", Tone::Neutral);
                }
                FfmpegStatus::Found { version } => {
                    theme::pill(ui, &format!("사용 가능 · {version}"), Tone::Success);
                }
                FfmpegStatus::Missing(msg) => {
                    theme::pill(ui, "없음", Tone::Warn).on_hover_text(msg);
                }
            }
            ui.end_row();

            label_col(ui, "FFmpeg 폴더");
            ui.horizontal(|ui| {
                let text = state
                    .settings
                    .ffmpeg_dir
                    .as_ref()
                    .map_or_else(|| "자동 탐색".to_owned(), |d| d.display().to_string());
                let w = (ui.available_width() - 220.0).max(120.0);
                path_label(ui, &text, w, theme::TEXT);
                if ui.small_button("변경").clicked()
                    && let Some(dir) = rfd::FileDialog::new()
                        .set_title("ffmpeg.exe 와 ffprobe.exe 가 있는 폴더 선택")
                        .pick_folder()
                {
                    state.settings.ffmpeg_dir = Some(dir);
                    state.ffmpeg_relocate_requested = true;
                }
                if state.settings.ffmpeg_dir.is_some() && ui.small_button("자동 탐색").clicked() {
                    state.settings.ffmpeg_dir = None;
                    state.ffmpeg_relocate_requested = true;
                }
                if ui.small_button("다시 찾기").clicked() {
                    state.ffmpeg_relocate_requested = true;
                }
            });
            ui.end_row();
        });
    });
    ui.add_space(12.0);

    theme::card(ui, |ui| {
        theme::card_title(ui, "시스템 정보", Some("Auto 모드가 엔진을 고를 때 사용합니다"));
        system_info(ui, state);
    });
    ui.add_space(12.0);

    theme::card(ui, |ui| {
        theme::card_title(ui, "저장 위치", Some("오류가 생기면 로그 폴더의 기록을 확인하세요 (API Key·인식 텍스트는 기록되지 않음)"));
        egui::Grid::new("paths_grid").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            for (name, path) in [
                ("설정 파일", &state.paths.settings_file),
                ("로그 폴더", &state.paths.log_dir),
                ("모델 폴더", &state.paths.models_dir),
                ("FFmpeg 자동 설치", &state.paths.ffmpeg_dir),
            ] {
                label_col(ui, name);
                let w = ui.available_width().max(160.0);
                path_label(ui, &path.display().to_string(), w, theme::TEXT_DIM);
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("로그 폴더 열기").clicked()
                && let Err(err) = crate::ui::result_panel::open_folder(&state.paths.log_dir)
            {
                state.set_status(StatusLevel::Error, format!("로그 폴더를 열 수 없습니다: {err}"));
            }
            if ui.button("모델 폴더 열기").clicked()
                && let Err(err) = crate::ui::result_panel::open_folder(&state.paths.models_dir)
            {
                state.set_status(StatusLevel::Error, format!("모델 폴더를 열 수 없습니다: {err}"));
            }
        });
    });
    ui.add_space(20.0);
}

fn model_text(model: WhisperModel) -> String {
    format!(
        "{}  (파일 {} · 메모리 약 {} MB)",
        model.label(),
        format_bytes(model::spec(model).size_bytes),
        model.approx_memory_mb()
    )
}

fn system_info(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(p) = state.system.clone() else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("하드웨어 정보 수집 중…");
        });
        return;
    };
    egui::Grid::new("system_grid").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
        ui.label("CPU");
        let physical = p.physical_cores.map(|c| format!(" / 물리 {c}코어")).unwrap_or_default();
        ui.label(format!("{} (논리 {}스레드{physical})", p.cpu_brand, p.logical_cores));
        ui.end_row();

        ui.label("RAM");
        ui.label(format!("전체 {} · 사용 가능 {}", format_bytes(p.total_ram), format_bytes(p.available_ram)));
        ui.end_row();

        ui.label("GPU");
        ui.vertical(|ui| {
            if p.gpus.is_empty() {
                ui.label(RichText::new("확인되지 않음").weak());
            }
            for gpu in &p.gpus {
                ui.label(format!("{} (전용 VRAM {})", gpu.name, format_bytes(gpu.dedicated_vram)));
            }
            ui.label(RichText::new("현재 빌드는 CPU 추론만 사용합니다 (GPU 가속은 추후 옵션).").weak());
        });
        ui.end_row();

        ui.label("모델 폴더 드라이브 여유");
        ui.label(p.models_disk_free.map_or_else(|| "알 수 없음".to_owned(), format_bytes));
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("다시 검사").clicked() {
            state.system_refresh_requested = true;
        }
    });

    ui.add_space(10.0);
    ui.label(RichText::new("로컬 처리 속도 (실시간 대비 배속, 높을수록 빠름)").strong());
    let busy = state.benchmark_running.is_some() || state.has_active_jobs();
    egui::Grid::new("bench_grid").num_columns(4).spacing([20.0, 6.0]).striped(true).show(ui, |ui| {
        for title in ["모델", "속도", "근거", ""] {
            ui.label(RichText::new(title).strong());
        }
        ui.end_row();
        for model in WhisperModel::ALL {
            let (rtf, source) = crate::engine_selector::analyzer::local_speed(model, &p, &state.settings.benchmarks);
            ui.label(model.label());
            ui.label(format!("약 {rtf:.1}배속"));
            let note = state
                .settings
                .benchmarks
                .iter()
                .find(|b| b.model == model)
                .map(|b| if b.real_audio { "측정값 (실제 음성)" } else { "측정값 (합성 신호)" })
                .unwrap_or(source.label());
            ui.label(RichText::new(note).weak());
            let installed = state.model_file_state(model) == ModelFileState::Installed;
            if state.benchmark_running == Some(model) {
                ui.spinner();
            } else if ui
                .add_enabled(installed && !busy, egui::Button::new("측정").small())
                .on_disabled_hover_text("모델 설치 후, 변환·측정 중이 아닐 때 측정할 수 있습니다.")
                .clicked()
            {
                state.benchmark_requested = Some(model);
            }
            ui.end_row();
        }
    });
    ui.label(
        RichText::new("측정 전 값은 CPU 정보 기반 추정치입니다. 등록된 파일이 있으면 그 앞 30초로 측정합니다.").weak(),
    );
}

fn api_key_editor(ui: &mut egui::Ui, state: &mut AppState, provider: CloudProvider) {
    let slot = state.key_slot(provider).clone();
    match &slot.key {
        Some(key) => {
            ui.horizontal(|ui| {
                let where_ = if slot.saved { "이 PC에 저장됨" } else { "이번 실행에만 사용" };
                ui.label(RichText::new(key.masked()).monospace());
                ui.label(RichText::new(format!("({where_})")).weak());
                match &slot.check {
                    KeyCheck::Unchecked => {}
                    KeyCheck::Checking => {
                        ui.spinner();
                        ui.label("확인 중…");
                    }
                    KeyCheck::Valid => {
                        ui.colored_label(egui::Color32::from_rgb(110, 200, 120), "사용 가능");
                    }
                    KeyCheck::Invalid(msg) => {
                        ui.colored_label(ui.visuals().error_fg_color, "확인 실패").on_hover_text(msg);
                    }
                }
            });
            if let KeyCheck::Invalid(msg) = &slot.check {
                ui.colored_label(ui.visuals().error_fg_color, msg);
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(slot.check != KeyCheck::Checking, egui::Button::new("다시 확인"))
                    .clicked()
                {
                    state.key_check_requested = Some(provider);
                }
                let label = if slot.saved { "키 삭제 (저장된 키 포함)" } else { "키 지우기" };
                if ui.button(label).clicked() {
                    state.forget_api_key(provider);
                }
            });
        }
        None => {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.api_key_input)
                        .password(true)
                        .hint_text(provider.key_hint())
                        .desired_width(320.0),
                );
                if ui.button("적용 및 확인").clicked() {
                    let remember = state.remember_api_key;
                    state.apply_api_key_input(provider, remember);
                }
            });
            ui.checkbox(&mut state.remember_api_key, "이 PC에 기억 (Windows 자격 증명 관리자)")
                .on_hover_text("선택하지 않으면 앱을 닫을 때 키가 사라지고, 디스크에 아무것도 남지 않습니다.");
        }
    }
}

fn model_manager(ui: &mut egui::Ui, state: &mut AppState) {
    let jobs_running = state.has_active_jobs();
    egui::Grid::new("model_grid").num_columns(4).spacing([20.0, 8.0]).striped(true).show(ui, |ui| {
        for title in ["모델", "크기 / 메모리", "상태", ""] {
            ui.label(RichText::new(title).strong());
        }
        ui.end_row();

        for model in WhisperModel::ALL {
            let selected = state.settings.whisper_model == model;
            let name = if selected { format!("{} (선택됨)", model.label()) } else { model.label().to_owned() };
            ui.label(name);
            ui.label(format!(
                "{} / 약 {} MB",
                format_bytes(model::spec(model).size_bytes),
                model.approx_memory_mb()
            ));

            if let Some(op) = state.model_op(model) {
                let label = match op.kind {
                    ModelOpKind::Download => "다운로드",
                    ModelOpKind::Verify => "검증",
                };
                let p = if op.total > 0 { op.done as f32 / op.total as f32 } else { 0.0 };
                ui.add(egui::ProgressBar::new(p).desired_width(200.0).text(format!(
                    "{label} {} / {}",
                    format_bytes(op.done),
                    format_bytes(op.total)
                )));
                if ui.button("취소").clicked() {
                    op.cancel.store(true, Ordering::Relaxed);
                }
                ui.end_row();
                continue;
            }

            let file_state = state.model_file_state(model);
            match file_state {
                ModelFileState::Installed if state.verified_models.contains(&model) => {
                    theme::pill(ui, "설치됨 · 검증 완료", Tone::Success);
                }
                ModelFileState::Installed => {
                    theme::pill(ui, "설치됨", Tone::Success);
                }
                ModelFileState::Missing => {
                    theme::pill(ui, "미설치", Tone::Neutral);
                }
                ModelFileState::Corrupted => {
                    theme::pill(ui, "손상됨 (크기 불일치)", Tone::Danger);
                }
            }

            ui.horizontal(|ui| match file_state {
                ModelFileState::Installed => {
                    if ui.button("무결성 검증").clicked() {
                        state.model_requests.push((model, ModelOpKind::Verify));
                    }
                    // 변환 중에는 사용 중인 모델을 지우지 않도록 막는다.
                    let can_delete = !jobs_running;
                    if state.delete_confirm == Some(model) {
                        if ui
                            .add_enabled(can_delete, egui::Button::new(RichText::new("정말 삭제").color(ui.visuals().error_fg_color)))
                            .clicked()
                        {
                            state.delete_confirm = None;
                            match model::delete(&state.paths.models_dir, model) {
                                Ok(()) => {
                                    tracing::info!(model = model.label(), "모델 삭제");
                                    state.verified_models.retain(|m| *m != model);
                                    state.set_status(StatusLevel::Info, format!("{} 모델을 삭제했습니다.", model.label()));
                                }
                                Err(err) => state.set_status(StatusLevel::Error, format!("모델 삭제 실패: {err}")),
                            }
                            state.refresh_model_state(model);
                        }
                        if ui.button("아니오").clicked() {
                            state.delete_confirm = None;
                        }
                    } else if ui
                        .add_enabled(can_delete, egui::Button::new("삭제"))
                        .on_disabled_hover_text("변환 작업 중에는 삭제할 수 없습니다.")
                        .clicked()
                    {
                        state.delete_confirm = Some(model);
                    }
                }
                ModelFileState::Missing | ModelFileState::Corrupted => {
                    if ui.button("다운로드").clicked() {
                        state.model_requests.push((model, ModelOpKind::Download));
                    }
                }
            });
            ui.end_row();
        }
    });
}

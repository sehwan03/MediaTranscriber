//! 디자인 시스템: 색상, 글꼴, 공통 위젯(카드·배지·버튼·분할 선택), 직접 그리는 아이콘.
//!
//! 기본 글꼴에는 이모지 아이콘이 없어서 아이콘은 Painter 로 그린다 (해상도와 무관하게 선명).

use std::path::PathBuf;
use std::sync::Arc;

use egui::epaint::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Pos2, Rect, Response,
    RichText, Sense, Shape, Stroke, StrokeKind, TextStyle, pos2, vec2,
};

// ---------------------------------------------------------------- 색상

pub const BG: Color32 = Color32::from_rgb(0x14, 0x16, 0x1B);
pub const HEADER: Color32 = Color32::from_rgb(0x1A, 0x1D, 0x24);
pub const CARD: Color32 = Color32::from_rgb(0x1F, 0x23, 0x2B);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x27, 0x2C, 0x36);
pub const INPUT: Color32 = Color32::from_rgb(0x15, 0x18, 0x1E);
pub const BORDER: Color32 = Color32::from_rgb(0x2E, 0x34, 0x40);
pub const TEXT: Color32 = Color32::from_rgb(0xE8, 0xEB, 0xF1);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9A, 0xA3, 0xB3);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x6B, 0x74, 0x84);
pub const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x52, 0x93, 0xF8);
pub const SUCCESS: Color32 = Color32::from_rgb(0x34, 0xD3, 0x99);
pub const WARN: Color32 = Color32::from_rgb(0xFB, 0xBF, 0x24);
pub const DANGER: Color32 = Color32::from_rgb(0xF8, 0x71, 0x71);

pub const RADIUS: u8 = 12;
pub const RADIUS_SM: u8 = 8;

fn soft(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

// ---------------------------------------------------------------- 스타일

/// 전체 스타일 적용 (앱 시작 시 한 번).
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        use TextStyle::*;
        style.text_styles = [
            (Heading, FontId::new(19.0, bold_family())),
            (Body, FontId::proportional(14.0)),
            (Button, FontId::proportional(14.0)),
            (Small, FontId::proportional(12.0)),
            (Monospace, FontId::monospace(12.5)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 8.0);
        s.button_padding = vec2(12.0, 6.0);
        s.interact_size.y = 30.0;
        s.combo_width = 180.0;
        s.window_margin = Margin::same(16);

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(RADIUS);
        v.menu_corner_radius = CornerRadius::same(RADIUS_SM);
        v.extreme_bg_color = INPUT;
        v.faint_bg_color = soft(Color32::WHITE, 6);
        v.code_bg_color = INPUT;
        v.hyperlink_color = ACCENT_HOVER;
        v.warn_fg_color = WARN;
        v.error_fg_color = DANGER;
        v.override_text_color = None;
        v.selection.bg_fill = soft(ACCENT, 110);
        v.selection.stroke = Stroke::new(1.0, TEXT);

        let r = CornerRadius::same(RADIUS_SM);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = CARD;
        w.noninteractive.weak_bg_fill = CARD;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = Color32::from_rgb(0x2A, 0x30, 0x3B);
        w.inactive.weak_bg_fill = Color32::from_rgb(0x2A, 0x30, 0x3B);
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = r;

        w.hovered.bg_fill = Color32::from_rgb(0x33, 0x3A, 0x47);
        w.hovered.weak_bg_fill = Color32::from_rgb(0x33, 0x3A, 0x47);
        w.hovered.bg_stroke = Stroke::new(1.0, soft(ACCENT, 120));
        w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = Color32::from_rgb(0x3A, 0x42, 0x50);
        w.active.weak_bg_fill = Color32::from_rgb(0x3A, 0x42, 0x50);
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open = w.active;
    });
}

pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, bold_family())
}

/// Windows 맑은 고딕(일반/굵게)을 기본 글꼴로 등록한다. 찾지 못하면 false.
/// "bold" 글꼴 계열은 항상 정의한다 (없는 계열을 쓰면 egui 가 패닉하므로).
pub fn install_fonts(ctx: &egui::Context) -> bool {
    let fonts_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let read = |name: &str| std::fs::read(fonts_dir.join(name)).ok();

    let mut defs = FontDefinitions::default();
    let regular = read("malgun.ttf").or_else(|| read("gulim.ttc"));
    let bold_data = read("malgunbd.ttf");
    let found = regular.is_some();

    if let Some(bytes) = regular {
        defs.font_data.insert("korean".into(), Arc::new(FontData::from_owned(bytes)));
        if let Some(list) = defs.families.get_mut(&FontFamily::Proportional) {
            list.insert(0, "korean".into());
        }
        if let Some(list) = defs.families.get_mut(&FontFamily::Monospace) {
            list.push("korean".into());
        }
    }
    if let Some(bytes) = bold_data {
        defs.font_data.insert("korean-bold".into(), Arc::new(FontData::from_owned(bytes)));
    }
    let mut bold_list: Vec<String> = Vec::new();
    if defs.font_data.contains_key("korean-bold") {
        bold_list.push("korean-bold".into());
    }
    bold_list.extend(defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    defs.families.insert(bold_family(), bold_list);
    ctx.set_fonts(defs);
    found
}

// ---------------------------------------------------------------- 레이아웃 위젯

/// 둥근 카드.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// 카드 제목 (굵은 글꼴 + 선택적 보조 문구)
pub fn card_title(ui: &mut egui::Ui, title: &str, subtitle: Option<&str>) {
    ui.label(RichText::new(title).font(bold(15.5)).color(TEXT));
    if let Some(sub) = subtitle {
        ui.add_space(-4.0);
        ui.label(RichText::new(sub).size(12.0).color(TEXT_DIM));
    }
    ui.add_space(4.0);
}

/// 작은 회색 필드 이름
pub fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Warn,
    Danger,
}

impl Tone {
    pub fn color(self) -> Color32 {
        match self {
            Self::Neutral => TEXT_DIM,
            Self::Info => ACCENT_HOVER,
            Self::Success => SUCCESS,
            Self::Warn => WARN,
            Self::Danger => DANGER,
        }
    }
}

/// 상태 배지 (둥근 알약 모양)
pub fn pill(ui: &mut egui::Ui, text: &str, tone: Tone) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, tone.color());
    let size = galley.size() + vec2(20.0, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    paint_pill(ui.painter(), rect, galley, tone);
    response
}

fn paint_pill(painter: &egui::Painter, rect: Rect, galley: Arc<egui::Galley>, tone: Tone) {
    let fill = match tone {
        Tone::Neutral => soft(Color32::WHITE, 16),
        _ => soft(tone.color(), 34),
    };
    painter.rect_filled(rect, CornerRadius::same(255), fill);
    let pos = rect.center() - galley.size() / 2.0;
    painter.galley(pos, galley, tone.color());
}

/// 주어진 사각형 오른쪽 끝에 배지를 그린다 (행 안에서 위치 지정용). 그린 영역을 돌려준다.
pub fn paint_pill_right(painter: &egui::Painter, right_center: Pos2, text: &str, tone: Tone) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(12.0), tone.color());
    let size = galley.size() + vec2(20.0, 8.0);
    let rect = Rect::from_min_size(pos2(right_center.x - size.x, right_center.y - size.y / 2.0), size);
    paint_pill(painter, rect, galley, tone);
    rect
}

/// 강조(파란색) 기본 버튼
pub fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).font(bold(15.0)).color(Color32::WHITE))
        .fill(ACCENT)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::NONE)
}

/// 폭 전체를 쓰는 큰 버튼 (글자 가운데 정렬). primary=true 면 파란색, 아니면 위험(빨강) 톤.
pub fn big_button(ui: &mut egui::Ui, text: &str, height: f32, enabled: bool, primary: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), if enabled { Sense::click() } else { Sense::hover() });
    let (fill, fg, stroke) = match (primary, enabled, resp.hovered()) {
        (true, false, _) => (soft(ACCENT, 70), soft(Color32::WHITE, 120), Stroke::NONE),
        (true, true, true) => (ACCENT_HOVER, Color32::WHITE, Stroke::NONE),
        (true, true, false) => (ACCENT, Color32::WHITE, Stroke::NONE),
        (false, _, true) => (soft(DANGER, 40), DANGER, Stroke::new(1.0, soft(DANGER, 140))),
        (false, _, false) => (soft(DANGER, 18), DANGER, Stroke::new(1.0, soft(DANGER, 90))),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(10), fill);
    if stroke != Stroke::NONE {
        painter.rect_stroke(rect, CornerRadius::same(10), stroke, StrokeKind::Inside);
    }
    let font = if primary { bold(15.5) } else { FontId::proportional(14.0) };
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, fg);
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 분할 선택 (탭처럼 생긴 단일 선택). 바뀌면 true.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)], enabled: bool) -> bool {
    let mut changed = false;
    let height = 30.0;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(RADIUS_SM), INPUT);
    painter.rect_stroke(rect, CornerRadius::same(RADIUS_SM), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let seg_w = (width - 4.0) / options.len() as f32;
    for (i, (opt, label)) in options.iter().enumerate() {
        let seg = Rect::from_min_size(pos2(rect.left() + 2.0 + seg_w * i as f32, rect.top() + 2.0), vec2(seg_w, height - 4.0));
        let id = ui.scope_id().with(("segmented", label, rect.min.y as i32));
        let resp = ui.interact(seg, id, if enabled { Sense::click() } else { Sense::hover() });
        let selected = *value == *opt;
        if selected {
            painter.rect_filled(seg, CornerRadius::same(RADIUS_SM - 2), ACCENT);
        } else if resp.hovered() && enabled {
            painter.rect_filled(seg, CornerRadius::same(RADIUS_SM - 2), CARD_HOVER);
        }
        let color = if selected {
            Color32::WHITE
        } else if enabled {
            TEXT_DIM
        } else {
            TEXT_FAINT
        };
        let font = if selected { bold(13.0) } else { FontId::proportional(13.0) };
        painter.text(seg.center(), Align2::CENTER_CENTER, *label, font, color);
        if resp.clicked() && !selected {
            *value = *opt;
            changed = true;
        }
    }
    changed
}

/// 한 줄로 자르고 끝에 … 를 붙인 글자 배치
pub fn truncated(painter: &egui::Painter, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: max_width.max(10.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

/// 얇은 진행 막대
pub fn paint_progress(painter: &egui::Painter, rect: Rect, fraction: f32, color: Color32) {
    painter.rect_filled(rect, CornerRadius::same(255), soft(Color32::WHITE, 18));
    let w = rect.width() * fraction.clamp(0.0, 1.0);
    if w > 0.5 {
        let fill = Rect::from_min_size(rect.min, vec2(w.max(rect.height()), rect.height()));
        painter.rect_filled(fill, CornerRadius::same(255), color);
    }
}

// ---------------------------------------------------------------- 아이콘

#[derive(Clone, Copy)]
pub enum Icon {
    Wave,
    Upload,
    Video,
    Audio,
    Close,
    Trash,
    Folder,
    FileText,
}

/// 아이콘을 `rect` 안에 그린다.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let w = (s / 12.0).max(1.4);
    let stroke = Stroke::new(w, color);
    let p = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    match icon {
        Icon::Wave | Icon::Audio => {
            let bars: &[f32] = if matches!(icon, Icon::Wave) {
                &[0.22, 0.42, 0.7, 0.42, 0.22]
            } else {
                &[0.3, 0.6, 0.4, 0.7, 0.3]
            };
            let step = s * 0.17;
            let x0 = c.x - step * (bars.len() as f32 - 1.0) / 2.0;
            for (i, h) in bars.iter().enumerate() {
                let x = x0 + step * i as f32;
                let half = h * s / 2.0;
                painter.line_segment([pos2(x, c.y - half), pos2(x, c.y + half)], Stroke::new(w * 1.25, color));
            }
        }
        Icon::Upload => {
            // 트레이 + 위쪽 화살표
            painter.add(Shape::line(vec![p(-0.38, 0.08), p(-0.38, 0.36), p(0.38, 0.36), p(0.38, 0.08)], stroke));
            painter.line_segment([p(0.0, 0.18), p(0.0, -0.38)], stroke);
            painter.add(Shape::line(vec![p(-0.2, -0.18), p(0.0, -0.38), p(0.2, -0.18)], stroke));
        }
        Icon::Video => {
            let r = Rect::from_min_max(p(-0.38, -0.3), p(0.38, 0.3));
            painter.rect_stroke(r, CornerRadius::same((s * 0.12) as u8), stroke, StrokeKind::Middle);
            painter.add(Shape::convex_polygon(vec![p(-0.1, -0.15), p(0.18, 0.0), p(-0.1, 0.15)], color, Stroke::NONE));
        }
        Icon::Close => {
            painter.line_segment([p(-0.25, -0.25), p(0.25, 0.25)], stroke);
            painter.line_segment([p(0.25, -0.25), p(-0.25, 0.25)], stroke);
        }
        Icon::Trash => {
            painter.line_segment([p(-0.3, -0.22), p(0.3, -0.22)], stroke);
            painter.line_segment([p(-0.1, -0.32), p(0.1, -0.32)], stroke);
            let body = vec![p(-0.22, -0.18), p(-0.17, 0.34), p(0.17, 0.34), p(0.22, -0.18)];
            painter.add(Shape::line(body, stroke));
        }
        Icon::Folder => {
            let pts = vec![p(-0.38, -0.26), p(-0.1, -0.26), p(0.0, -0.16), p(0.38, -0.16), p(0.38, 0.3), p(-0.38, 0.3), p(-0.38, -0.26)];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::FileText => {
            let pts = vec![p(-0.26, -0.36), p(0.1, -0.36), p(0.28, -0.18), p(0.28, 0.36), p(-0.26, 0.36), p(-0.26, -0.36)];
            painter.add(Shape::line(pts, stroke));
            for y in [-0.05, 0.08, 0.21] {
                painter.line_segment([p(-0.14, y), p(0.16, y)], Stroke::new(w * 0.8, color));
            }
        }
    }
}

/// 아이콘 하나만 있는 작은 버튼 (마우스를 올리면 배경과 설명 표시)
pub fn icon_button(ui: &mut egui::Ui, rect: Rect, id: egui::Id, icon: Icon, tooltip: &str, color: Color32) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(RADIUS_SM), soft(Color32::WHITE, 18));
    }
    paint_icon(painter, rect.shrink(rect.width() * 0.22), icon, if resp.hovered() { TEXT } else { color });
    resp.on_hover_text(tooltip).clicked()
}

/// 레이아웃 흐름 안에 아이콘 버튼을 배치
pub fn icon_button_inline(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
    let id = ui.scope_id().with(("icon-btn", tooltip, rect.min.x as i32, rect.min.y as i32));
    icon_button(ui, rect, id, icon, tooltip, TEXT_DIM)
}

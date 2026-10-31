//! Visual design: palette, typography and egui style.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};
use ferrocord_core::Theme;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Server rail.
    pub bg_rail: Color32,
    /// Channel list / member list.
    pub bg_side: Color32,
    /// Chat area.
    pub bg_main: Color32,
    /// Inputs, embeds, code blocks.
    pub bg_elevated: Color32,
    pub bg_hover: Color32,
    pub bg_selected: Color32,
    pub bg_mention: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub link: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub online: Color32,
    pub idle: Color32,
    pub dnd: Color32,
    pub offline: Color32,
}

impl Palette {
    pub fn for_theme(theme: Theme, accent: Color32) -> Self {
        match theme {
            Theme::Dark => Self {
                bg_rail: hex(0x16171b),
                bg_side: hex(0x1c1d22),
                bg_main: hex(0x222329),
                bg_elevated: hex(0x2b2c33),
                bg_hover: hex(0x2a2b31),
                bg_selected: hex(0x33343c),
                bg_mention: Color32::from_rgba_unmultiplied(240, 178, 50, 22),
                border: hex(0x2f3038),
                text: hex(0xe6e7eb),
                text_muted: hex(0xa3a6b0),
                text_faint: hex(0x737682),
                accent,
                link: hex(0x5aa8ff),
                danger: hex(0xf04f5c),
                success: hex(0x3ba55d),
                warning: hex(0xf0b232),
                online: hex(0x23a55a),
                idle: hex(0xf0b232),
                dnd: hex(0xf23f43),
                offline: hex(0x80848e),
            },
            Theme::Midnight => Self {
                bg_rail: hex(0x000000),
                bg_side: hex(0x09090b),
                bg_main: hex(0x0f0f12),
                bg_elevated: hex(0x18181d),
                bg_hover: hex(0x16161a),
                bg_selected: hex(0x202027),
                bg_mention: Color32::from_rgba_unmultiplied(240, 178, 50, 20),
                border: hex(0x1d1d22),
                text: hex(0xdcdde2),
                text_muted: hex(0x9a9ca6),
                text_faint: hex(0x676a75),
                accent,
                link: hex(0x5aa8ff),
                danger: hex(0xf04f5c),
                success: hex(0x3ba55d),
                warning: hex(0xf0b232),
                online: hex(0x23a55a),
                idle: hex(0xf0b232),
                dnd: hex(0xf23f43),
                offline: hex(0x6d717b),
            },
            Theme::Light => Self {
                bg_rail: hex(0xe3e5e8),
                bg_side: hex(0xf0f1f3),
                bg_main: hex(0xffffff),
                bg_elevated: hex(0xebedef),
                bg_hover: hex(0xe8e9ec),
                bg_selected: hex(0xdcdee2),
                bg_mention: Color32::from_rgba_unmultiplied(240, 178, 50, 40),
                border: hex(0xdddfe3),
                text: hex(0x1f2024),
                text_muted: hex(0x4e5058),
                text_faint: hex(0x80838c),
                accent,
                link: hex(0x0068e0),
                danger: hex(0xd83c3e),
                success: hex(0x248046),
                warning: hex(0xc27c0e),
                online: hex(0x23a55a),
                idle: hex(0xf0b232),
                dnd: hex(0xf23f43),
                offline: hex(0x80848e),
            },
        }
    }

    pub fn status(&self, s: ferrocord_core::model::Status) -> Color32 {
        use ferrocord_core::model::Status::*;
        match s {
            Online => self.online,
            Idle => self.idle,
            Dnd => self.dnd,
            Invisible | Offline => self.offline,
        }
    }
}

pub const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim_start_matches('#');
    u32::from_str_radix(s, 16).ok().filter(|_| s.len() == 6).map(hex)
}

/// Role colors come as 0xRRGGBB integers; 0 means "no color".
pub fn role_color(c: u32) -> Option<Color32> {
    (c != 0).then(|| hex(c))
}

pub const BOLD: &str = "bold";
pub const ITALIC: &str = "italic";

pub fn bold() -> FontFamily {
    FontFamily::Name(BOLD.into())
}

pub fn italic() -> FontFamily {
    FontFamily::Name(ITALIC.into())
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    add(&mut fonts, "inter-semibold", include_bytes!("../assets/fonts/Inter-SemiBold.ttf"));
    add(&mut fonts, "inter-italic", include_bytes!("../assets/fonts/Inter-Italic.ttf"));
    add(&mut fonts, "jbmono", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"));
    add(&mut fonts, "noto-emoji", include_bytes!("../assets/fonts/NotoEmoji.ttf"));

    // Fallback chain: full monochrome emoji coverage first, then egui's bundled symbol fonts.
    let mut fallbacks = vec!["noto-emoji".to_string()];
    fallbacks.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    let chain = |primary: &str| {
        let mut v = vec![primary.to_string()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, chain("inter"));
    fonts.families.insert(bold(), chain("inter-semibold"));
    fonts.families.insert(italic(), chain("inter-italic"));
    let mut mono = vec!["jbmono".to_string(), "noto-emoji".to_string()];
    mono.extend(fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Monospace, mono);
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context, p: &Palette, theme: Theme) {
    let mut v = if theme == Theme::Light { Visuals::light() } else { Visuals::dark() };
    v.panel_fill = p.bg_main;
    v.window_fill = p.bg_elevated;
    v.extreme_bg_color = p.bg_elevated;
    v.faint_bg_color = p.bg_hover;
    v.code_bg_color = p.bg_elevated;
    v.hyperlink_color = p.link;
    v.override_text_color = Some(p.text);
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.popup_shadow.blur = 18;
    v.window_shadow.blur = 28;
    v.widgets.noninteractive.bg_fill = p.bg_elevated;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.bg_fill = p.bg_elevated;
    v.widgets.inactive.weak_bg_fill = p.bg_elevated;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.bg_fill = p.bg_selected;
    v.widgets.hovered.weak_bg_fill = p.bg_selected;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.active.bg_fill = p.accent;
    v.widgets.active.weak_bg_fill = p.accent;
    v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(6);
    }
    ctx.set_visuals(v);

    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(20.0, bold())),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.window_margin = Margin::same(16);
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating = true;
        s.interaction.selectable_labels = true;
        s.url_in_tooltip = true;
    });
}

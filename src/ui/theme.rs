//! Colours, fonts and icon glyphs.

use crate::settings::ThemeChoice;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Theme, ThemePreference, Visuals,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The brand colours: sun yellow over deep teal.
pub const SUN: Color32 = Color32::from_rgb(0xf7, 0xc8, 0x48);
pub const TEAL: Color32 = Color32::from_rgb(0x2e, 0xc4, 0xb6);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x5f, 0x62);
pub const YELLOW: Color32 = Color32::from_rgb(0xf0, 0xc5, 0x41);
pub const GREEN: Color32 = Color32::from_rgb(0x3e, 0xcf, 0x8e);

#[derive(Clone, Copy)]
pub struct Palette {
    #[allow(dead_code)]
    pub dark: bool,
    pub bg: Color32,
    pub rail: Color32,
    pub card: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    /// Sun yellow, tuned for contrast on this background.
    pub accent: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub info: Color32,
    /// Healthy, charging, done.
    pub good: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            dark,
            bg: Color32::from_rgb(0x11, 0x12, 0x1a),
            rail: Color32::from_rgb(0x0c, 0x0d, 0x14),
            card: Color32::from_rgb(0x18, 0x1a, 0x25),
            hover: Color32::from_rgb(0x1f, 0x22, 0x30),
            selected: Color32::from_rgb(0x2c, 0x2a, 0x1c),
            border: Color32::from_rgb(0x2a, 0x2e, 0x3f),
            text: Color32::from_rgb(0xec, 0xee, 0xf5),
            muted: Color32::from_rgb(0xa0, 0xa6, 0xb8),
            faint: Color32::from_rgb(0x68, 0x70, 0x86),
            accent: SUN,
            on_accent: Color32::from_rgb(0x2a, 0x1f, 0x00),
            danger: RED,
            warn: YELLOW,
            info: TEAL,
            good: GREEN,
        }
    } else {
        Palette {
            dark,
            bg: Color32::from_rgb(0xf4, 0xf5, 0xf9),
            rail: Color32::from_rgb(0xff, 0xff, 0xff),
            card: Color32::from_rgb(0xff, 0xff, 0xff),
            hover: Color32::from_rgb(0xec, 0xee, 0xf4),
            selected: Color32::from_rgb(0xff, 0xf2, 0xc8),
            border: Color32::from_rgb(0xda, 0xdd, 0xe7),
            text: Color32::from_rgb(0x17, 0x1a, 0x26),
            muted: Color32::from_rgb(0x57, 0x5e, 0x72),
            faint: Color32::from_rgb(0x8b, 0x92, 0xa5),
            accent: Color32::from_rgb(0x8a, 0x5f, 0x00),
            on_accent: Color32::WHITE,
            danger: Color32::from_rgb(0xd9, 0x3a, 0x3f),
            warn: Color32::from_rgb(0xa6, 0x7a, 0x00),
            info: Color32::from_rgb(0x0b, 0x7f, 0x86),
            good: Color32::from_rgb(0x1c, 0x9a, 0x63),
        }
    }
}

pub fn pal(ui: &egui::Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

pub fn pal_ctx(ctx: &egui::Context) -> Palette {
    palette(ctx.style().visuals.dark_mode)
}

fn visuals(dark: bool) -> Visuals {
    let p = palette(dark);
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = if dark {
        Color32::from_rgb(0x0b, 0x0c, 0x12)
    } else {
        Color32::WHITE
    };
    v.faint_bg_color = p.hover;
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = if dark {
        Color32::from_rgb(0x5c, 0x4a, 0x14)
    } else {
        Color32::from_rgb(0xff, 0xe8, 0xa8)
    };
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    let r = CornerRadius::same(8);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.muted);
    v.widgets.inactive.bg_fill = p.hover;
    v.widgets.inactive.weak_bg_fill = p.hover;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.hovered.bg_fill = p.border;
    v.widgets.hovered.weak_bg_fill = p.border;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.faint);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.active.bg_fill = p.border;
    v.widgets.active.weak_bg_fill = p.border;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.open.bg_fill = p.hover;
    v.widgets.open.weak_bg_fill = p.hover;
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, p.faint);
    v
}

static ICON_FONT: AtomicBool = AtomicBool::new(false);

fn load_fonts(ctx: &egui::Context) {
    let dir = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str| -> bool {
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                fonts
                    .font_data
                    .insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    // System fonts are loaded at run time so the program stays small.
    let regular = add("ui", "segoeui.ttf");
    let semibold = add("ui-semibold", "seguisb.ttf");
    let icons = add("ui-icons", "segmdl2.ttf") || add("ui-icons", "SegoeIcons.ttf");
    ICON_FONT.store(icons, Ordering::Relaxed);

    let defaults = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut body = Vec::new();
    if regular {
        body.push("ui".to_string());
    }
    if icons {
        body.push("ui-icons".to_string());
    }
    body.extend(defaults.iter().cloned());
    let mut strong = Vec::new();
    if semibold {
        strong.push("ui-semibold".to_string());
    }
    strong.extend(body.iter().cloned());
    fonts.families.insert(FontFamily::Proportional, body);
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), strong);
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// One-time setup: fonts, spacing and both colour schemes.
pub fn install(ctx: &egui::Context) {
    load_fonts(ctx);
    ctx.set_visuals_of(Theme::Dark, visuals(true));
    ctx.set_visuals_of(Theme::Light, visuals(false));
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, semibold(22.0)),
            (TextStyle::Body, regular(14.0)),
            (TextStyle::Button, regular(14.0)),
            (TextStyle::Small, regular(12.0)),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.scroll.bar_width = 10.0;
        style.spacing.scroll.floating = true;
        style.interaction.selectable_labels = false;
        style.visuals.indent_has_left_vline = false;
    });
}

pub fn apply_choice(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(match choice {
        ThemeChoice::Dark => ThemePreference::Dark,
        ThemeChoice::Light => ThemePreference::Light,
        ThemeChoice::System => ThemePreference::System,
    });
}

/// Icon glyphs from the system icon font (Segoe MDL2 Assets). If the font is
/// missing the icons are simply left out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Icon(pub &'static str);

#[allow(dead_code)] // a glyph catalogue
impl Icon {
    pub const SEARCH: Icon = Icon("\u{E721}");
    pub const SETTINGS: Icon = Icon("\u{E713}");
    pub const DELETE: Icon = Icon("\u{E74D}");
    pub const REFRESH: Icon = Icon("\u{E72C}");
    pub const APPS: Icon = Icon("\u{E71D}");
    pub const SHOP: Icon = Icon("\u{E719}");
    pub const FOLDER: Icon = Icon("\u{E8B7}");
    pub const FOLDER_OPEN: Icon = Icon("\u{E838}");
    pub const HISTORY: Icon = Icon("\u{E81C}");
    pub const WARNING: Icon = Icon("\u{E7BA}");
    pub const INFO: Icon = Icon("\u{E946}");
    pub const SHIELD: Icon = Icon("\u{EA18}");
    pub const CHECK: Icon = Icon("\u{E73E}");
    pub const CLOSE: Icon = Icon("\u{E711}");
    pub const POWER: Icon = Icon("\u{E7E8}");
    pub const REPAIR: Icon = Icon("\u{E90F}");
    pub const EXPORT: Icon = Icon("\u{EDE1}");
    pub const CHEVRON_RIGHT: Icon = Icon("\u{E76C}");
    pub const CHEVRON_DOWN: Icon = Icon("\u{E70D}");
    pub const BACK: Icon = Icon("\u{E72B}");
    pub const DOCUMENT: Icon = Icon("\u{E8A5}");
    pub const LINK: Icon = Icon("\u{E71B}");
    pub const GLOBE: Icon = Icon("\u{E774}");
    pub const SORT_UP: Icon = Icon("\u{E70E}");
    pub const SORT_DOWN: Icon = Icon("\u{E70D}");
    pub const REGISTRY: Icon = Icon("\u{E8F1}");
    pub const SERVICE: Icon = Icon("\u{E912}");
    pub const CLOCK: Icon = Icon("\u{E823}");
    pub const BROOM: Icon = Icon("\u{EA99}");
    pub const DONE: Icon = Icon("\u{E930}");
    pub const ERROR: Icon = Icon("\u{EA39}");
    pub const FORCE: Icon = Icon("\u{E945}");
    pub const UNDO: Icon = Icon("\u{E7A7}");
    pub const PLAY: Icon = Icon("\u{E768}");
    pub const PAUSE: Icon = Icon("\u{E769}");
    pub const STOP: Icon = Icon("\u{E71A}");
    pub const CALENDAR: Icon = Icon("\u{E787}");
    pub const CHART: Icon = Icon("\u{E9D2}");
    pub const TAG: Icon = Icon("\u{E8EC}");
    pub const LOCK: Icon = Icon("\u{E72E}");
    pub const FLAG: Icon = Icon("\u{E7C1}");
    pub const ADD: Icon = Icon("\u{E710}");
    pub const EDIT: Icon = Icon("\u{E70F}");
    pub const STOPWATCH: Icon = Icon("\u{E916}");
    pub const HIDE: Icon = Icon("\u{ED1A}");
    pub const SAVE: Icon = Icon("\u{E74E}");
    pub const HOME: Icon = Icon("\u{E80F}");
    pub const LIST: Icon = Icon("\u{EA37}");
    pub const PEOPLE: Icon = Icon("\u{E716}");
    pub const MORE: Icon = Icon("\u{E712}");
    pub const FORWARD: Icon = Icon("\u{E72A}");
    pub const COFFEE: Icon = Icon("\u{EC32}");
    pub const FOCUS: Icon = Icon("\u{E7B3}");
    pub const CUT: Icon = Icon("\u{E8C6}");
    pub const MERGE: Icon = Icon("\u{EA3C}");
    pub const UP: Icon = Icon("\u{E74A}");
    pub const DOWN: Icon = Icon("\u{E74B}");
    pub const BRIEFCASE: Icon = Icon("\u{E821}");
    pub const PHONE: Icon = Icon("\u{E717}");
    pub const BATTERY: Icon = Icon("\u{E83F}");
    pub const PLUG: Icon = Icon("\u{E83E}");
    pub const HEART: Icon = Icon("\u{EB51}");
    pub const SPEED: Icon = Icon("\u{EC4A}");
    pub const SUNNY: Icon = Icon("\u{E706}");
    pub const MOON: Icon = Icon("\u{E708}");
    pub const LIGHTNING: Icon = Icon("\u{E945}");
    pub const ALARM: Icon = Icon("\u{EA8F}");
    pub const DEVICE: Icon = Icon("\u{E772}");
    pub const NETWORK: Icon = Icon("\u{E968}");
    pub const KEYBOARD: Icon = Icon("\u{E765}");
    pub const MOUSE: Icon = Icon("\u{E962}");
    pub const USB: Icon = Icon("\u{E88E}");
    pub const BLUETOOTH: Icon = Icon("\u{E702}");
    pub const PROCESSOR: Icon = Icon("\u{E950}");
    pub const LEAF: Icon = Icon("\u{EC0A}");
    pub const SLIDERS: Icon = Icon("\u{E9E9}");

    /// The glyph, or an empty string when the icon font is unavailable.
    pub fn glyph(self) -> &'static str {
        if ICON_FONT.load(Ordering::Relaxed) {
            self.0
        } else {
            ""
        }
    }

    /// `"<glyph>  text"` for buttons.
    pub fn with(self, text: &str) -> String {
        if ICON_FONT.load(Ordering::Relaxed) {
            format!("{}  {text}", self.0)
        } else {
            text.to_string()
        }
    }
}

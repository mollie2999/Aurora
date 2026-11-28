//! Theme tokens, presets and typography.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightPreset {
    #[default]
    Heavy,
    Balanced,
}

/// Control geometry and interaction treatment, independent of the color palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetStyle {
    #[default]
    Aurora,
    Adwaita,
}

/// Optional custom font bytes provided by a theme package.
#[derive(Debug, Clone, Default)]
pub struct CustomFonts {
    pub regular: Option<Vec<u8>>,
    pub bold: Option<Vec<u8>>,
    pub mono: Option<Vec<u8>>,
}

fn default_font_scale() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    /// Window background.
    pub bg: Color32,
    /// Top bar, status bar, side panels.
    pub chrome: Color32,
    /// Cards.
    pub surface: Color32,
    pub surface_hover: Color32,
    /// Inputs, menus, raised elements.
    pub raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    /// Second stop of the accent gradient.
    pub accent_2: Color32,
    /// Text on accent fills.
    pub on_accent: Color32,
    pub play: Color32,
    pub online: Color32,
    pub in_game: Color32,
    pub away: Color32,
    pub danger: Color32,
    pub discount: Color32,
    pub radius: u8,
    #[serde(default)]
    pub weight_preset: WeightPreset,
    #[serde(default)]
    pub widget_style: WidgetStyle,
    /// Live native style data. Saved themes never retain machine-specific data.
    #[serde(skip)]
    pub native_metrics: Option<crate::native_style::NativeControlMetrics>,
    #[serde(default = "default_font_scale")]
    pub font_scale: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self::nevko()
    }
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Theme {
    pub fn presets() -> Vec<Theme> {
        vec![Self::nevko(), Self::midnight(), Self::aurora(), Self::classic(), Self::oled(), Self::daylight()]
    }

    pub fn preset_by_name(name: &str) -> Option<Theme> {
        Self::presets().into_iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// After the NEVKO-UI Steam skin (github.com/dotFelixan/NEVKO-UI): one
    /// deep slate for the chrome, flat grey-blue controls with no borders,
    /// and Steam blue used only for what is active.
    pub fn nevko() -> Self {
        Self {
            name: "NEVKO".into(),
            dark: true,
            bg: hex(0x1A2129),
            chrome: hex(0x161D25),
            surface: hex(0x20272F),
            surface_hover: hex(0x2E3540),
            raised: hex(0x272D36),
            border: hex(0x232A33),
            text: hex(0xDFE3E6),
            text_dim: hex(0x9198A1),
            text_faint: hex(0x6B727C),
            accent: hex(0x1A9FFF),
            accent_2: hex(0x1A9FFF),
            on_accent: hex(0xFFFFFF),
            play: hex(0x4CAF50),
            online: hex(0x57B8F7),
            in_game: hex(0x4CD18A),
            away: hex(0xE8B04B),
            danger: hex(0xE64A35),
            discount: hex(0x4C6B22),
            radius: 9,
            weight_preset: WeightPreset::Heavy,
            widget_style: WidgetStyle::Aurora,
            native_metrics: None,
            font_scale: 1.0,
        }
    }

    /// Default: inky blue-black, electric blue to violet accent.
    pub fn midnight() -> Self {
        Self {
            name: "Midnight".into(),
            dark: true,
            bg: hex(0x0A0D12),
            chrome: hex(0x0E1218),
            surface: hex(0x131922),
            surface_hover: hex(0x1A222D),
            raised: hex(0x1C2530),
            border: hex(0x202935),
            text: hex(0xEEF2F7),
            text_dim: hex(0x9AA5B4),
            text_faint: hex(0x5F6B7A),
            accent: hex(0x4F8CFF),
            accent_2: hex(0x8B5CF6),
            on_accent: hex(0xFFFFFF),
            play: hex(0x2FB36B),
            online: hex(0x4FC3F7),
            in_game: hex(0x9CCC4B),
            away: hex(0xF2B84B),
            danger: hex(0xF0544F),
            discount: hex(0x4C6B22),
            radius: 12,
            weight_preset: WeightPreset::Heavy,
            widget_style: WidgetStyle::Aurora,
            native_metrics: None,
            font_scale: 1.0,
        }
    }

    /// Deep teal with a mint-to-cyan accent.
    pub fn aurora() -> Self {
        Self {
            name: "Aurora".into(),
            bg: hex(0x081112),
            chrome: hex(0x0B1718),
            surface: hex(0x0F1E20),
            surface_hover: hex(0x15292B),
            raised: hex(0x172D30),
            border: hex(0x1B3336),
            text_dim: hex(0x93AFAE),
            text_faint: hex(0x58716F),
            accent: hex(0x2DD4BF),
            accent_2: hex(0x38BDF8),
            on_accent: hex(0x04201C),
            ..Self::midnight()
        }
    }

    /// Familiar Steam-like blues.
    pub fn classic() -> Self {
        Self {
            name: "Classic".into(),
            bg: hex(0x1B2838),
            chrome: hex(0x171A21),
            surface: hex(0x213045),
            surface_hover: hex(0x2A3F5A),
            raised: hex(0x2A3B4F),
            border: hex(0x2E4058),
            text: hex(0xDCDEDF),
            text_dim: hex(0x8F98A0),
            text_faint: hex(0x5F6A75),
            accent: hex(0x66C0F4),
            accent_2: hex(0x417A9B),
            on_accent: hex(0x0E1C2A),
            play: hex(0x5C7E10),
            radius: 4,
            ..Self::midnight()
        }
    }

    /// True black for OLED screens.
    pub fn oled() -> Self {
        Self {
            name: "OLED".into(),
            bg: hex(0x000000),
            chrome: hex(0x050505),
            surface: hex(0x0E0E0F),
            surface_hover: hex(0x18181A),
            raised: hex(0x161618),
            border: hex(0x1F1F22),
            accent: hex(0xF5F5F5),
            accent_2: hex(0xA3A3A3),
            on_accent: hex(0x000000),
            ..Self::midnight()
        }
    }

    pub fn daylight() -> Self {
        Self {
            name: "Daylight".into(),
            dark: false,
            bg: hex(0xF3F5F8),
            chrome: hex(0xFFFFFF),
            surface: hex(0xFFFFFF),
            surface_hover: hex(0xEEF2F7),
            raised: hex(0xF7F9FB),
            border: hex(0xDEE3EA),
            text: hex(0x121A24),
            text_dim: hex(0x546273),
            text_faint: hex(0x8A97A6),
            accent: hex(0x2563EB),
            accent_2: hex(0x7C3AED),
            on_accent: hex(0xFFFFFF),
            play: hex(0x16A34A),
            online: hex(0x0284C7),
            in_game: hex(0x4D7C0F),
            away: hex(0xB45309),
            danger: hex(0xDC2626),
            discount: hex(0x4D7C0F),
            radius: 12,
            weight_preset: WeightPreset::Heavy,
            widget_style: WidgetStyle::Aurora,
            native_metrics: None,
            font_scale: 1.0,
        }
    }

    /// Adwaita surfaces and semantic colors from libadwaita's CSS variables.
    ///
    /// https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/css-variables.html
    /// The profile uses flat 34 px controls; desktop preference/accent tracking
    /// is supplied by the application's theme resolver.
    pub fn adwaita(dark: bool) -> Self {
        let bg = if dark { hex(0x222226) } else { hex(0xfafafb) };
        let text = if dark { Color32::WHITE } else { Color32::from_rgba_unmultiplied(0, 0, 6, 204) };
        Self {
            name: "Adwaita".into(),
            dark,
            bg,
            chrome: if dark { hex(0x2e2e32) } else { Color32::WHITE },
            // Dark cards are white at 8% over the window background.
            surface: if dark { hex(0x343437) } else { Color32::WHITE },
            surface_hover: if dark { hex(0x414145) } else { hex(0xebebed) },
            raised: if dark { hex(0x36363a) } else { Color32::WHITE },
            border: text.gamma_multiply(0.15),
            text,
            text_dim: text.gamma_multiply(0.55),
            text_faint: text.gamma_multiply(0.5),
            accent: hex(0x3584e4),
            accent_2: hex(0x3584e4),
            on_accent: Color32::WHITE,
            play: if dark { hex(0x26a269) } else { hex(0x2ec27e) },
            online: if dark { hex(0x81d0ff) } else { hex(0x0461be) },
            in_game: if dark { hex(0x78e9ab) } else { hex(0x007c3d) },
            away: if dark { hex(0xffc252) } else { hex(0x905400) },
            danger: if dark { hex(0xff938c) } else { hex(0xc30000) },
            discount: if dark { hex(0x26a269) } else { hex(0x007c3d) },
            radius: 12,
            weight_preset: WeightPreset::Balanced,
            widget_style: WidgetStyle::Adwaita,
            native_metrics: None,
            font_scale: 1.0,
        }
    }

    /// A saved theme with its preset's current colors. Accent colors, corner
    /// roundness, and typography overrides are kept; the rest comes from the
    /// preset, so fixes to a preset reach themes saved before them. Unknown or
    /// custom theme names stay untouched.
    pub fn refreshed(self) -> Self {
        match Self::presets().into_iter().find(|p| p.name == self.name) {
            Some(preset) => Self {
                accent: self.accent,
                accent_2: self.accent_2,
                radius: self.radius,
                weight_preset: self.weight_preset,
                font_scale: self.font_scale,
                ..preset
            },
            None => self,
        }
    }

    pub fn corner(&self) -> CornerRadius {
        CornerRadius::same(self.radius)
    }

    pub fn corner_small(&self) -> CornerRadius {
        if self.widget_style == WidgetStyle::Adwaita {
            CornerRadius::same(6)
        } else {
            CornerRadius::same((self.radius * 2 / 3).max(2))
        }
    }

    /// A standalone accent for text on neutral surfaces. Libadwaita's accent
    /// fill is paired with white text and is deliberately a different color.
    pub fn accent_text(&self) -> Color32 {
        if let Some(native) = self.native_metrics.filter(|native| native.accent == self.accent) {
            return native.accent_text;
        }
        if self.widget_style != WidgetStyle::Adwaita {
            return self.accent;
        }
        let rgb = (u32::from(self.accent.r()) << 16) | (u32::from(self.accent.g()) << 8) | u32::from(self.accent.b());
        let (light, dark) = match rgb {
            0x3584e4 => (0x0461be, 0x81d0ff),
            0x2190a4 => (0x007184, 0x7bdff4),
            0x3a944a => (0x15772e, 0x8de698),
            0xc88800 => (0x905300, 0xffc057),
            0xed5b00 => (0xb62200, 0xff9c5b),
            0xe62d42 => (0xc00023, 0xff888c),
            0xd56199 => (0xa2326c, 0xffa0d8),
            0x9141ac => (0x8939a4, 0xfba7ff),
            0x6f8396 => (0x526678, 0xbbd1e5),
            // Custom fills have no native standalone counterpart. Keep their
            // neutral-surface labels legible instead of guessing a tint.
            _ => return self.text,
        };
        hex(if self.dark { dark } else { light })
    }

    /// Color for hyperlinks on neutral surfaces.
    pub fn link_color(&self) -> Color32 {
        if self.name.eq_ignore_ascii_case("Metro") && self.dark { Color32::WHITE } else { self.accent_text() }
    }

    /// Background paired with white text for a destructive action or badge.
    pub fn danger_fill(&self) -> Color32 {
        if let Some(native) = self.native_metrics.filter(|native| native.danger == self.danger) {
            return native.danger_fill;
        }
        if self.widget_style == WidgetStyle::Adwaita {
            hex(if self.dark { 0xc01c28 } else { 0xe01b24 })
        } else {
            self.danger
        }
    }

    /// Push these tokens into egui's global style.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        v.override_text_color = Some(self.text);
        v.panel_fill = self.bg;
        v.window_fill = self.raised;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_corner_radius = self.corner();
        v.window_shadow = egui::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(140) };
        v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(120) };
        v.menu_corner_radius = self.corner_small();
        v.extreme_bg_color = self.raised;
        v.faint_bg_color = self.surface;
        v.code_bg_color = self.raised;
        v.hyperlink_color = self.link_color();
        v.selection.bg_fill = self.accent.linear_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, self.accent_text());
        v.text_cursor.stroke = Stroke::new(2.0, self.accent);
        if self.dark && cfg!(target_os = "linux") {
            // Linux: egui's dark default (2c - c²) thickens every edge pixel,
            // which reads as jagged at 1x; a gentle gamma keeps text smooth.
            // Other systems keep egui's defaults.
            v.text_options.color_transfer_function = egui::epaint::FontColorTransferFunction::Gamma(0.75);
        }

        let r = self.corner_small();
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.surface;
        w.noninteractive.weak_bg_fill = self.surface;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text_dim);
        w.inactive.bg_fill = self.raised;
        w.inactive.weak_bg_fill = self.raised;
        w.inactive.bg_stroke = Stroke::new(1.0, self.border);
        w.inactive.fg_stroke = Stroke::new(1.0, self.text);
        w.hovered.bg_fill = self.surface_hover;
        w.hovered.weak_bg_fill = self.surface_hover;
        w.hovered.bg_stroke = Stroke::new(1.0, self.border);
        w.hovered.fg_stroke = Stroke::new(1.5, self.text);
        w.hovered.expansion = 0.0;
        w.active.bg_fill = self.accent.linear_multiply(0.5);
        w.active.weak_bg_fill = self.accent.linear_multiply(0.5);
        w.active.bg_stroke = Stroke::new(1.0, self.accent);
        w.active.fg_stroke = Stroke::new(2.0, self.on_accent);
        w.active.expansion = 0.0;
        w.open.weak_bg_fill = self.surface_hover;
        w.open.bg_stroke = Stroke::new(1.0, self.border);
        if self.widget_style == WidgetStyle::Adwaita {
            w.inactive.bg_fill = self.chrome;
            w.inactive.weak_bg_fill = self.chrome;
            w.inactive.bg_stroke = Stroke::NONE;
            w.hovered.bg_stroke = Stroke::NONE;
            w.active.bg_fill = self.surface_hover;
            w.active.weak_bg_fill = self.surface_hover;
            w.active.fg_stroke = Stroke::new(1.0, self.text);
            w.open.bg_fill = self.surface_hover;
            v.extreme_bg_color = self.native_metrics.map(|m| m.view).unwrap_or_else(|| if self.dark { hex(0x1d1d20) } else { Color32::WHITE });
        }
        for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = r;
        }

        let fs = self.font_scale.clamp(0.75, 1.5);
        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 8.0);
            s.spacing.button_padding = egui::vec2(14.0, 7.0);
            s.spacing.interact_size.y = if self.widget_style == WidgetStyle::Adwaita { self.native_metrics.map(|m| m.min_height).unwrap_or(34.0) } else { 32.0 };
            s.spacing.menu_margin = egui::Margin::same(10);
            s.spacing.scroll.bar_width = 6.0;
            s.spacing.scroll.floating = true;
            s.spacing.scroll.bar_inner_margin = 2.0;
            s.interaction.selectable_labels = false;
            use egui::TextStyle::*;
            s.text_styles = [
                (Heading, FontId::new(24.0 * fs, semibold())),
                (Body, FontId::new(14.0 * fs, FontFamily::Proportional)),
                (Button, FontId::new(14.0 * fs, medium())),
                (Small, FontId::new(12.0 * fs, FontFamily::Proportional)),
                (Monospace, FontId::new(13.0 * fs, FontFamily::Monospace)),
            ]
            .into();
        });
    }
}

pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

// Heavy weight preset: body and medium text in Geist Bold, semibold and bold
// in Geist ExtraBold. Balanced weight preset: Inter Regular/Medium/SemiBold/Bold.
const GEIST_BOLD: &[u8] = include_bytes!("../assets/fonts/Geist-Bold.ttf");
const GEIST_EXTRABOLD: &[u8] = include_bytes!("../assets/fonts/Geist-ExtraBold.ttf");
const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
const INTER_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");
const PHOSPHOR: &[u8] = include_bytes!("../assets/Phosphor.ttf");

/// Text rendering: light hinting (snap stems vertically only, keep the
/// designed shapes) and a softer coverage curve than egui's dark default,
/// so edges are smoothly anti-aliased instead of crunchy at 1x.
fn text_tweak() -> egui::epaint::text::FontTweak {
    use egui::epaint::text::{FontTweak, HintingTarget, SmoothHinting};
    if !cfg!(target_os = "linux") {
        // Only Linux gets the tuned text; elsewhere egui's defaults.
        return FontTweak::default();
    }
    FontTweak {
        hinting: Some(true),
        hinting_target: HintingTarget::Smooth(SmoothHinting { light: true, symmetric_rendering: true, preserve_linear_metrics: true }),
        ..Default::default()
    }
}

/// A system font file, mapped into memory instead of copied: only the parts
/// actually drawn get loaded, and they count as shared file cache rather than
/// the app's own memory (the CJK fonts are 70+ MB together).
fn map_font(path: &std::path::Path) -> Option<&'static [u8]> {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    static MAPPED: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let cache = MAPPED.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(bytes) = cache.lock().ok()?.get(&key).copied() {
        return Some(bytes);
    }
    let file = std::fs::File::open(&key).ok()?;
    if file.metadata().ok()?.len() == 0 {
        return None;
    }
    // SAFETY: system font files don't change while the app runs; the mapping
    // lives for the whole program, like the font itself.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let bytes: &'static [u8] = &Box::leak(Box::new(map))[..];
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, bytes);
    }
    Some(bytes)
}

/// Basic header check before handing a system font to epaint (which panics on corrupt files).
#[cfg(any(target_os = "linux", test))]
fn valid_font_header(bytes: &[u8], index: u32) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    match &bytes[..4] {
        b"ttcf" => {
            let num_fonts = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
            index < num_fonts && 12 + (index as usize + 1) * 4 <= bytes.len()
        }
        [0x00, 0x01, 0x00, 0x00] | b"OTTO" | b"true" | b"typ1" => index == 0,
        _ => false,
    }
}

#[cfg(target_os = "linux")]
mod linux_fonts {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub const MAX_FALLBACK_FONTS: usize = 12;
    const MAX_SCAN_DEPTH: usize = 4;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DiscoveredFont {
        pub path: PathBuf,
        pub index: u32,
        pub is_emoji: bool,
    }

    pub fn discover() -> &'static [DiscoveredFont] {
        static FONTS: std::sync::OnceLock<Vec<DiscoveredFont>> = std::sync::OnceLock::new();
        FONTS.get_or_init(|| {
            let mut fonts = dedup_and_cap(query_fc_match(), MAX_FALLBACK_FONTS);
            if fonts.len() < MAX_FALLBACK_FONTS {
                fonts.extend(discover_fonts_in_dirs(&standard_font_dirs(), MAX_FALLBACK_FONTS));
            }
            dedup_and_cap(fonts, MAX_FALLBACK_FONTS)
        })
    }

    pub fn standard_font_dirs() -> Vec<PathBuf> {
        let mut dirs = vec![
            PathBuf::from("/usr/share/fonts"),
            PathBuf::from("/usr/local/share/fonts"),
        ];
        if let Some(xdg_data) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(xdg_data).join("fonts"));
        } else if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(home).join(".local/share/fonts"));
        }
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(home).join(".fonts"));
        }
        dirs
    }

    /// Mask away Fontconfig's named-instance high bits (upper 16 bits); TTC face index is lower 16 bits.
    pub fn parse_fc_match_line(line: &str, expected_families: Option<&[&str]>, is_emoji: bool) -> Option<DiscoveredFont> {
        let mut parts = line.trim().split('\t');
        let path_str = parts.next()?.trim();
        let index_str = parts.next()?.trim();
        let family = parts.next().unwrap_or_default().trim();
        if path_str.is_empty() || !is_font_ext(Path::new(path_str)) {
            return None;
        }
        if let Some(expected) = expected_families {
            let fam_lower = family.to_ascii_lowercase();
            let file_lower = Path::new(path_str)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let matches = expected.iter().any(|e| fam_lower.contains(e) || file_lower.contains(&e.replace(' ', "")));
            if !matches {
                return None;
            }
        }
        let raw_index: u32 = index_str.parse().ok()?;
        let index = raw_index & 0xFFFF;
        Some(DiscoveredFont {
            path: PathBuf::from(path_str),
            index,
            is_emoji,
        })
    }

    fn query_fc_match() -> Vec<DiscoveredFont> {
        use std::sync::atomic::{AtomicBool, Ordering};

        const LANG_QUERIES: &[&str] = &[
            "sans-serif:lang=zh-cn",
            "sans-serif:lang=zh-tw",
            "sans-serif:lang=ja",
            "sans-serif:lang=ko",
            "sans-serif:lang=ar",
            "sans-serif:lang=th",
            "sans-serif:lang=hi",
            "sans-serif:lang=he",
        ];
        const FAMILY_QUERIES: &[(&str, &[&str], bool)] = &[
            ("DejaVu Sans", &["dejavu sans"], false),
            ("Noto Sans Symbols", &["noto sans symbols"], false),
            ("Noto Sans Symbols 2", &["noto sans symbols 2"], false),
            ("Noto Color Emoji", &["noto color emoji", "emoji"], true),
        ];

        let missing = AtomicBool::new(false);
        let run = |pattern: &str| -> Option<String> {
            if missing.load(Ordering::Relaxed) {
                return None;
            }
            match Command::new("fc-match")
                .args(["-f", "%{file}\t%{index}\t%{family}\n", pattern])
                .output()
            {
                Ok(out) if out.status.success() => String::from_utf8(out.stdout).ok(),
                Ok(_) => None,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        missing.store(true, Ordering::Relaxed);
                    }
                    None
                }
            }
        };

        // Run fontconfig lookups concurrently with bounded, fallible worker
        // threads (falling back to inline execution if the OS refuses a thread)
        // while preserving deterministic priority order.
        enum Slot<'a> {
            Pending(std::thread::ScopedJoinHandle<'a, Option<DiscoveredFont>>),
            Ready(Option<DiscoveredFont>),
        }
        let mut results: Vec<Option<DiscoveredFont>> = std::thread::scope(|s| {
            let mut slots = Vec::with_capacity(LANG_QUERIES.len() + FAMILY_QUERIES.len());
            for &q in LANG_QUERIES {
                let job = move || {
                    let stdout = run(q)?;
                    let line = stdout.lines().next()?;
                    parse_fc_match_line(line, None, false)
                };
                match std::thread::Builder::new().name("aurora-fc-match".into()).spawn_scoped(s, job) {
                    Ok(h) => slots.push(Slot::Pending(h)),
                    Err(_) => slots.push(Slot::Ready(job())),
                }
            }
            for &(pattern, expected, is_emoji) in FAMILY_QUERIES {
                let job = move || {
                    let stdout = run(pattern)?;
                    let line = stdout.lines().next()?;
                    parse_fc_match_line(line, Some(expected), is_emoji)
                };
                match std::thread::Builder::new().name("aurora-fc-match".into()).spawn_scoped(s, job) {
                    Ok(h) => slots.push(Slot::Pending(h)),
                    Err(_) => slots.push(Slot::Ready(job())),
                }
            }
            slots
                .into_iter()
                .map(|slot| match slot {
                    Slot::Pending(h) => h.join().ok().flatten(),
                    Slot::Ready(v) => v,
                })
                .collect()
        });
        results.drain(..).flatten().collect()
    }

    fn is_font_ext(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
            Some("ttf" | "otf" | "ttc")
        )
    }

    fn normalize_stem(path: &Path) -> Option<String> {
        let stem = path.file_stem()?.to_str()?;
        Some(
            stem.chars()
                .filter(|c| !matches!(c, '-' | '_' | ' '))
                .flat_map(|c| c.to_lowercase())
                .collect(),
        )
    }

    /// Assign (group_slot, priority_order, weight_rank, is_emoji). Lower weight_rank is preferred.
    fn classify_candidate(norm_stem: &str) -> Option<(usize, u8, bool)> {
        if norm_stem.contains("italic")
            || norm_stem.contains("oblique")
            || norm_stem.contains("condensed")
            || norm_stem.contains("thin")
            || norm_stem.contains("light")
            || norm_stem.contains("black")
            || norm_stem.contains("serif")
            || norm_stem.contains("mono")
        {
            return None;
        }
        let weight_rank = if norm_stem.contains("regular") {
            0
        } else if norm_stem.contains("medium") {
            1
        } else if !norm_stem.contains("bold") && !norm_stem.contains("semibold") {
            2
        } else if norm_stem.contains("bold") && !norm_stem.contains("semibold") {
            3
        } else {
            return None;
        };

        // Order determines slot priority; one best weight is kept per slot.
        const SLOTS: &[(&str, bool)] = &[
            ("notosanscjksc", false),
            ("notosanscjktc", false),
            ("notosanscjkjp", false),
            ("notosanscjkkr", false),
            ("notosanscjk", false),
            ("notosanssc", false),
            ("notosanstc", false),
            ("notosansjp", false),
            ("notosanskr", false),
            ("sourcehansanssc", false),
            ("sourcehansanscn", false),
            ("sourcehansanstc", false),
            ("sourcehansanstw", false),
            ("sourcehansansjp", false),
            ("sourcehansanskr", false),
            ("sourcehansans", false),
            ("wqymicrohei", false),
            ("wqyzenhei", false),
            ("wenquanyi", false),
            ("ipaexg", false),
            ("ipag", false),
            ("undotum", false),
            ("notosansarabic", false),
            ("notosansthai", false),
            ("notosansdevanagari", false),
            ("notosanshebrew", false),
            ("notosansbengali", false),
            ("notosanstamil", false),
            ("notosanssymbols2", false),
            ("notosanssymbols", false),
            ("dejavusans", false),
            ("notocoloremoji", true),
            ("notoemoji", true),
        ];

        for (slot, &(pat, is_emoji)) in SLOTS.iter().enumerate() {
            if norm_stem.starts_with(pat) || norm_stem.contains(pat) {
                return Some((slot, weight_rank, is_emoji));
            }
        }
        None
    }

    pub fn discover_fonts_in_dirs(dirs: &[PathBuf], cap: usize) -> Vec<DiscoveredFont> {
        let mut best: Vec<Option<(u8, DiscoveredFont)>> = vec![None; 33];
        let mut visited_dirs = HashSet::new();
        for dir in dirs {
            scan_dir(dir, 0, &mut visited_dirs, &mut best);
        }
        let list: Vec<DiscoveredFont> = best.into_iter().flatten().map(|(_, f)| f).collect();
        dedup_and_cap(list, cap)
    }

    fn scan_dir(
        dir: &Path,
        depth: usize,
        visited_dirs: &mut HashSet<PathBuf>,
        best: &mut [Option<(u8, DiscoveredFont)>],
    ) {
        if depth > MAX_SCAN_DEPTH {
            return;
        }
        let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !visited_dirs.insert(canon) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                scan_dir(&path, depth + 1, visited_dirs, best);
            } else if is_font_ext(&path) {
                let Some(norm) = normalize_stem(&path) else {
                    continue;
                };
                let Some((slot, rank, is_emoji)) = classify_candidate(&norm) else {
                    continue;
                };
                let replace = match &best[slot] {
                    None => true,
                    Some((prev_rank, _)) => rank < *prev_rank,
                };
                if replace {
                    best[slot] = Some((
                        rank,
                        DiscoveredFont {
                            path,
                            index: 0,
                            is_emoji,
                        },
                    ));
                }
            }
        }
    }

    pub fn dedup_and_cap(fonts: Vec<DiscoveredFont>, cap: usize) -> Vec<DiscoveredFont> {
        if cap == 0 {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut non_emoji = Vec::new();
        let mut emoji = Vec::new();
        for f in fonts {
            let Ok(meta) = std::fs::metadata(&f.path) else {
                continue;
            };
            if !meta.is_file() || meta.len() == 0 {
                continue;
            }
            let key = std::fs::canonicalize(&f.path).unwrap_or_else(|_| f.path.clone());
            if !seen.insert(key) {
                continue;
            }
            if f.is_emoji {
                emoji.push(f);
            } else {
                non_emoji.push(f);
            }
        }
        let reserve_emoji = usize::from(!emoji.is_empty());
        non_emoji.truncate(cap.saturating_sub(reserve_emoji));
        non_emoji.extend(emoji.into_iter().take(cap.saturating_sub(non_emoji.len())));
        non_emoji
    }
}

/// Install Geist (Bold and ExtraBold, Inter Bold behind it for scripts Geist lacks) plus
/// the icon font, with grayscale anti-aliasing. Call once at startup.
pub fn install_fonts(ctx: &egui::Context) {
    install_fonts_for_theme(ctx, false, WeightPreset::Heavy, None);
}

/// Like [`install_fonts`], but `lcd` renders text and icons with subpixel
/// (RGB) anti-aliasing, like the rest of the desktop. Only for contexts drawn
/// by this app's patched glow painter: the in-game overlay hands its meshes
/// to another renderer and must stay grayscale.
pub fn install_fonts_with(ctx: &egui::Context, lcd: bool) {
    install_fonts_for_theme(ctx, lcd, WeightPreset::Heavy, None);
}

fn valid_custom_font(bytes: &[u8]) -> bool {
    // Font family creation in epaint panics if its parser rejects a face.
    // Ignore damaged theme fonts, including those in an already-active package.
    match skrifa::FontRef::from_index(bytes, 0) {
        Ok(_) => true,
        Err(error) => {
            log::warn!("Ignoring invalid custom font; using bundled fallback: {error}");
            false
        }
    }
}

/// Install fonts for a theme's [`WeightPreset`] and optional [`CustomFonts`],
/// keeping Phosphor icons, Linux LCD subpixel rendering, and system CJK
/// fallbacks intact.
pub fn install_fonts_for_theme(
    ctx: &egui::Context,
    lcd: bool,
    weight: WeightPreset,
    custom: Option<&CustomFonts>,
) {
    use egui::epaint::text::FontTweak;
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    match weight {
        WeightPreset::Heavy => {
            for (name, bytes) in [
                ("geist-bold", GEIST_BOLD),
                ("geist-extrabold", GEIST_EXTRABOLD),
                ("inter-bold", INTER_BOLD),
            ] {
                fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_static(bytes).tweak(text_tweak()).lcd(lcd)));
            }
        }
        WeightPreset::Balanced => {
            for (name, bytes) in [
                ("inter-regular", INTER_REGULAR),
                ("inter-medium", INTER_MEDIUM),
                ("inter-semibold", INTER_SEMIBOLD),
                ("inter-bold", INTER_BOLD),
            ] {
                fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_static(bytes).tweak(text_tweak()).lcd(lcd)));
            }
        }
    }

    let mut custom_regular = None;
    let mut custom_bold = None;
    let mut custom_mono = None;
    if let Some(cf) = custom {
        if let Some(bytes) = cf.regular.as_ref().filter(|bytes| valid_custom_font(bytes)) {
            fonts.font_data.insert("custom-regular".into(), Arc::new(egui::FontData::from_owned(bytes.clone()).tweak(text_tweak()).lcd(lcd)));
            custom_regular = Some("custom-regular");
        }
        if let Some(bytes) = cf.bold.as_ref().filter(|bytes| valid_custom_font(bytes)) {
            fonts.font_data.insert("custom-bold".into(), Arc::new(egui::FontData::from_owned(bytes.clone()).tweak(text_tweak()).lcd(lcd)));
            custom_bold = Some("custom-bold");
        }
        if let Some(bytes) = cf.mono.as_ref().filter(|bytes| valid_custom_font(bytes)) {
            fonts.font_data.insert("custom-mono".into(), Arc::new(egui::FontData::from_owned(bytes.clone()).tweak(text_tweak()).lcd(lcd)));
            custom_mono = Some("custom-mono");
        }
    }

    // Icons are drawn from their outlines unhinted, exactly as the SVGs
    // would be: the auto-hinter otherwise bends them onto the pixel grid.
    let icon_tweak = if cfg!(target_os = "linux") { FontTweak { hinting: Some(false), ..Default::default() } } else { FontTweak::default() };
    let icons = egui::FontData::from_static(PHOSPHOR).tweak(icon_tweak).lcd(lcd);
    fonts.font_data.insert("phosphor".into(), Arc::new(icons));
    // System fonts for scripts Geist and Inter don't cover, so game names in Chinese,
    // Japanese, Korean, Thai, Arabic, Hindi… show instead of empty boxes.
    let mut system: Vec<String> = Vec::new();
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut system_emoji: Vec<String> = Vec::new();
    #[cfg(windows)]
    if let Some(dir) = std::env::var_os("WINDIR").map(|w| std::path::PathBuf::from(w).join("Fonts")) {
        for file in ["msyh.ttc", "YuGothM.ttc", "malgun.ttf", "Nirmala.ttc", "tahoma.ttf", "seguisym.ttf"] {
            if let Some(bytes) = map_font(&dir.join(file)) {
                let name = format!("system-{file}");
                fonts.font_data.insert(name.clone(), Arc::new(egui::FontData::from_static(bytes)));
                system.push(name);
            }
        }
    }
    #[cfg(target_os = "linux")]
    for (i, f) in linux_fonts::discover().iter().enumerate() {
        if let Some(bytes) = map_font(&f.path) {
            if !valid_font_header(bytes, f.index) {
                continue;
            }
            let file_name = f.path.file_name().and_then(|n| n.to_str()).unwrap_or("font");
            let name = format!("system-{i}-{file_name}");
            let mut data = egui::FontData::from_static(bytes);
            data.index = f.index;
            if !f.is_emoji {
                data = data.lcd(lcd);
            }
            fonts.font_data.insert(name.clone(), Arc::new(data));
            if f.is_emoji {
                system_emoji.push(name);
            } else {
                system.push(name);
            }
        }
    }
    // egui's bundled fonts stay as fallbacks for emoji and rare scripts.
    let mut fallbacks: Vec<String> = system.clone();
    fallbacks.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fallbacks.extend(system_emoji.iter().cloned());
    // Phosphor has only private-use glyphs and Inter none, so the icon font
    // can sit right after the text fonts. (Geist's one private-use glyph,
    // U+F8FF, is outside Phosphor's range.)
    let family = |primary: Option<&str>, base: &[&str]| {
        let mut v = Vec::new();
        if let Some(p) = primary {
            v.push(p.to_string());
        }
        for b in base {
            v.push((*b).to_string());
        }
        v.push("phosphor".to_string());
        v.extend(fallbacks.iter().cloned());
        v
    };
    let bold_primary = custom_bold.or(custom_regular);
    match weight {
        WeightPreset::Heavy => {
            fonts.families.insert(FontFamily::Proportional, family(custom_regular, &["geist-bold", "inter-bold"]));
            fonts.families.insert(medium(), family(custom_regular, &["geist-bold", "inter-bold"]));
            fonts.families.insert(semibold(), family(bold_primary, &["geist-extrabold", "inter-bold"]));
            fonts.families.insert(bold(), family(bold_primary, &["geist-extrabold", "inter-bold"]));
        }
        WeightPreset::Balanced => {
            fonts.families.insert(FontFamily::Proportional, family(custom_regular, &["inter-regular"]));
            fonts.families.insert(medium(), family(custom_regular, &["inter-medium"]));
            fonts.families.insert(semibold(), family(bold_primary, &["inter-semibold"]));
            fonts.families.insert(bold(), family(bold_primary, &["inter-bold"]));
        }
    }
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    if let Some(cm) = custom_mono {
        mono.insert(0, cm.to_string());
    }
    mono.push("phosphor".into());
    mono.extend(fallbacks);
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dark_metro_uses_white_links() {
        let mut metro = Theme { name: "Metro".into(), dark: true, ..Theme::default() };
        assert_eq!(metro.link_color(), Color32::WHITE);
        metro.dark = false;
        assert_eq!(metro.link_color(), metro.accent_text());
        assert_ne!(metro.link_color(), Color32::WHITE);
    }

    #[test]
    fn valid_font_header_checks_sfnt_and_ttc_bounds() {
        assert!(!valid_font_header(&[], 0));
        assert!(!valid_font_header(&[0; 12], 0));
        let ttf = [0x00, 0x01, 0x00, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        assert!(valid_font_header(&ttf, 0));
        assert!(!valid_font_header(&ttf, 1));

        let otf = [b'O', b'T', b'T', b'O', 0, 1, 0, 0, 0, 0, 0, 0];
        assert!(valid_font_header(&otf, 0));

        // TTC with 2 fonts needs 12 + 2*4 = 20 bytes minimum.
        let mut ttc = vec![0u8; 20];
        ttc[..4].copy_from_slice(b"ttcf");
        ttc[8..12].copy_from_slice(&2u32.to_be_bytes());
        assert!(valid_font_header(&ttc, 0));
        assert!(valid_font_header(&ttc, 1));
        assert!(!valid_font_header(&ttc, 2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fc_match_line_parses_and_masks_variable_instance_bits() {
        let f = linux_fonts::parse_fc_match_line(
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc\t2\tNoto Sans CJK SC",
            None,
            false,
        )
        .unwrap();
        assert_eq!(f.path, std::path::Path::new("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"));
        assert_eq!(f.index, 2);
        assert!(!f.is_emoji);

        // Variable font named-instance bits live in upper 16 bits.
        let vf = linux_fonts::parse_fc_match_line(
            "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf\t458752\tAdwaita Sans",
            None,
            false,
        )
        .unwrap();
        assert_eq!(vf.index, 0);

        // Family filter rejects mismatched fallback for explicit family queries.
        assert!(linux_fonts::parse_fc_match_line(
            "/usr/share/fonts/TTF/DejaVuSans.ttf\t0\tDejaVu Sans",
            Some(&["noto color emoji"]),
            true,
        )
        .is_none());
        assert!(linux_fonts::parse_fc_match_line(
            "/usr/share/fonts/noto/NotoColorEmoji.ttf\t0\tNoto Color Emoji",
            Some(&["noto color emoji"]),
            true,
        )
        .is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_dir_discovery_deduplicates_prefers_regular_and_caps() {
        let tmp = std::env::temp_dir().join(format!("aurora-font-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let sub = tmp.join("noto-cjk");
        std::fs::create_dir_all(&sub).unwrap();

        let make = |p: &std::path::Path| {
            std::fs::write(p, b"OTTO\x00\x01\x00\x00\x00\x00\x00\x00").unwrap();
        };
        make(&sub.join("NotoSansCJK-Bold.ttc"));
        make(&sub.join("NotoSansCJK-Regular.ttc"));
        make(&sub.join("NotoSansCJK-Italic.ttc"));
        make(&tmp.join("SourceHanSansSC-Regular.otf"));
        make(&tmp.join("wqy-microhei.ttc"));
        make(&tmp.join("ipag.ttf"));
        make(&tmp.join("UnDotum.ttf"));
        make(&tmp.join("DejaVuSans.ttf"));
        make(&tmp.join("NotoColorEmoji.ttf"));
        make(&tmp.join("NotoSansArabic-Regular.ttf"));
        make(&tmp.join("NotoSansThai-Regular.ttf"));
        make(&tmp.join("NotoSansDevanagari-Regular.ttf"));
        make(&tmp.join("NotoSansHebrew-Regular.ttf"));
        make(&tmp.join("NotoSansBengali-Regular.ttf"));
        make(&tmp.join("NotoSansTamil-Regular.ttf"));
        make(&tmp.join("NotoSansSymbols-Regular.ttf"));
        // Symlink duplicate should be deduplicated by canonical path.
        let _ = std::os::unix::fs::symlink(tmp.join("DejaVuSans.ttf"), tmp.join("DejaVuSans-Regular.ttf"));

        let discovered = linux_fonts::discover_fonts_in_dirs(&[tmp.clone()], 8);
        assert_eq!(discovered.len(), 8);
        // NotoSansCJK-Regular.ttc chosen over Bold/Italic.
        assert!(discovered.iter().any(|f| f.path.ends_with("NotoSansCJK-Regular.ttc")));
        assert!(!discovered.iter().any(|f| f.path.ends_with("NotoSansCJK-Bold.ttc")));
        assert!(!discovered.iter().any(|f| f.path.ends_with("NotoSansCJK-Italic.ttc")));
        // Emoji stays last so it never shadows symbol/text fonts.
        assert!(discovered.last().unwrap().is_emoji);
        assert!(discovered.last().unwrap().path.ends_with("NotoColorEmoji.ttf"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_fonts_registers_fallbacks_into_egui_context() {
        let ctx = egui::Context::default();
        install_fonts_with(&ctx, false);
        let discovered = linux_fonts::discover();
        // Verify fonts load cleanly and text layout works for Latin, CJK, and symbols without panic.
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = ui.painter().layout_no_wrap(
                "Aurora 永劫无间 コール オブ デューティ 한국어 ★".into(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            );
            assert!(galley.size().x > 0.0);
            assert_eq!(galley.rows.len(), 1);
            assert!(galley.rows[0].glyphs.iter().all(|g| g.advance_width > 0.0 || g.chr == ' '));
            if !discovered.is_empty() {
                ui.fonts(|f| {
                    let defs = f.definitions();
                    for fam in [FontFamily::Proportional, medium(), semibold(), bold(), FontFamily::Monospace] {
                        assert!(
                            defs.families[&fam].iter().any(|n| n.starts_with("system-")),
                            "missing Linux fallback font in {fam:?}"
                        );
                    }
                });
            }
        });
        out.textures_delta.clear();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn map_font_caches_mapped_slice_and_rejects_empty() {
        let tmp = std::env::temp_dir().join(format!("aurora-map-font-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let empty = tmp.join("empty.ttf");
        std::fs::write(&empty, b"").unwrap();
        assert!(map_font(&empty).is_none());

        let font = tmp.join("dummy.ttf");
        std::fs::write(&font, b"\x00\x01\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00").unwrap();
        let first = map_font(&font).unwrap();
        let second = map_font(&font).unwrap();
        assert!(std::ptr::eq(first.as_ptr(), second.as_ptr()));
        assert_eq!(first.len(), 12);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn malformed_custom_fonts_fall_back_without_crashing_text_layout() {
        for bytes in [Vec::new(), b"not a font".to_vec(), INTER_REGULAR[..128].to_vec()] {
            let ctx = egui::Context::default();
            let custom = CustomFonts {
                regular: Some(bytes.clone()),
                bold: Some(bytes.clone()),
                mono: Some(bytes),
            };
            install_fonts_for_theme(&ctx, false, WeightPreset::Balanced, Some(&custom));
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.label("Regular fallback");
                    ui.label(egui::RichText::new("Bold fallback").family(bold()));
                    ui.monospace("Monospace fallback");
                });
            });
            output.textures_delta.clear();
        }
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |color: Color32| {
            let linear = |channel: u8| {
                let channel = channel as f32 / 255.0;
                if channel <= 0.04045 { channel / 12.92 } else { ((channel + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn adwaita_accent_labels_have_readable_contrast_for_every_system_accent() {
        let ctx = egui::Context::default();
        for dark in [false, true] {
            let mut theme = Theme::adwaita(dark);
            for accent in [0x3584e4, 0x2190a4, 0x3a944a, 0xc88800, 0xed5b00, 0xe62d42, 0xd56199, 0x9141ac, 0x6f8396] {
                theme.accent = hex(accent);
                theme.apply(&ctx);
                let color = ctx.global_style().visuals.hyperlink_color;
                assert!(contrast(color, theme.bg) >= 4.5, "accent {accent:x}, dark={dark}");
            }
        }
    }

    #[test]
    fn adwaita_destructive_badge_text_has_readable_contrast() {
        for dark in [false, true] {
            let theme = Theme::adwaita(dark);
            assert!(contrast(Color32::WHITE, theme.danger_fill()) >= 4.5);
        }
    }

    #[test]
    fn adwaita_applies_native_surfaces_and_control_metrics() {
        let ctx = egui::Context::default();
        for (dark, bg, chrome) in [(false, 0xfafafb, 0xffffff), (true, 0x222226, 0x2e2e32)] {
            Theme::adwaita(dark).apply(&ctx);
            let style = ctx.global_style();
            assert_eq!(style.visuals.dark_mode, dark);
            assert_eq!(style.visuals.panel_fill, hex(bg));
            assert_eq!(style.visuals.widgets.inactive.bg_fill, hex(chrome));
            assert_eq!(style.spacing.interact_size.y, 34.0);
            assert_eq!(style.visuals.widgets.inactive.corner_radius, CornerRadius::same(6));
        }
    }

    #[test]
    fn switching_from_adwaita_restores_aurora_style() {
        let ctx = egui::Context::default();
        Theme::adwaita(false).apply(&ctx);
        Theme::midnight().apply(&ctx);
        let style = ctx.global_style();
        assert_eq!(style.spacing.interact_size.y, 32.0);
        assert_eq!(style.visuals.panel_fill, hex(0x0a0d12));
        assert_eq!(style.visuals.widgets.inactive.corner_radius, CornerRadius::same(8));
    }

    #[test]
    fn preset_by_name_is_case_insensitive() {
        assert_eq!(Theme::preset_by_name("nevko").map(|t| t.name), Some("NEVKO".into()));
        assert_eq!(Theme::preset_by_name("OLED").map(|t| t.name), Some("OLED".into()));
        assert!(Theme::preset_by_name("custom-unknown").is_none());
    }

    #[test]
    fn refreshed_keeps_unknown_custom_theme_untouched() {
        let mut custom = Theme::nevko();
        custom.name = "My Custom Theme".into();
        custom.bg = Color32::from_rgb(12, 34, 56);
        custom.weight_preset = WeightPreset::Balanced;
        custom.font_scale = 1.1;

        assert_eq!(custom.clone().refreshed(), custom);
    }

    #[test]
    fn refreshed_updates_preset_colors_while_preserving_overrides() {
        let mut saved = Theme::nevko();
        let accent = Color32::from_rgb(255, 100, 50);
        saved.bg = Color32::from_rgb(1, 2, 3);
        saved.accent = accent;
        saved.radius = 4;
        saved.weight_preset = WeightPreset::Balanced;
        saved.font_scale = 0.95;

        let out = saved.refreshed();
        assert_eq!(out.bg, Theme::nevko().bg);
        assert_eq!(out.accent, accent);
        assert_eq!(out.radius, 4);
        assert_eq!(out.weight_preset, WeightPreset::Balanced);
        assert_eq!(out.font_scale, 0.95);
    }
}

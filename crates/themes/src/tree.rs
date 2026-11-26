//! Generic CSS-like box-model & declarative UI component tree (`layout.toml`).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::color::ColorRef;

/// Box-model edge insets (padding / margin).
///
/// Deserializes from:
/// - a single number `v` -> all four edges = `v`
/// - a 2-element array `[vertical, horizontal]`
/// - a 4-element array `[top, right, bottom, left]`
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const ZERO: Self = Self { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 };

    pub const fn all(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }

    pub const fn symmetric(vertical: f32, horizontal: f32) -> Self {
        Self { top: vertical, right: horizontal, bottom: vertical, left: horizontal }
    }

    pub fn to_egui_margin(&self) -> egui::Margin {
        let clamp_i8 = |v: f32| v.round().clamp(i8::MIN as f32, i8::MAX as f32) as i8;
        egui::Margin {
            left: clamp_i8(self.left),
            right: clamp_i8(self.right),
            top: clamp_i8(self.top),
            bottom: clamp_i8(self.bottom),
        }
    }
}

impl Serialize for Edges {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        [self.top, self.right, self.bottom, self.left].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Edges {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EdgesVisitor;

        impl<'de> de::Visitor<'de> for EdgesVisitor {
            type Value = Edges;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a number, [vertical, horizontal], or [top, right, bottom, left]")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(Edges::all(v as f32))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(Edges::all(v as f32))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(Edges::all(v as f32))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut vals: Vec<f32> = Vec::with_capacity(4);
                while let Some(v) = seq.next_element::<f32>()? {
                    vals.push(v);
                }
                match vals.as_slice() {
                    [all] => Ok(Edges::all(*all)),
                    [v, h] => Ok(Edges::symmetric(*v, *h)),
                    [top, right, bottom, left] => {
                        Ok(Edges { top: *top, right: *right, bottom: *bottom, left: *left })
                    }
                    other => Err(de::Error::invalid_length(other.len(), &self)),
                }
            }
        }

        deserializer.deserialize_any(EdgesVisitor)
    }
}

/// Dimension value: explicit pixels, `"fill"`, or `"auto"`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SizeVal {
    Px(f32),
    Fill,
    #[default]
    Auto,
}

impl Serialize for SizeVal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Px(v) => serializer.serialize_f32(v),
            Self::Fill => serializer.serialize_str("fill"),
            Self::Auto => serializer.serialize_str("auto"),
        }
    }
}

impl<'de> Deserialize<'de> for SizeVal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SizeValVisitor;

        impl<'de> de::Visitor<'de> for SizeValVisitor {
            type Value = SizeVal;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a pixel number, \"fill\", or \"auto\"")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(SizeVal::Px(v as f32))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(SizeVal::Px(v as f32))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(SizeVal::Px(v as f32))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v.trim().to_ascii_lowercase().as_str() {
                    "fill" => Ok(SizeVal::Fill),
                    "auto" => Ok(SizeVal::Auto),
                    _ => Err(de::Error::custom(format!(
                        "invalid size '{v}' (expected number, \"fill\", or \"auto\")"
                    ))),
                }
            }
        }

        deserializer.deserialize_any(SizeValVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignMode {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontWeight {
    #[default]
    Regular,
    Medium,
    Semibold,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelSide {
    Top,
    Bottom,
    Left,
    Right,
    #[default]
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorStyle {
    None,
    Underline,
    #[default]
    Pill,
    LeftBar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WinCtrlVariant {
    #[default]
    Windows,
    Macos,
    Minimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientSpec {
    pub from: ColorRef,
    pub to: ColorRef,
    #[serde(default)]
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BorderEdge {
    #[serde(default = "default_one")]
    pub width: f32,
    pub color: ColorRef,
}

/// Generic CSS-like box-model and visual style attached to any [`Node`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeStyle {
    pub width: Option<SizeVal>,
    pub height: Option<SizeVal>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub padding: Option<Edges>,
    pub margin: Option<Edges>,
    pub gap: Option<f32>,
    pub bg: Option<ColorRef>,
    pub bg_gradient: Option<GradientSpec>,
    pub hover_bg: Option<ColorRef>,
    pub active_bg: Option<ColorRef>,
    pub border: Option<BorderEdge>,
    pub border_top: Option<BorderEdge>,
    pub border_bottom: Option<BorderEdge>,
    pub border_left: Option<BorderEdge>,
    pub border_right: Option<BorderEdge>,
    pub radius: Option<u8>,
    pub color: Option<ColorRef>,
    pub hover_color: Option<ColorRef>,
    pub active_color: Option<ColorRef>,
    pub font_size: Option<f32>,
    pub font_weight: Option<FontWeight>,
    pub uppercase: Option<bool>,
    pub align: Option<AlignMode>,
}

/// Maximum nesting depth for a declarative menu submenu.
pub const MAX_MENU_DEPTH: usize = 4;

/// An entry in a declarative [`Node::Menu`] dropdown.
///
/// Accepts the string `"separator"`, a table `{ label, icon?, action }`,
/// or a nested submenu table `{ label, icon?, items = [...] }`.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuItemSpec {
    Separator,
    Item {
        label: String,
        icon: Option<String>,
        action: String,
    },
    Submenu {
        label: String,
        icon: Option<String>,
        items: Vec<MenuItemSpec>,
    },
}

impl Serialize for MenuItemSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct ItemHelper<'a> {
            label: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            icon: Option<&'a str>,
            action: &'a str,
        }
        #[derive(Serialize)]
        struct SubmenuHelper<'a> {
            label: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            icon: Option<&'a str>,
            items: &'a [MenuItemSpec],
        }
        match self {
            Self::Separator => serializer.serialize_str("separator"),
            Self::Item { label, icon, action } => ItemHelper {
                label,
                icon: icon.as_deref(),
                action,
            }
            .serialize(serializer),
            Self::Submenu { label, icon, items } => SubmenuHelper {
                label,
                icon: icon.as_deref(),
                items,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MenuItemSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct ItemHelper {
            label: String,
            #[serde(default)]
            icon: Option<String>,
            #[serde(default)]
            action: Option<String>,
            #[serde(default)]
            items: Option<Vec<MenuItemSpec>>,
        }

        struct MenuItemVisitor;

        impl<'de> de::Visitor<'de> for MenuItemVisitor {
            type Value = MenuItemSpec;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "\"separator\", {{ label, icon?, action }}, or {{ label, icon?, items }}")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v.eq_ignore_ascii_case("separator") {
                    Ok(MenuItemSpec::Separator)
                } else {
                    Err(de::Error::custom(format!(
                        "unexpected menu item string '{v}' (expected \"separator\")"
                    )))
                }
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let h = ItemHelper::deserialize(de::value::MapAccessDeserializer::new(map))?;
                if let Some(items) = h.items {
                    Ok(MenuItemSpec::Submenu {
                        label: h.label,
                        icon: h.icon,
                        items,
                    })
                } else if let Some(action) = h.action {
                    Ok(MenuItemSpec::Item {
                        label: h.label,
                        icon: h.icon,
                        action,
                    })
                } else {
                    Err(de::Error::custom("menu item requires either 'action' or 'items'"))
                }
            }
        }

        deserializer.deserialize_any(MenuItemVisitor)
    }
}

/// Ordering of the themed library sidebar. Existing themes keep installation groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryGrouping {
    #[default]
    Installed,
    LastPlayed,
}

fn default_true() -> bool {
    true
}
fn default_one() -> f32 {
    1.0
}
fn default_logo_size() -> f32 {
    20.0
}
fn default_indicator_height() -> f32 {
    2.5
}
fn default_tab_gap() -> f32 {
    12.0
}
fn default_icon_btn_size() -> f32 {
    28.0
}
fn default_win_ctrl_height() -> f32 {
    28.0
}
fn default_avatar_size() -> f32 {
    22.0
}
fn default_friends_drawer_width() -> f32 {
    280.0
}
fn default_filters_variant() -> String {
    "default".into()
}
fn default_list_row_height() -> f32 {
    26.0
}
fn default_list_icon_size() -> f32 {
    18.0
}
fn default_whats_new_w() -> f32 {
    240.0
}
fn default_whats_new_h() -> f32 {
    135.0
}
fn default_whats_new_max() -> usize {
    5
}
fn default_gap() -> f32 {
    12.0
}
fn default_featured_w() -> f32 {
    360.0
}
fn default_capsule_w() -> f32 {
    156.0
}
fn default_capsule_h() -> f32 {
    234.0
}
fn default_card_aspect() -> String {
    "2:3".into()
}

/// A declarative UI node in Aurora's shell or library layout tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    // ── Containers ───────────────────────────────────────────────────────
    Panel {
        side: PanelSide,
        #[serde(default)]
        size: Option<f32>,
        #[serde(default)]
        resizable: bool,
        #[serde(default)]
        drag_window: bool,
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Row {
        #[serde(default)]
        justify: Justify,
        #[serde(default)]
        align: AlignMode,
        #[serde(default)]
        wrap: bool,
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Col {
        #[serde(default)]
        align: AlignMode,
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Box {
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Scroll {
        #[serde(default)]
        direction: Direction,
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
    Spacer {
        #[serde(default)]
        size: Option<f32>,
        #[serde(default)]
        flex: bool,
        #[serde(default)]
        style: NodeStyle,
    },
    Divider {
        #[serde(default)]
        orientation: Direction,
        #[serde(default)]
        color: Option<ColorRef>,
        #[serde(default = "default_one")]
        thickness: f32,
        #[serde(default)]
        style: NodeStyle,
    },

    // ── Shell Components ─────────────────────────────────────────────────
    Logo {
        #[serde(default = "default_logo_size")]
        size: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    Text {
        value: String,
        #[serde(default)]
        style: NodeStyle,
    },
    Button {
        label: String,
        #[serde(default)]
        icon: Option<String>,
        action: String,
        #[serde(default)]
        style: NodeStyle,
    },
    Menu {
        label: String,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        items: Vec<MenuItemSpec>,
        #[serde(default)]
        style: NodeStyle,
    },
    NavTabs {
        #[serde(default)]
        direction: Direction,
        #[serde(default = "default_true")]
        show_icons: bool,
        #[serde(default)]
        uppercase: bool,
        #[serde(default)]
        profile_as_username: bool,
        #[serde(default)]
        indicator: IndicatorStyle,
        #[serde(default = "default_indicator_height")]
        indicator_height: f32,
        #[serde(default = "default_tab_gap")]
        tab_gap: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    BackForward {
        #[serde(default = "default_icon_btn_size")]
        size: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    WindowControls {
        #[serde(default)]
        variant: WinCtrlVariant,
        #[serde(default = "default_win_ctrl_height")]
        height: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    Account {
        #[serde(default = "default_true")]
        show_avatar: bool,
        #[serde(default = "default_true")]
        show_name: bool,
        #[serde(default)]
        show_wallet: bool,
        #[serde(default = "default_true")]
        show_caret: bool,
        #[serde(default = "default_avatar_size")]
        avatar_size: f32,
        /// Optional image rounding, independent of the account button frame.
        #[serde(default)]
        avatar_radius: Option<u8>,
        /// A primary-click action; absent preserves the account/status menu.
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        style: NodeStyle,
    },
    Wallet {
        #[serde(default)]
        style: NodeStyle,
    },
    Notifications {
        #[serde(default = "default_icon_btn_size")]
        size: f32,
        #[serde(default)]
        highlight_unread_bg: Option<ColorRef>,
        #[serde(default)]
        style: NodeStyle,
    },
    FriendsButton {
        #[serde(default)]
        icon: Option<String>,
        #[serde(default = "default_icon_btn_size")]
        size: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    DownloadsBar {
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        style: NodeStyle,
    },
    FriendsDrawer {
        #[serde(default = "default_friends_drawer_width")]
        width: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    PageContent {
        #[serde(default)]
        style: NodeStyle,
    },

    // ── Library Components ───────────────────────────────────────────────
    LibrarySearch {
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        show_filter_button: bool,
        #[serde(default)]
        style: NodeStyle,
    },
    LibraryFilters {
        #[serde(default = "default_filters_variant")]
        variant: String,
        #[serde(default)]
        style: NodeStyle,
    },
    LibraryList {
        #[serde(default)]
        group_by: LibraryGrouping,
        #[serde(default = "default_list_row_height")]
        row_height: f32,
        #[serde(default = "default_list_icon_size")]
        icon_size: f32,
        #[serde(default = "default_true")]
        group_headers: bool,
        #[serde(default = "default_true")]
        show_counts: bool,
        #[serde(default)]
        style: NodeStyle,
    },
    WhatsNewShelf {
        #[serde(default)]
        title: Option<String>,
        #[serde(default = "default_whats_new_w")]
        card_width: f32,
        #[serde(default = "default_whats_new_h")]
        card_height: f32,
        #[serde(default)]
        card_radius: u8,
        #[serde(default)]
        date_above: bool,
        #[serde(default = "default_whats_new_max")]
        max_items: usize,
        #[serde(default = "default_gap")]
        gap: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    RecentGamesShelf {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        featured_first: bool,
        #[serde(default = "default_featured_w")]
        featured_width: f32,
        #[serde(default = "default_capsule_w")]
        capsule_width: f32,
        #[serde(default = "default_capsule_h")]
        capsule_height: f32,
        #[serde(default)]
        card_radius: u8,
        #[serde(default = "default_gap")]
        gap: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    AllGamesGrid {
        #[serde(default)]
        title: Option<String>,
        #[serde(default = "default_true")]
        show_count: bool,
        #[serde(default = "default_true")]
        show_sort: bool,
        #[serde(default = "default_card_aspect")]
        card_aspect: String,
        #[serde(default = "default_capsule_w")]
        card_width: f32,
        #[serde(default = "default_capsule_h")]
        card_height: f32,
        #[serde(default)]
        card_radius: u8,
        #[serde(default = "default_gap")]
        gap: f32,
        #[serde(default)]
        style: NodeStyle,
    },
    LibraryHomeOrDetail {
        #[serde(default)]
        style: NodeStyle,
        #[serde(default)]
        children: Vec<Node>,
    },
}

/// Top-level declarative layout tree for a theme (`layout.toml` or inline `[layout]`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutTree {
    pub shell: Vec<Node>,
    pub library: Vec<Node>,
}

/// Maximum declarative layout source size (1 MiB).
pub const MAX_LAYOUT_TOML_BYTES: usize = 1024 * 1024;
/// Maximum nesting depth for a declarative layout tree.
pub const MAX_TREE_DEPTH: usize = 32;
/// Maximum total number of nodes across `shell` and `library`.
pub const MAX_TREE_NODES: usize = 2_000;
/// Maximum menu items inside a single `Node::Menu`.
pub const MAX_MENU_ITEMS: usize = 128;

fn check_finite(val: f32, name: &str, min: f32, max: f32) -> Result<f32, String> {
    if !val.is_finite() {
        return Err(format!("non-finite numeric value for '{name}'"));
    }
    Ok(val.clamp(min, max))
}

fn sanitize_opt_f32(slot: &mut Option<f32>, name: &str, min: f32, max: f32) -> Result<(), String> {
    if let Some(v) = slot.as_mut() {
        *v = check_finite(*v, name, min, max)?;
    }
    Ok(())
}

fn sanitize_edges(e: &mut Edges, name: &str) -> Result<(), String> {
    e.top = check_finite(e.top, name, -256.0, 512.0)?;
    e.right = check_finite(e.right, name, -256.0, 512.0)?;
    e.bottom = check_finite(e.bottom, name, -256.0, 512.0)?;
    e.left = check_finite(e.left, name, -256.0, 512.0)?;
    Ok(())
}

fn sanitize_size_val(slot: &mut Option<SizeVal>, name: &str) -> Result<(), String> {
    if let Some(SizeVal::Px(v)) = slot.as_mut() {
        *v = check_finite(*v, name, 0.0, 8192.0)?;
    }
    Ok(())
}

fn sanitize_border(slot: &mut Option<BorderEdge>, name: &str) -> Result<(), String> {
    if let Some(b) = slot.as_mut() {
        b.width = check_finite(b.width, name, 0.0, 64.0)?;
    }
    Ok(())
}

fn sanitize_style(s: &mut NodeStyle) -> Result<(), String> {
    sanitize_size_val(&mut s.width, "style.width")?;
    sanitize_size_val(&mut s.height, "style.height")?;
    sanitize_opt_f32(&mut s.min_width, "style.min_width", 0.0, 8192.0)?;
    sanitize_opt_f32(&mut s.max_width, "style.max_width", 0.0, 8192.0)?;
    sanitize_opt_f32(&mut s.min_height, "style.min_height", 0.0, 8192.0)?;
    if let Some(p) = s.padding.as_mut() {
        sanitize_edges(p, "style.padding")?;
    }
    if let Some(m) = s.margin.as_mut() {
        sanitize_edges(m, "style.margin")?;
    }
    sanitize_opt_f32(&mut s.gap, "style.gap", 0.0, 512.0)?;
    sanitize_border(&mut s.border, "style.border.width")?;
    sanitize_border(&mut s.border_top, "style.border_top.width")?;
    sanitize_border(&mut s.border_bottom, "style.border_bottom.width")?;
    sanitize_border(&mut s.border_left, "style.border_left.width")?;
    sanitize_border(&mut s.border_right, "style.border_right.width")?;
    sanitize_opt_f32(&mut s.font_size, "style.font_size", 4.0, 128.0)?;
    Ok(())
}

fn validate_nodes(nodes: &mut [Node], depth: usize, total: &mut usize) -> Result<(), String> {
    if depth > MAX_TREE_DEPTH {
        return Err(format!("layout tree exceeds maximum nesting depth of {MAX_TREE_DEPTH}"));
    }
    for node in nodes {
        *total += 1;
        if *total > MAX_TREE_NODES {
            return Err(format!("layout tree exceeds maximum of {MAX_TREE_NODES} nodes"));
        }
        match node {
            Node::Panel { size, style, children, .. } => {
                sanitize_opt_f32(size, "panel.size", 0.0, 4096.0)?;
                sanitize_style(style)?;
                validate_nodes(children, depth + 1, total)?;
            }
            Node::Row { style, children, .. }
            | Node::Col { style, children, .. }
            | Node::Box { style, children }
            | Node::Scroll { style, children, .. }
            | Node::LibraryHomeOrDetail { style, children } => {
                sanitize_style(style)?;
                validate_nodes(children, depth + 1, total)?;
            }
            Node::Spacer { size, style, .. } => {
                sanitize_opt_f32(size, "spacer.size", 0.0, 4096.0)?;
                sanitize_style(style)?;
            }
            Node::Divider { thickness, style, .. } => {
                *thickness = check_finite(*thickness, "divider.thickness", 0.0, 64.0)?;
                sanitize_style(style)?;
            }
            Node::Logo { size, style } => {
                *size = check_finite(*size, "logo.size", 1.0, 512.0)?;
                sanitize_style(style)?;
            }
            Node::Text { style, .. }
            | Node::Button { style, .. }
            | Node::Wallet { style }
            | Node::PageContent { style }
            | Node::LibrarySearch { style, .. }
            | Node::LibraryFilters { style, .. } => {
                sanitize_style(style)?;
            }
            Node::Menu { items, style, .. } => {
                fn validate_menu_items(items: &[MenuItemSpec], depth: usize, total: &mut usize) -> Result<(), String> {
                    if depth > MAX_MENU_DEPTH {
                        return Err(format!("menu submenu nesting exceeds maximum depth of {MAX_MENU_DEPTH}"));
                    }
                    for item in items {
                        *total += 1;
                        if *total > MAX_MENU_ITEMS {
                            return Err(format!("menu exceeds maximum of {MAX_MENU_ITEMS} items"));
                        }
                        if let MenuItemSpec::Submenu { items: sub, .. } = item {
                            validate_menu_items(sub, depth + 1, total)?;
                        }
                    }
                    Ok(())
                }
                let mut total_items = 0;
                validate_menu_items(items, 1, &mut total_items)?;
                sanitize_style(style)?;
            }
            Node::NavTabs { indicator_height, tab_gap, style, .. } => {
                *indicator_height = check_finite(*indicator_height, "nav_tabs.indicator_height", 0.0, 64.0)?;
                *tab_gap = check_finite(*tab_gap, "nav_tabs.tab_gap", 0.0, 256.0)?;
                sanitize_style(style)?;
            }
            Node::BackForward { size, style }
            | Node::Notifications { size, style, .. }
            | Node::FriendsButton { size, style, .. } => {
                *size = check_finite(*size, "control.size", 1.0, 256.0)?;
                sanitize_style(style)?;
            }
            Node::WindowControls { height, style, .. } => {
                *height = check_finite(*height, "window_controls.height", 1.0, 256.0)?;
                sanitize_style(style)?;
            }
            Node::Account { avatar_size, style, .. } => {
                *avatar_size = check_finite(*avatar_size, "account.avatar_size", 1.0, 256.0)?;
                sanitize_style(style)?;
            }
            Node::DownloadsBar { width, style } => {
                sanitize_opt_f32(width, "downloads_bar.width", 0.0, 4096.0)?;
                sanitize_style(style)?;
            }
            Node::FriendsDrawer { width, style } => {
                *width = check_finite(*width, "friends_drawer.width", 0.0, 4096.0)?;
                sanitize_style(style)?;
            }
            Node::LibraryList { row_height, icon_size, style, .. } => {
                *row_height = check_finite(*row_height, "library_list.row_height", 8.0, 256.0)?;
                *icon_size = check_finite(*icon_size, "library_list.icon_size", 4.0, 256.0)?;
                sanitize_style(style)?;
            }
            Node::WhatsNewShelf { card_width, card_height, max_items, gap, style, .. } => {
                *card_width = check_finite(*card_width, "whats_new_shelf.card_width", 16.0, 2048.0)?;
                *card_height = check_finite(*card_height, "whats_new_shelf.card_height", 16.0, 2048.0)?;
                *max_items = (*max_items).min(100);
                *gap = check_finite(*gap, "whats_new_shelf.gap", 0.0, 256.0)?;
                sanitize_style(style)?;
            }
            Node::RecentGamesShelf { featured_width, capsule_width, capsule_height, gap, style, .. } => {
                *featured_width = check_finite(*featured_width, "recent_games_shelf.featured_width", 16.0, 2048.0)?;
                *capsule_width = check_finite(*capsule_width, "recent_games_shelf.capsule_width", 16.0, 2048.0)?;
                *capsule_height = check_finite(*capsule_height, "recent_games_shelf.capsule_height", 16.0, 2048.0)?;
                *gap = check_finite(*gap, "recent_games_shelf.gap", 0.0, 256.0)?;
                sanitize_style(style)?;
            }
            Node::AllGamesGrid { card_width, card_height, gap, style, .. } => {
                *card_width = check_finite(*card_width, "all_games_grid.card_width", 16.0, 2048.0)?;
                *card_height = check_finite(*card_height, "all_games_grid.card_height", 16.0, 2048.0)?;
                *gap = check_finite(*gap, "all_games_grid.gap", 0.0, 256.0)?;
                sanitize_style(style)?;
            }
        }
    }
    Ok(())
}

impl LayoutTree {
    /// Parse a `layout.toml` document into a validated [`LayoutTree`].
    pub fn parse(src: &str) -> Result<Self, String> {
        if src.len() > MAX_LAYOUT_TOML_BYTES {
            return Err(format!("layout.toml exceeds maximum size of {MAX_LAYOUT_TOML_BYTES} bytes"));
        }
        let mut tree: Self = toml::from_str(src).map_err(|e| e.to_string())?;
        tree.validate()?;
        Ok(tree)
    }

    /// Enforce tree depth, node count, and finite clamped numeric bounds.
    pub fn validate(&mut self) -> Result<(), String> {
        let mut total = 0usize;
        validate_nodes(&mut self.shell, 1, &mut total)?;
        validate_nodes(&mut self.library, 1, &mut total)?;
        Ok(())
    }

    /// True if neither `shell` nor `library` defines any declarative nodes.
    pub fn is_empty(&self) -> bool {
        self.shell.is_empty() && self.library.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_excessive_depth_node_count_and_non_finite_floats() {
        // Deep nesting (> MAX_TREE_DEPTH) rejected.
        let mut node = Node::Box { style: NodeStyle::default(), children: Vec::new() };
        for _ in 0..(MAX_TREE_DEPTH + 2) {
            node = Node::Box { style: NodeStyle::default(), children: vec![node] };
        }
        let mut deep = LayoutTree { shell: vec![node], library: Vec::new() };
        assert!(deep.validate().is_err());

        // Excessive node count (> MAX_TREE_NODES) rejected.
        let many = (0..=MAX_TREE_NODES)
            .map(|_| Node::Wallet { style: NodeStyle::default() })
            .collect();
        let mut bloated = LayoutTree { shell: many, library: Vec::new() };
        assert!(bloated.validate().is_err());

        // Non-finite floats rejected; extreme finite values clamped.
        let mut nan_tree = LayoutTree {
            shell: vec![Node::Logo { size: f32::NAN, style: NodeStyle::default() }],
            library: Vec::new(),
        };
        assert!(nan_tree.validate().is_err());

        let mut huge_tree = LayoutTree {
            shell: vec![Node::Logo {
                size: 1e9,
                style: NodeStyle { font_size: Some(99999.0), ..Default::default() },
            }],
            library: Vec::new(),
        };
        huge_tree.validate().unwrap();
        match &huge_tree.shell[0] {
            Node::Logo { size, style } => {
                assert_eq!(*size, 512.0);
                assert_eq!(style.font_size, Some(128.0));
            }
            _ => panic!("expected Logo"),
        }
    }

    #[test]
    fn parses_submenu_items_and_rejects_excessive_nesting() {
        let valid = r#"
[[shell]]
type = "menu"
label = "View"
items = [
    { label = "Library", action = "navigate:library" },
    "separator",
    { label = "More", icon = "eye", items = [
        { label = "Servers", action = "navigate:servers" },
        { label = "Screenshots", action = "navigate:screenshots" },
    ] },
]
"#;
        let tree = LayoutTree::parse(valid).unwrap();
        assert_eq!(tree.shell.len(), 1);

        let deep = r#"
[[shell]]
type = "menu"
label = "Deep"
items = [{ label = "1", items = [{ label = "2", items = [{ label = "3", items = [{ label = "4", items = [{ label = "5", action = "x" }] }] }] }] }]
"#;
        assert!(LayoutTree::parse(deep).is_err());
    }
}

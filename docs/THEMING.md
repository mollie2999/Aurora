# Aurora Declarative Theme Authoring Guide

Aurora includes a **Declarative Theme Engine** inspired by Millennium and CSS box-model layouts. A theme is a self-contained folder (or `.zip` package) that can restyle colors and typography, completely rearrange the window chrome and library layout via `layout.toml`, expose user-configurable `[options.*]`, bundle custom fonts/backgrounds/logos, and draw custom ambient effects with sandboxed `theme.rhai` scripts.

Built-in theme packages are bundled with Aurora and automatically installed on launch (for both fresh installs and updates):

- [Aurora Desktop (Steam replica)](examples/steam-replica/README.md): Steam-style chrome and library shelves, with Modern, Classic Blue and VGUI Olive palettes.
- [Adwaita](examples/adwaita/README.md): compact AdwSteamGtk-inspired chrome around Aurora's default library, with optional native GTK4/libadwaita styling on Linux.
- [Metro](examples/metro/README.md): port of the MetroSteam skin, with a single flat black title bar, square corners and solid-accent selection, plus Variation and Accent Color options.
- [Aurora Square](examples/square/README.md): Aurora's default interface with sharp `0px` square corners and selectable built-in palettes, suited for compositors that apply native window rounding (e.g. KDE Plasma / KWin).

Open **Settings → Appearance** to activate either theme right away. If you uninstall a bundled theme and later want it back, click **Restore Built-in Themes** in **Settings → Appearance**.

To package a modified copy from a repository checkout as an installable ZIP (Python 3 is only needed to create the archive):

```sh
python3 -m zipfile -c steam-replica-theme.zip docs/examples/steam-replica
python3 -m zipfile -c adwaita-theme.zip docs/examples/adwaita
```

Install a custom ZIP through **Settings → Appearance → Install Theme…**, then activate it. Keep Adwaita's bundled fonts and license files in the package.

---

## Theme Package Structure

Aurora discovers installed themes inside your platform config directory:

| Platform | Themes Directory (`<config_dir>/themes/<id>/`) | Saved Theme Option Data |
| --- | --- | --- |
| **Linux** | `~/.config/steaminrust/themes/<id>/` | `~/.config/steaminrust/theme-data/<id>.json` |
| **macOS** | `~/Library/Application Support/dev.SteamInRust.SteamInRust/themes/<id>/` | `~/Library/Application Support/dev.SteamInRust.SteamInRust/theme-data/<id>.json` |
| **Windows** | `%APPDATA%\SteamInRust\SteamInRust\config\themes\<id>\` | `%APPDATA%\SteamInRust\SteamInRust\config\theme-data\<id>.json` |

Aurora retains the existing SteamInRust settings directory. On Linux, `XDG_CONFIG_HOME` overrides `~/.config` when set.

A theme package contains:

```text
my-theme/
├── theme.toml          # Required: metadata, color tokens, typography, assets, options
├── layout.toml         # Optional: declarative [[shell]] and [[library]] UI node trees
├── theme.rhai          # Optional: sandboxed visual hooks (gradients, borders, overlays)
├── preview.png         # Optional: preview image
└── assets/             # Optional: custom fonts, background image, or custom logo
    ├── background.png
    ├── logo.png
    └── fonts/
        ├── MyFont-Regular.ttf
        ├── MyFont-Bold.ttf
        └── MyMono-Regular.ttf
```

- If `layout.toml` is omitted (and no inline `[layout]` table is present in `theme.toml`), Aurora renders its built-in default shell and library using your theme's colors, fonts, and assets.
- All asset paths (`fonts.*`, `background.image`, `logo.image`, `layout_file`, `meta.preview`) are **relative to the theme folder**; absolute paths and `..` segments are rejected.

---

## `theme.toml` Reference

### `[meta]` (Required)

```toml
[meta]
id = "steam-replica"
name = "Steam Desktop"
version = "2.0.0"
author = "Aurora"
description = "Declarative replica of the Valve Steam desktop client."
preview = "preview.png"
min_aurora_version = "0.1.0"
```

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | `string` | **Yes** | Unique package id: 1–64 chars matching `[a-z0-9_-]+`. |
| `name` | `string` | **Yes** | Display name shown in Settings → Appearance (must not be empty). |
| `version` | `string` | No | Default: `"1.0.0"`. |
| `author` | `string` | No | Author name or handle. |
| `description` | `string` | No | Short description shown in the theme list. |
| `preview` | `string` | No | Relative path to a preview image. |
| `min_aurora_version` | `string` | No | Minimum recommended Aurora version. |

### `[native]` — Desktop appearance integration

Packages can opt into an Adwaita base profile:

```toml
[native]
provider = "adwaita"
color_scheme = "system" # "system", "light", or "dark"
accent = "system"       # or blue, teal, green, yellow, orange, red, pink, purple, slate
```

On Linux, an isolated helper loads the installed GTK4 and libadwaita (1.6 or newer) libraries. It reads real stylesheet colors, native accent conversions and button metrics, and renders GTK button backgrounds for normal, hover and pressed states. Shared secondary buttons use this artwork with preserved corners; egui retains text, focus and input handling. There is no Python dependency and no visible helper window.

A background worker follows GNOME's GSettings appearance, accent and high-contrast preferences and refreshes native style data when they change. Helper output is size-limited, the process has a two-second timeout, and drawing never waits for it. These updates work independently of theme file hot reload. Missing libraries/display and other platforms keep the portable Adwaita profile; missing desktop preferences fall back to light and blue. Explicit light/dark and accent choices work on every platform.

Resolution applies native settings (including `overrides.native` from selected options), then `[theme]`, then each option's token overrides. Package token overrides therefore remain authoritative. Avoid specifying `base`, `dark`, or fixed palette colors when they should come from the native profile.

For example, a manual appearance choice can preserve the system accent:

```toml
[[options.appearance.choices]]
value = "light"
label = "Light"
[options.appearance.choices.overrides.native]
color_scheme = "light"
```

Aurora keeps its egui renderer and layout. GTK interprets its own stylesheet in the helper; Aurora consumes its palette, metrics and rendered neutral button backgrounds. This does not embed an interactive GTK widget tree or execute theme-provided GTK CSS. The portable fallback follows [libadwaita's documented color roles](https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/css-variables.html). Bundled theme fonts remain in use. Run `aurora --adwaita-helper` to inspect the native snapshot; the normal log reports loaded library versions.

### `[theme]` — Palette & Typography Tokens

Colors accept hex strings (`#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`) or `[r, g, b]` / `[r, g, b, a]` byte arrays.

```toml
[theme]
base = "NEVKO"
dark = true

bg = "#232730"
chrome = "#171D25"
surface = "#2B303A"
surface_hover = "#343B48"
raised = "#2E333C"
border = "#14171D"

text = "#DCDEDF"
text_dim = "#8B929A"
text_faint = "#626972"

accent = "#1A9FFF"
accent_2 = "#00BBFF"
on_accent = "#FFFFFF"

play = "#59BF40"
online = "#57CBDE"
in_game = "#A4D007"
away = "#E8B04B"
danger = "#D94126"
discount = "#4C6B22"

radius = 2
weight_preset = "balanced"
font_scale = 0.96
```

| Field | Type / Values | Description |
| --- | --- | --- |
| `base` | `string` | Optional built-in preset to inherit before overrides (case-insensitive): `NEVKO`, `Midnight`, `Aurora`, `Classic`, `OLED`, `Daylight`. |
| `dark` | `bool` | Dark vs light egui baseline visuals. |
| `bg`, `chrome`, `surface`, `surface_hover`, `raised`, `border` | `color` | Core canvas, chrome bar, card, hover, input/popover, and border colors. |
| `text`, `text_dim`, `text_faint` | `color` | Primary, secondary, and muted text hierarchy. |
| `accent`, `accent_2`, `on_accent` | `color` | Brand highlight, secondary gradient stop, and foreground on accent fills. |
| `play`, `online`, `in_game`, `away`, `danger`, `discount` | `color` | Semantic status and action colors. |
| `radius` | `u8` | Default widget corner radius in logical pixels (`0`–`255`). |
| `weight_preset` | `"heavy"` \| `"balanced"` | `"heavy"` uses Aurora's bold Geist styling; `"balanced"` loads crisp **Inter Regular / Medium / SemiBold / Bold**. |
| `widget_style` | `"aurora"` \| `"adwaita"` | Control geometry and interaction styling, independently of palette; native Adwaita selects `"adwaita"` automatically. |
| `font_scale` | `f32` (`0.75`–`1.5`) | Global UI font size multiplier. |

### `[fonts]`, `[background]`, `[logo]`, and `layout_file`

```toml
# Optional: load a custom layout filename instead of the default "layout.toml"
layout_file = "layout.toml"

[fonts]
regular = "assets/fonts/Inter-Regular.ttf"
bold = "assets/fonts/Inter-Bold.ttf"
mono = "assets/fonts/JetBrainsMono-Regular.ttf"

[background]
image = "assets/wallpaper.png"
fit = "cover"          # "cover" | "contain" | "tile"
opacity = 0.25         # 0.0..=1.0
tint = "#171D2580"     # optional color tint

[logo]
image = "assets/logo.png"
```

Malformed custom font faces are ignored with a log warning, and Aurora uses its bundled fallback fonts.

---

## `layout.toml` — Declarative UI Tree Reference

`layout.toml` (or an inline `[layout]` table in `theme.toml`) defines two root node arrays:

- `[[shell]]` — The outer application chrome (top/bottom/left/right panels, menus, navigation tabs, status bar, friends drawer, and central `page_content`).
- `[[library]]` — The Library page layout (game-list sidebar, search/filters, and `library_home_or_detail` shelves).

Every node is a table with `type = "<node_type>"`, node-specific properties, an optional `style = { ... }` table, and (for containers) a `children = [...]` array (typically written with TOML `[[...children]]` array-of-tables syntax).

### Style Primitives

#### `ColorRef`

Used anywhere a layout style or node accepts a color:

| Syntax | Example | Meaning |
| --- | --- | --- |
| `"transparent"` | `"transparent"` | Fully transparent (`rgba(0,0,0,0)`). |
| Hex literal | `"#1A9FFF"`, `"#1A9FFFCC"`, `"#FFF"` | Explicit RGB/RGBA color. |
| Token reference | `"$chrome"`, `"$bg"`, `"$accent"` | Resolves to any of the 18 `[theme]` color tokens (`bg`, `chrome`, `surface`, `surface_hover`, `raised`, `border`, `text`, `text_dim`, `text_faint`, `accent`, `accent_2`, `on_accent`, `play`, `online`, `in_game`, `away`, `danger`, `discount`). |
| Token with alpha | `"$accent@0.2"`, `"$bg@0.85"` | Theme token multiplied by an alpha in `0.0..=1.0`. |

#### `Edges` (padding / margin)

Accepts CSS-style shorthands:
- Scalar: `padding = 8` → all four sides `8px`
- 2-element array: `padding = [4, 12]` → `[vertical, horizontal]`
- 4-element array: `padding = [2, 8, 0, 8]` → `[top, right, bottom, left]`

#### `SizeVal` (width / height)

- Explicit pixels: `width = 28`, `height = 38`
- Fill available space: `width = "fill"`
- Content-sized (default): `width = "auto"`

#### `NodeStyle` Box Model

Every node accepts an optional `style = { ... }` table with any subset of these fields:

| Category | Fields & Types |
| --- | --- |
| **Dimensions** | `width` (`SizeVal`), `height` (`SizeVal`), `min_width` (`f32`), `max_width` (`f32`), `min_height` (`f32`) |
| **Spacing** | `padding` (`Edges`), `margin` (`Edges`), `gap` (`f32`) |
| **Background** | `bg` (`ColorRef`), `bg_gradient` (`{ from, to, direction = "horizontal" \| "vertical" }`), `hover_bg` (`ColorRef`), `active_bg` (`ColorRef`) |
| **Borders & Radius** | `border`, `border_top`, `border_bottom`, `border_left`, `border_right` (each `{ width = 1.0, color = ColorRef }`), `radius` (`u8`) |
| **Typography** | `color` (`ColorRef`), `hover_color` (`ColorRef`), `active_color` (`ColorRef`), `font_size` (`f32`), `font_weight` (`"regular"` \| `"medium"` \| `"semibold"` \| `"bold"`), `uppercase` (`bool`) |
| **Alignment** | `align` (`"start"` \| `"center"` \| `"end"`) |

---

### Container Nodes

| `type` | Fields (with defaults) | Description |
| --- | --- | --- |
| `"panel"` | **`side`** (`"top"` \| `"bottom"` \| `"left"` \| `"right"` \| `"center"`), `size` (`f32?`), `resizable` (`false`), `drag_window` (`false`), `style`, `children` | Docked top-level region (`egui::Panel` / `CentralPanel`). Set `drag_window = true` on titlebar panels to allow dragging/double-click maximizing the window. |
| `"row"` | `justify` (`"start"` \| `"center"` \| `"end"` \| `"space_between"`), `align` (`"start"` \| `"center"` \| `"end"`), `wrap` (`false`), `style`, `children` | Horizontal flex-like layout container. Supports `spacer` with `flex = true` to push trailing items to the right edge. |
| `"col"` | `align` (`"start"` \| `"center"` \| `"end"`), `style`, `children` | Vertical stack container. |
| `"box"` | `style`, `children` | Styled block container with background, `bg_gradient`, borders, radius, and padding. |
| `"scroll"` | `direction` (`"horizontal"` \| `"vertical"`, default `"horizontal"`), `style`, `children` | Scrollable viewport container. |
| `"spacer"` | `size` (`f32?`), `flex` (`false`), `style` | Fixed-size gap or flexible expanding spacer (`flex = true`). |
| `"divider"` | `orientation` (`"horizontal"` \| `"vertical"`), `color` (`ColorRef?`), `thickness` (`1.0`), `style` | Hairline separator rule. |

---

### Shell Component Nodes

| `type` | Fields (with defaults) | Description |
| --- | --- | --- |
| `"logo"` | `size` (`20.0`), `style` | Renders custom `[logo].image` or Aurora's built-in logo. |
| `"text"` | **`value`** (`string`), `style` | Static or interpolated text label (see Interpolation Tokens below). |
| `"button"` | **`label`** (`string`), `icon` (`string?`), **`action`** (`string`), `style` | Clickable button dispatching an action string. |
| `"menu"` | **`label`** (`string`), `icon` (`string?`), `items` (`Vec<MenuItemSpec>`), `style` | Clickable dropdown menu. Each item in `items` is either `"separator"` or `{ label = "...", icon = "...", action = "..." }`. |
| `"nav_tabs"` | `direction` (`"horizontal"` \| `"vertical"`), `show_icons` (`true`), `uppercase` (`false`), `profile_as_username` (`false`), `indicator` (`"pill"` \| `"underline"` \| `"left_bar"` \| `"none"`), `indicator_height` (`2.5`), `tab_gap` (`12.0`), `style` | Renders Aurora's main navigation pages (Store, Library, Community, Profile, plus plugin pages). |
| `"back_forward"` | `size` (`28.0`), `style` | History back/forward navigation arrows. |
| `"window_controls"` | `variant` (`"windows"` \| `"macos"` \| `"minimal"`), `height` (`28.0`), `style` | Window minimize, maximize/restore, and close controls. |
| `"account"` | `show_avatar` (`true`), `show_name` (`true`), `show_wallet` (`false`), `show_caret` (`true`), `avatar_size` (`22.0`), `avatar_radius` (`u8?`), `action` (`string?`), `style` | User account pill with avatar and optional name/wallet. By default opens the account menu. Set `action = "navigate:profile"` for direct profile navigation; `avatar_radius` rounds the image independently of the outer button. |
| `"wallet"` | `style` | Standalone Steam wallet balance badge. |
| `"notifications"` | `size` (`28.0`), `highlight_unread_bg` (`ColorRef?`), `style` | Notifications bell button with optional highlight fill when unread notifications exist. |
| `"friends_button"` | `size` (`28.0`), `icon` (`string?`, defaults to friends), `style` | Quick button to open/toggle Friends & Chat. |
| `"downloads_bar"` | `width` (`f32?`), `style` | Live download status & progress bar. |
| `"friends_drawer"` | `width` (`280.0`), `style` | Docked Friends & Chat drawer panel. |
| `"page_content"` | `style` | Hosts the currently active page (Store, Library, Downloads, Settings, etc.). Place inside the center `panel`. |

---

### Library Component Nodes

| `type` | Fields (with defaults) | Description |
| --- | --- | --- |
| `"library_search"` | `placeholder` (`string?`), `show_filter_button` (`false`), `style` | Library search box with optional filter button. |
| `"library_filters"` | `variant` (`"default"` \| `"dropdown_icons"`), `style` | Category / Installed / Ready-to-Play filter controls. |
| `"library_list"` | `group_by` (`"installed"` or `"last_played"`, defaults to `"installed"`), `row_height` (`26.0`), `icon_size` (`18.0`), `group_headers` (`true`), `show_counts` (`true`), `style` | Scrollable sidebar list with icons and collapsible group headers/counts. Last-played mode shows Recent, calendar months, and Never played. Optional row styling: `style.active_bg` / `style.active_color` (selected row fill and text, default a faint accent tint), `style.hover_bg` and `style.radius` (default `2`). |
| `"whats_new_shelf"` | `title` (`string?`), `card_width` (`240.0`), `card_height` (`135.0`), `card_radius` (`0`), `date_above` (`false`), `max_items` (`5`), `gap` (`12.0`), `style` | Horizontal shelf of recent game news/updates. |
| `"recent_games_shelf"` | `title` (`string?`), `featured_first` (`false`), `featured_width` (`360.0`), `capsule_width` (`156.0`), `capsule_height` (`234.0`), `card_radius` (`0`), `gap` (`12.0`), `style` | Recently played games shelf. When `featured_first = true`, the first game renders as a wide 16:9 hero tile followed by vertical capsules. |
| `"all_games_grid"` | `title` (`string?`), `show_count` (`true`), `show_sort` (`true`), `card_aspect` (`"2:3"` \| `"capsule"` \| `"banner"` \| `"16:9"`), `card_width` (`156.0`), `card_height` (`234.0`), `card_radius` (`0`), `gap` (`12.0`), `style` | Responsive full-width game grid with optional count badge and sort dropdown. |
| `"library_home_or_detail"` | `style`, `children` | When a game is selected, renders Aurora's game detail page; otherwise renders `children` as the Library Home shelves view. |

---

### Action Strings & Text Interpolation

#### Action Strings (for `button` and `menu` items)

| Action | Behavior |
| --- | --- |
| `"navigate:<page>"` | Navigate to a page (`library`, `store`, `community`, `downloads`, `friends`, `settings`, `profile`). |
| `"navigate:<page>:<arg>"` | Navigate to a page with an argument (e.g. `"navigate:library:add"`). |
| `"add_game"` | Open the Add a Non-Steam Game dialog. |
| `"activate_product"` | Open the Activate a Product on Steam dialog. |
| `"open_friends"` | Open Friends & Chat. |
| `"toggle_friends_drawer"` | Toggle the docked Friends drawer. |
| `"back"` / `"forward"` | Navigate history back or forward. |
| `"check_updates"` | Check for Aurora updates. |
| `"logout"` | Sign out / switch account. |
| `"exit"` | Quit Aurora. |
| `"open_url:<url>"` | Open an external HTTPS URL in the browser. |

#### Text Interpolation Tokens (for `text` nodes)

Use curly-brace placeholders inside `value`:

- `{username}` — Signed-in persona name
- `{wallet}` — Formatted Steam wallet balance
- `{installed_count}` — Number of installed games
- `{total_games}` — Total games in library
- `{online_friends}` — Count of online friends
- `{playing_game}` — Currently running game title (if any)
- `{downloads_label}` — Current download status summary

---

## User-Configurable `[options.<key>]` & Alternate Layouts

Themes can expose Millennium-style settings in **Settings → Appearance**. Choices are saved per theme in `<config_dir>/theme-data/<id>.json`.

Supported `kind` values:

1. **`select`** — Dropdown with `[[options.<key>.choices]]`, each carrying `value`, `label`, and an optional `overrides` bundle (`overrides.native`, `overrides.theme`, `overrides.background`, and/or `overrides.layout_file`).
2. **`bool`** — Toggle switch with `on_overrides` and `off_overrides` bundles.
3. **`color`** — Color picker targeting any of the 18 `theme.<token>` colors or `background.tint`.
4. **`slider`** — Numeric slider (`min`, `max`, `step`) targeting `theme.radius`, `theme.font_scale`, or `background.opacity`.

### Switching Entire Layouts via `layout_file`

An option can even swap the active `layout.toml` file (for example, toggling between a top header and a left sidebar layout):

```toml
[options.layout_mode]
kind = "select"
label = "Layout Mode"
default = "classic"

[[options.layout_mode.choices]]
value = "classic"
label = "Classic Two-Row Header"
[options.layout_mode.choices.overrides]
layout_file = "layout.toml"

[[options.layout_mode.choices]]
value = "sidebar"
label = "Left Rail Sidebar"
[options.layout_mode.choices.overrides]
layout_file = "layout-sidebar.toml"
```

---

## `theme.rhai` Visual Hooks Guide

Place an optional `theme.rhai` next to `theme.toml` to paint custom ambient gradients, chrome hairlines, or card overlays.

### Sandbox Limits

`theme.rhai` runs in a strictly sandboxed Rhai engine with no filesystem, network, process, or `eval` access (`print`/`debug` are no-ops):

- **50,000** operations per hook call
- **16** call stack levels
- **8,192** byte max string size, **512** max array elements, **256** max map entries
- Runtime errors disable the script gracefully and display the error in Settings without crashing Aurora.

### Hook Functions

```rhai
fn paint_background(w, h, time, theme) { ... }
fn paint_chrome(w, h, time, theme) { ... }
fn paint_card_overlay(w, h, hovered, installed, theme) { ... }
```

### `theme` Map Keys

Every hook receives a read-only `theme` map containing:
`name`, `dark`, `radius`, `font_scale`, `bg`, `chrome`, `surface`, `surface_hover`, `raised`, `border`, `text`, `text_dim`, `text_faint`, `accent`, `accent_2`, `on_accent`, `play`, `online`, `in_game`, `away`, `danger`, `discount`.

### `DrawCmd` Reference

Return an array of maps (`[ #{ type: "...", ... } ]`). All coordinates are logical pixels relative to the target rectangle's top-left `(0, 0)`, and colors accept hex strings (`"#1A9FFF44"`) or `[r, g, b(, a)]` arrays.

| `type` | Fields (with defaults) | Description |
| --- | --- | --- |
| `"rect"` | `x` (`0`), `y` (`0`), `w` (`0`), `h` (`0`), `radius` (`0`), **`color`** | Filled rectangle. |
| `"rect_stroke"` | `x` (`0`), `y` (`0`), `w` (`0`), `h` (`0`), `radius` (`0`), `width` (`1`), **`color`** | Inside-stroked rectangle border. |
| `"gradient"` | `x` (`0`), `y` (`0`), `w` (`0`), `h` (`0`), **`from`**, **`to`**, `horizontal` (`false`) | Linear 2-stop gradient. |
| `"circle"` | `cx` (`0`), `cy` (`0`), `r` (`0`), **`color`** | Filled circle. |
| `"line"` | `x1` (`0`), `y1` (`0`), `x2` (`0`), `y2` (`0`), `width` (`1`), **`color`** | Line segment. |
| `"text"` | `x` (`0`), `y` (`0`), **`text`**, `size` (`14`, clamped `6`–`128`), **`color`** | Proportional text anchored top-left. |

---

## Packaging, Installing & Live Hot-Reload

### Packaging as `.zip`

Aurora's safe importer accepts both **flat** `.zip` archives (`theme.toml` at root) and **GitHub-style** archives (`my-theme-main/theme.toml` inside one top-level folder):

```bash
# Flat zip from inside your theme directory
zip -r ../my-theme.zip theme.toml layout.toml theme.rhai assets/
```

Importer safety checks run before extraction:
- Validates `theme.toml` and `layout.toml` before atomically installing to `<config_dir>/themes/<id>/`.
- Enforces **500 files** and **50 MiB** uncompressed limits, skips symlinks, and rejects absolute or `..` traversal paths.

### Installing & Developing with Hot-Reload

1. Open **Settings → Appearance** in Aurora.
2. Click **Install Theme…** and select your `.zip` (or `.aurora-theme`) archive — Aurora installs and activates it immediately.
3. Click **Open Themes Folder** and toggle **Hot-Reload** on while editing `theme.toml`, `layout.toml`, or `theme.rhai` — saving any file reloads the theme live within half a second.

---

## Complete Example

See [`docs/examples/steam-replica/`](examples/steam-replica/theme.toml) for a complete, production-ready Steam Desktop theme using `theme.toml`, `layout.toml`, `theme.rhai`, and `[options.steam_era]`.

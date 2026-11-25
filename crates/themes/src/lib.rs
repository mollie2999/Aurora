//! # Aurora Declarative Theme Engine
//!
//! Manages theme packages (`<config_dir>/themes/<id>/`), CSS-like box-model
//! declarative UI trees (`layout.toml`), per-theme user option overrides
//! (`<config_dir>/theme-data/<id>.json`), safe `.zip`/folder installation,
//! and optional sandboxed `theme.rhai` visual hooks.

pub mod adwaita;
pub mod bundled;
pub mod color;
pub mod install;
pub mod manifest;
pub mod native;
pub mod script;
pub mod tree;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::Color32;

pub use bundled::{
    BUNDLED_THEMES, BundledFile, BundledSyncReport, BundledTheme, is_bundled_theme,
    missing_bundled_themes, restore_missing_bundled_themes, sync_bundled_themes,
};
pub use color::{ColorRef, ColorToken, ThemeColor, color_to_hex, parse_hex_color};
pub use install::{install_folder, install_zip, uninstall_theme};
pub use manifest::{
    BackgroundConfig, BackgroundFit, BackgroundOverrides, FontsConfig, LogoConfig, OptionChoice,
    OptionKind, OptionValue, OverrideBundle, ResolvedManifestParts, ThemeManifest, ThemeMeta,
    ThemeOption, ThemeOverrides, safe_rel_path, validate_id,
};
pub use script::{DrawCmd, ThemeScript, paint_draw_cmds, theme_to_rhai_map};
pub use tree::{
    AlignMode, BorderEdge, Direction, Edges, FontWeight, GradientSpec, IndicatorStyle, Justify,
    LayoutTree, LibraryGrouping, MenuItemSpec, Node, NodeStyle, PanelSide, SizeVal, WinCtrlVariant,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("zip archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid theme id '{0}' (must match [a-z0-9_-]+)")]
    InvalidId(String),
    #[error("invalid color '{0}'")]
    InvalidColor(String),
    #[error("invalid theme manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid layout tree: {0}")]
    InvalidLayout(String),
    #[error("missing theme.toml in package")]
    MissingManifest,
    #[error("unsafe path in theme package: '{0}'")]
    UnsafePath(String),
    #[error("theme package limit exceeded: {0}")]
    PackageLimit(String),
    #[error("theme '{0}' not found")]
    NotFound(String),
    #[error("option '{key}' not found on theme '{theme_id}'")]
    OptionNotFound { theme_id: String, key: String },
    #[error("invalid option value '{value}' (expected {expected})")]
    InvalidOptionValue { expected: &'static str, value: String },
}

/// A discovered, valid theme package on disk.
#[derive(Debug, Clone)]
pub struct InstalledTheme {
    pub dir: PathBuf,
    pub manifest: ThemeManifest,
    pub has_layout: bool,
    pub has_script: bool,
}

/// The active theme fully resolved with user option overrides, declarative
/// layout tree, custom font bytes, background/logo assets, and optional Rhai script.
#[derive(Debug)]
pub struct ResolvedTheme {
    pub meta: ThemeMeta,
    pub dir: PathBuf,
    pub theme: ui::Theme,
    pub layout: LayoutTree,
    pub custom_fonts: Option<ui::CustomFonts>,
    pub background_path: Option<PathBuf>,
    pub background_fit: BackgroundFit,
    pub background_opacity: f32,
    pub background_tint: Option<Color32>,
    pub logo_path: Option<PathBuf>,
    pub script: Option<ThemeScript>,
    pub script_error: Option<String>,
}

/// Discovers, activates, resolves, and hot-reloads declarative themes.
#[derive(Debug)]
pub struct ThemeManager {
    pub themes_dir: PathBuf,
    pub data_dir: PathBuf,
    pub installed: Vec<InstalledTheme>,
    pub broken: Vec<(PathBuf, String)>,
    pub active_id: Option<String>,
    pub resolved: Option<ResolvedTheme>,
    active_layout_file: Option<String>,
    last_mtime: Option<SystemTime>,
    native_appearance: native::NativeAppearance,
    active_option_values: BTreeMap<String, OptionValue>,
}

impl ThemeManager {
    pub fn new(themes_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            themes_dir,
            data_dir,
            installed: Vec::new(),
            broken: Vec::new(),
            active_id: None,
            resolved: None,
            active_layout_file: None,
            last_mtime: None,
            native_appearance: native::NativeAppearance::default(),
            active_option_values: BTreeMap::new(),
        }
    }

    /// Ensure built-in themes bundled with Aurora are installed in `themes_dir`
    /// and upgraded when Aurora ships newer versions.
    pub fn sync_bundled_themes(&self) -> Result<BundledSyncReport, Error> {
        bundled::sync_bundled_themes(&self.themes_dir, &self.data_dir)
    }

    /// Uninstall a theme by id. If it is a bundled theme, marks it dismissed so
    /// subsequent startups do not silently re-install it.
    pub fn uninstall(&mut self, id: &str) -> Result<(), Error> {
        uninstall_theme(&self.themes_dir, id)?;
        bundled::mark_dismissed(&self.data_dir, id)?;
        if self.active_id.as_deref() == Some(id) {
            let _ = self.activate(None);
        }
        Ok(())
    }

    /// Clear any bundled-theme dismissal flag after a manual install.
    pub fn note_theme_installed(&self, id: &str) {
        if let Err(e) = bundled::clear_dismissed(&self.data_dir, id) {
            tracing::warn!("clear dismissed state for theme '{id}': {e}");
        }
    }

    /// Return the IDs of bundled themes currently missing from `themes_dir`.
    pub fn missing_bundled_themes(&self) -> Vec<&'static str> {
        bundled::missing_bundled_themes(&self.themes_dir)
    }

    /// Restore any missing bundled themes to `themes_dir` and rescan.
    pub fn restore_bundled_themes(&mut self) -> Result<Vec<&'static str>, Error> {
        let restored = bundled::restore_missing_bundled_themes(&self.themes_dir, &self.data_dir)?;
        self.scan();
        Ok(restored)
    }

    /// Scan `themes_dir` for installed themes, recording any broken folders in
    /// `self.broken`. If an active theme is set, re-resolves it.
    pub fn scan(&mut self) {
        self.installed.clear();
        self.broken.clear();

        let Ok(entries) = fs::read_dir(&self.themes_dir) else {
            self.resolved = None;
            self.active_option_values.clear();
            return;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            let manifest_path = path.join("theme.toml");
            if !manifest_path.is_file() {
                continue;
            }
            match read_bounded_text(&manifest_path, tree::MAX_LAYOUT_TOML_BYTES as u64)
                .and_then(|s| ThemeManifest::parse(&s))
            {
                Ok(manifest) => {
                    let layout_rel = manifest.layout_file.as_deref().unwrap_or("layout.toml");
                    let has_layout_file = safe_rel_path(&path, layout_rel).is_some_and(|p| p.is_file());
                    let has_inline_layout = manifest.layout.as_ref().is_some_and(|l| !l.is_empty());
                    let has_script = path.join("theme.rhai").is_file();
                    self.installed.push(InstalledTheme {
                        dir: path,
                        manifest,
                        has_layout: has_layout_file || has_inline_layout,
                        has_script,
                    });
                }
                Err(e) => {
                    self.broken.push((path, e.to_string()));
                }
            }
        }

        self.installed.sort_by(|a, b| {
            a.manifest
                .meta
                .name
                .to_lowercase()
                .cmp(&b.manifest.meta.name.to_lowercase())
        });

        if let Some(id) = self.active_id.clone() {
            if self.resolve_active(&id).is_err() {
                self.active_id = None;
                self.resolved = None;
                self.active_layout_file = None;
                self.last_mtime = None;
                self.active_option_values.clear();
            }
        }
    }

    /// Activate a theme by id, or pass `None` to return to Aurora's default UI.
    pub fn activate(&mut self, id: Option<&str>) -> Result<(), Error> {
        match id {
            None => {
                self.active_id = None;
                self.resolved = None;
                self.active_layout_file = None;
                self.last_mtime = None;
                self.active_option_values.clear();
                Ok(())
            }
            Some(theme_id) => {
                self.resolve_active(theme_id)?;
                self.active_id = Some(theme_id.to_string());
                Ok(())
            }
        }
    }

    pub fn active_manifest(&self) -> Option<&ThemeManifest> {
        let id = self.active_id.as_deref()?;
        self.installed
            .iter()
            .find(|t| t.manifest.meta.id == id)
            .map(|t| &t.manifest)
    }

    pub fn uses_native_appearance(&self) -> bool {
        self.active_manifest().is_some_and(|manifest| manifest.native.is_some())
    }

    /// Update palette tokens from cached manifest/options only. Assets and file
    /// reload timestamps stay untouched; desktop changes never read packages.
    pub fn set_native_appearance(&mut self, appearance: native::NativeAppearance) -> bool {
        if self.native_appearance == appearance { return false; }
        self.native_appearance = appearance;
        let Some(manifest) = self.active_manifest().filter(|manifest| manifest.native.is_some()) else { return false; };
        let theme = manifest.resolve_tokens_with_native(&self.active_option_values, appearance).theme;
        let Some(resolved) = self.resolved.as_mut() else { return false; };
        if resolved.theme == theme { return false; }
        resolved.theme = theme;
        true
    }

    pub fn get(&self, id: &str) -> Option<&InstalledTheme> {
        self.installed.iter().find(|t| t.manifest.meta.id == id)
    }

    pub fn load_option_values(&self, theme_id: &str) -> BTreeMap<String, OptionValue> {
        if validate_id(theme_id).is_err() {
            return BTreeMap::new();
        }
        let path = self.data_dir.join(format!("{theme_id}.json"));
        let Ok(raw) = fs::read_to_string(&path) else {
            return BTreeMap::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save_option_values(
        &self,
        theme_id: &str,
        values: &BTreeMap<String, OptionValue>,
    ) -> Result<(), Error> {
        validate_id(theme_id)?;
        fs::create_dir_all(&self.data_dir)?;
        let path = self.data_dir.join(format!("{theme_id}.json"));
        let content = serde_json::to_string_pretty(values)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn set_option(
        &mut self,
        theme_id: &str,
        key: &str,
        value: OptionValue,
    ) -> Result<(), Error> {
        let installed = self
            .get(theme_id)
            .ok_or_else(|| Error::NotFound(theme_id.to_string()))?;
        if !installed.manifest.options.contains_key(key) {
            return Err(Error::OptionNotFound {
                theme_id: theme_id.to_string(),
                key: key.to_string(),
            });
        }
        let mut values = self.load_option_values(theme_id);
        values.insert(key.to_string(), value);
        self.save_option_values(theme_id, &values)?;
        if self.active_id.as_deref() == Some(theme_id) {
            self.resolve_active(theme_id)?;
        }
        Ok(())
    }

    pub fn set_option_str(
        &mut self,
        theme_id: &str,
        key: &str,
        raw_value: &str,
    ) -> Result<(), Error> {
        let kind = self
            .get(theme_id)
            .ok_or_else(|| Error::NotFound(theme_id.to_string()))?
            .manifest
            .options
            .get(key)
            .ok_or_else(|| Error::OptionNotFound {
                theme_id: theme_id.to_string(),
                key: key.to_string(),
            })?
            .kind;
        let parsed = OptionValue::parse_for_kind(kind, raw_value)?;
        self.set_option(theme_id, key, parsed)
    }

    pub fn reset_options(&mut self, theme_id: &str) -> Result<(), Error> {
        validate_id(theme_id)?;
        let path = self.data_dir.join(format!("{theme_id}.json"));
        if path.is_file() {
            fs::remove_file(path)?;
        }
        if self.active_id.as_deref() == Some(theme_id) {
            self.resolve_active(theme_id)?;
        }
        Ok(())
    }

    /// Check whether the active theme's `theme.toml`, active layout file, or
    /// `theme.rhai` changed on disk; if so, reload and return `true`.
    pub fn check_hot_reload(&mut self) -> bool {
        if self.active_id.is_none() {
            return false;
        }
        let Some(dir) = self.resolved.as_ref().map(|r| r.dir.clone()) else {
            return false;
        };
        let current = theme_dir_mtime(&dir, self.active_layout_file.as_deref());
        if current.is_some() && current != self.last_mtime {
            self.scan();
            return true;
        }
        false
    }

    fn resolve_active(&mut self, theme_id: &str) -> Result<(), Error> {
        let installed = self
            .installed
            .iter()
            .find(|t| t.manifest.meta.id == theme_id)
            .ok_or_else(|| Error::NotFound(theme_id.to_string()))?
            .clone();

        let user_options = self.load_option_values(theme_id);
        let parts = installed.manifest.resolve_tokens_with_native(&user_options, self.native_appearance);

        // Load declarative LayoutTree:
        // 1. Explicit `layout_file` (from manifest or option override), else
        // 2. `layout.toml` in theme folder if present, else
        // 3. inline `[layout]` in `theme.toml`, else empty `LayoutTree::default()`.
        let layout_rel = parts.layout_file.as_deref().unwrap_or("layout.toml");
        let layout = if let Some(path) =
            safe_rel_path(&installed.dir, layout_rel).filter(|p| p.is_file())
        {
            let src = read_bounded_text(&path, tree::MAX_LAYOUT_TOML_BYTES as u64)?;
            LayoutTree::parse(&src).map_err(Error::InvalidLayout)?
        } else if let Some(mut inline) = installed.manifest.layout.clone() {
            inline.validate().map_err(Error::InvalidLayout)?;
            inline
        } else {
            LayoutTree::default()
        };

        let custom_fonts = load_custom_fonts(&installed.dir, &installed.manifest.fonts);
        let background_path = parts
            .background
            .image
            .as_deref()
            .and_then(|rel| safe_rel_path(&installed.dir, rel))
            .filter(|p| p.is_file());
        let logo_path = installed
            .manifest
            .logo
            .image
            .as_deref()
            .and_then(|rel| safe_rel_path(&installed.dir, rel))
            .filter(|p| p.is_file());

        let script_path = installed.dir.join("theme.rhai");
        let (script, script_error) = if script_path.is_file() {
            match ThemeScript::from_file(&script_path) {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };

        self.active_layout_file = Some(layout_rel.to_string());
        self.last_mtime = theme_dir_mtime(&installed.dir, Some(layout_rel));
        self.active_option_values = user_options;
        self.resolved = Some(ResolvedTheme {
            meta: installed.manifest.meta,
            dir: installed.dir,
            theme: parts.theme,
            layout,
            custom_fonts,
            background_path,
            background_fit: parts.background.fit,
            background_opacity: parts.background.opacity,
            background_tint: parts.background.tint.map(|c| c.0),
            logo_path,
            script,
            script_error,
        });
        Ok(())
    }
}

/// Maximum size allowed for a single custom theme font file (16 MiB).
pub const MAX_FONT_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn open_regular_no_symlinks(path: &Path) -> Result<fs::File, Error> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::ffi::OsStrExt;
        use std::path::Component;

        let mut comps: Vec<Component<'_>> = Vec::new();
        let mut is_abs = false;
        for c in path.components() {
            match c {
                Component::RootDir => is_abs = true,
                Component::CurDir => {}
                Component::ParentDir | Component::Normal(_) => comps.push(c),
                Component::Prefix(_) => return Err(Error::UnsafePath(path.display().to_string())),
            }
        }
        if comps.is_empty() {
            return Err(Error::UnsafePath(path.display().to_string()));
        }
        let start = if is_abs { c"/" } else { c"." };
        let fd = unsafe { libc::open(start.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        let mut dirfd = unsafe { OwnedFd::from_raw_fd(fd) };
        for (idx, comp) in comps.iter().enumerate() {
            let is_last = idx + 1 == comps.len();
            let cseg = match comp {
                Component::ParentDir => CString::new("..").unwrap(),
                Component::Normal(s) => CString::new(s.as_bytes()).map_err(|_| Error::UnsafePath(path.display().to_string()))?,
                _ => continue,
            };
            let flags = if is_last {
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
            } else {
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
            };
            let next = unsafe { libc::openat(dirfd.as_raw_fd(), cseg.as_ptr(), flags) };
            if next < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ELOOP) {
                    return Err(Error::UnsafePath(path.display().to_string()));
                }
                return Err(Error::Io(err));
            }
            let owned = unsafe { OwnedFd::from_raw_fd(next) };
            if is_last {
                return Ok(fs::File::from(owned));
            }
            dirfd = owned;
        }
        Err(Error::UnsafePath(path.display().to_string()))
    }
    #[cfg(not(unix))]
    {
        let pre = fs::symlink_metadata(path)?;
        if pre.file_type().is_symlink() || !pre.is_file() {
            return Err(Error::UnsafePath(path.display().to_string()));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if (pre.file_attributes() & 0x0400) != 0 {
                return Err(Error::UnsafePath(path.display().to_string()));
            }
        }
        let mut opts = fs::OpenOptions::new();
        opts.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.custom_flags(0x0020_0000);
        }
        Ok(opts.open(path)?)
    }
}

pub(crate) fn read_bounded_bytes(path: &Path, max_bytes: u64) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    let file = open_regular_no_symlinks(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::UnsafePath(path.display().to_string()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if (meta.file_attributes() & 0x0400) != 0 {
            return Err(Error::UnsafePath(path.display().to_string()));
        }
    }
    if meta.len() > max_bytes {
        return Err(Error::PackageLimit(format!(
            "file '{}' exceeds maximum size of {max_bytes} bytes",
            path.display()
        )));
    }
    let mut buf = Vec::new();
    file.take(max_bytes.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > max_bytes {
        return Err(Error::PackageLimit(format!(
            "file '{}' exceeds maximum size of {max_bytes} bytes",
            path.display()
        )));
    }
    Ok(buf)
}

pub(crate) fn read_bounded_text(path: &Path, max_bytes: u64) -> Result<String, Error> {
    let bytes = read_bounded_bytes(path, max_bytes)?;
    String::from_utf8(bytes).map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
}

fn load_custom_fonts(dir: &Path, cfg: &FontsConfig) -> Option<ui::CustomFonts> {
    let load = |rel: &Option<String>| -> Option<Vec<u8>> {
        let p = safe_rel_path(dir, rel.as_deref()?)?;
        read_bounded_bytes(&p, MAX_FONT_BYTES).ok()
    };
    let regular = load(&cfg.regular);
    let bold = load(&cfg.bold);
    let mono = load(&cfg.mono);
    if regular.is_none() && bold.is_none() && mono.is_none() {
        None
    } else {
        Some(ui::CustomFonts { regular, bold, mono })
    }
}

fn theme_dir_mtime(dir: &Path, layout_file: Option<&str>) -> Option<SystemTime> {
    let mut latest: Option<SystemTime> = None;
    let mut check = |p: PathBuf| {
        if let Ok(meta) = fs::metadata(p) {
            if let Ok(mtime) = meta.modified() {
                latest = Some(match latest {
                    Some(prev) => prev.max(mtime),
                    None => mtime,
                });
            }
        }
    };
    check(dir.join("theme.toml"));
    check(dir.join("theme.rhai"));
    if let Some(lf) = layout_file.and_then(|rel| safe_rel_path(dir, rel)) {
        check(lf);
    } else {
        check(dir.join("layout.toml"));
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn native_adwaita_light_manifest_uses_adwaita_window_palette() {
        let manifest = ThemeManifest::parse("[meta]\nid = 'native-test'\nname = 'Native test'\n[native]\nprovider = 'adwaita'\ncolor_scheme = 'light'\naccent = 'red'\n").unwrap();
        let resolved = manifest.resolve_tokens(&BTreeMap::new());
        assert!(!resolved.theme.dark);
        assert_eq!(resolved.theme.bg, Color32::from_rgb(0xfa, 0xfa, 0xfb));
        assert_eq!(resolved.theme.accent, Color32::from_rgb(0xe6, 0x2d, 0x42));
    }

    #[test]
    fn native_package_options_keep_manual_and_system_preferences_independent() {
        use native::NativeAppearance;
        let manifest = ThemeManifest::parse(include_str!("../../../docs/examples/adwaita/theme.toml")).unwrap();
        let desktop = NativeAppearance { dark: Some(false), accent: Some(Color32::from_rgb(0x91, 0x41, 0xac)), high_contrast: false, ..Default::default() };
        let mut options = BTreeMap::new();
        let system = manifest.resolve_tokens_with_native(&options, desktop);
        assert!(!system.theme.dark);
        assert_eq!(system.theme.accent, Color32::from_rgb(0x91, 0x41, 0xac));
        options.insert("appearance".into(), OptionValue::Text("dark".into()));
        let manual_dark = manifest.resolve_tokens_with_native(&options, desktop);
        assert!(manual_dark.theme.dark);
        assert_eq!(manual_dark.theme.accent, Color32::from_rgb(0x91, 0x41, 0xac));
        options.insert("accent".into(), OptionValue::Text("yellow".into()));
        options.insert("appearance".into(), OptionValue::Text("system".into()));
        let manual_accent = manifest.resolve_tokens_with_native(&options, desktop);
        assert!(!manual_accent.theme.dark);
        assert_eq!(manual_accent.theme.accent, Color32::from_rgb(0xc8, 0x88, 0x00));
        assert_eq!(manual_accent.theme.on_accent, Color32::WHITE);
    }

    #[test]
    fn native_fallback_high_contrast_and_explicit_tokens_resolve_in_order() {
        let manifest = ThemeManifest::parse("[meta]\nid = 'native-test'\nname = 'Native test'\n[native]\nprovider = 'adwaita'\n[theme]\nbg = '#123456'\n").unwrap();
        let fallback = manifest.resolve_tokens(&BTreeMap::new());
        assert!(!fallback.theme.dark);
        assert_eq!(fallback.theme.accent, Color32::from_rgb(0x35, 0x84, 0xe4));
        assert_eq!(fallback.theme.bg, Color32::from_rgb(0x12, 0x34, 0x56));
        let contrast = manifest.resolve_tokens_with_native(&BTreeMap::new(), native::NativeAppearance { dark: Some(false), high_contrast: true, ..Default::default() });
        assert_eq!(contrast.theme.text_dim, Color32::BLACK);
        assert_eq!(contrast.theme.border, Color32::BLACK);
        assert_eq!(contrast.theme.bg, Color32::from_rgb(0x12, 0x34, 0x56));
        let legacy = ThemeManifest::parse("[meta]\nid = 'legacy-test'\nname = 'Legacy test'\n[theme]\naccent = '#abcdef'\n").unwrap();
        assert_eq!(legacy.resolve_tokens(&BTreeMap::new()).theme, legacy.resolve_tokens_with_native(&BTreeMap::new(), native::NativeAppearance { dark: Some(false), high_contrast: true, ..Default::default() }).theme);
    }

    #[test]
    fn adwaita_package_installs_and_native_changes_update_only_active_native_theme() {
        let root = temp_dir("native-adwaita");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/adwaita");
        let installed = root.join("themes");
        install_folder(&source, &installed).unwrap();
        let mut manager = ThemeManager::new(installed, root.join("options"));
        manager.scan();
        assert!(!manager.uses_native_appearance());
        assert!(!manager.set_native_appearance(native::NativeAppearance { dark: Some(false), ..Default::default() }));
        manager.activate(Some("adwaita")).unwrap();
        assert!(manager.uses_native_appearance());
        let resolved = manager.resolved.as_ref().unwrap();
        assert!(!resolved.theme.dark);
        assert!(resolved.custom_fonts.is_some());
        assert!(resolved.layout.shell.iter().any(|node| matches!(node, Node::Panel { side: PanelSide::Top, size: Some(44.0), .. })), "Adwaita should provide its compact header");
        assert!(resolved.layout.shell.iter().any(|node| matches!(node, Node::Panel { side: PanelSide::Bottom, size: Some(28.0), .. })), "Adwaita should provide its compact footer");
        assert!(resolved.layout.library.is_empty(), "Adwaita should retain Aurora's default library");
        assert!(manager.set_native_appearance(native::NativeAppearance { dark: Some(true), ..Default::default() }));
        assert!(manager.resolved.as_ref().unwrap().theme.dark);
        assert!(!manager.set_native_appearance(native::NativeAppearance { dark: Some(true), ..Default::default() }));
        manager.set_option("adwaita", "appearance", OptionValue::Text("light".into())).unwrap();
        manager.set_option("adwaita", "accent", OptionValue::Text("red".into())).unwrap();
        assert!(!manager.set_native_appearance(native::NativeAppearance { dark: Some(false), accent: Some(Color32::GREEN), high_contrast: false, ..Default::default() }));
        assert!(!manager.resolved.as_ref().unwrap().theme.dark);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn account_profile_action_is_opt_in_and_rounding_round_trips() {
        let legacy: Node = serde_json::from_value(serde_json::json!({"type": "account"})).unwrap();
        assert!(matches!(legacy, Node::Account { action: None, avatar_radius: None, .. }));
        let configured: Node = serde_json::from_value(serde_json::json!({
            "type": "account", "action": "navigate:profile", "avatar_radius": 6,
        })).unwrap();
        let saved = serde_json::to_value(&configured).unwrap();
        assert_eq!(saved["action"], "navigate:profile");
        assert_eq!(saved["avatar_radius"], 6);
    }

    #[test]
    fn native_desktop_updates_leave_layout_and_hot_reload_state_untouched() {
        let root = temp_dir("native-layout-reload");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/adwaita");
        let installed = root.join("themes");
        install_folder(&source, &installed).unwrap();
        // Test hot reload independently of the shipped package's default layout.
        fs::write(installed.join("adwaita/layout.toml"), "[[shell]]\ntype = 'panel'\nside = 'center'\n").unwrap();
        let mut manager = ThemeManager::new(installed.clone(), root.join("options"));
        manager.scan();
        manager.activate(Some("adwaita")).unwrap();
        let shell_count = manager.resolved.as_ref().unwrap().layout.shell.len();
        let previous_mtime = manager.last_mtime;
        fs::write(installed.join("adwaita/layout.toml"), "broken = [").unwrap();
        assert!(manager.set_native_appearance(native::NativeAppearance { dark: Some(true), ..Default::default() }));
        assert!(manager.resolved.as_ref().unwrap().theme.dark);
        assert_eq!(manager.resolved.as_ref().unwrap().layout.shell.len(), shell_count);
        assert_eq!(manager.last_mtime, previous_mtime);

        fs::write(installed.join("adwaita/layout.toml"), "").unwrap();
        // Ensure a file reload is pending regardless of filesystem timestamp precision.
        manager.last_mtime = Some(SystemTime::UNIX_EPOCH);
        let accent = Color32::from_rgb(0x91, 0x41, 0xac);
        assert!(manager.set_native_appearance(native::NativeAppearance { dark: Some(true), accent: Some(accent), high_contrast: false, ..Default::default() }));
        assert_eq!(manager.resolved.as_ref().unwrap().theme.accent, accent);
        assert_eq!(manager.resolved.as_ref().unwrap().layout.shell.len(), shell_count);
        assert_eq!(manager.last_mtime, Some(SystemTime::UNIX_EPOCH));
        assert!(manager.check_hot_reload());
        assert!(manager.resolved.as_ref().unwrap().layout.shell.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aurora-declarative-themes-test-{}-{}-{n}",
            std::process::id(),
            label
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn library_grouping_is_opt_in_and_rejects_unknown_values() {
        let old = LayoutTree::parse("[[library]]\ntype = 'library_list'").unwrap();
        assert!(matches!(old.library[0], Node::LibraryList { group_by: LibraryGrouping::Installed, .. }));
        let recent = LayoutTree::parse("[[library]]\ntype = 'library_list'\ngroup_by = 'last_played'").unwrap();
        assert!(matches!(recent.library[0], Node::LibraryList { group_by: LibraryGrouping::LastPlayed, .. }));
        assert!(LayoutTree::parse("[[library]]\ntype = 'library_list'\ngroup_by = 'typo'").is_err());
    }

    #[test]
    fn steam_replica_package_installs_and_resolves_every_era() {
        let root = temp_dir("steam-replica");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/steam-replica");
        let installed = root.join("themes");
        install_folder(&source, &installed).expect("install the shipped theme package");
        let mut manager = ThemeManager::new(installed, root.join("options"));
        manager.scan();
        manager.activate(Some("steam-replica")).expect("activate installed replica");
        for (era, chrome) in [
            ("modern_2023", Color32::from_rgb(0x17, 0x1D, 0x25)),
            ("classic_2018", Color32::from_rgb(0x17, 0x1A, 0x21)),
            ("vgui_2010", Color32::from_rgb(0x1E, 0x20, 0x1D)),
        ] {
            manager.set_option("steam-replica", "steam_era", OptionValue::Text(era.into())).unwrap();
            let resolved = manager.resolved.as_mut().unwrap();
            assert_eq!(resolved.theme.chrome, chrome, "palette for {era}");
            assert!(!resolved.layout.shell.is_empty());
            assert!(!resolved.layout.library.is_empty());
            assert!(resolved.script_error.is_none());
            let script = resolved.script.as_mut().expect("visual hooks");
            assert!(!script.paint_background(1280.0, 800.0, 0.0, &resolved.theme).is_empty());
            assert!(script.error().is_none());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn color_ref_edges_and_size_val_deserialize() {
        #[derive(serde::Deserialize)]
        struct BoxSample {
            c_hex: ColorRef,
            c_tok: ColorRef,
            c_alpha: ColorRef,
            c_trans: ColorRef,
            e_scalar: Edges,
            e_pair: Edges,
            e_quad: Edges,
            s_px: SizeVal,
            s_fill: SizeVal,
            s_auto: SizeVal,
        }

        let src = r##"
c_hex = "#1A9FFF"
c_tok = "$accent"
c_alpha = "$accent@0.2"
c_trans = "transparent"
e_scalar = 8
e_pair = [6, 12]
e_quad = [1, 2, 3, 4]
s_px = 240
s_fill = "fill"
s_auto = "auto"
"##;
        let sample: BoxSample = toml::from_str(src).expect("parse BoxSample");
        let theme = ui::Theme::nevko();

        assert_eq!(sample.c_hex.resolve(&theme), Color32::from_rgb(0x1A, 0x9F, 0xFF));
        assert_eq!(sample.c_tok.resolve(&theme), theme.accent);
        assert_eq!(sample.c_alpha.resolve(&theme), theme.accent.gamma_multiply(0.2));
        assert_eq!(sample.c_trans.resolve(&theme), Color32::TRANSPARENT);

        assert_eq!(sample.e_scalar, Edges::all(8.0));
        assert_eq!(sample.e_pair, Edges::symmetric(6.0, 12.0));
        assert_eq!(sample.e_quad, Edges { top: 1.0, right: 2.0, bottom: 3.0, left: 4.0 });
        assert_eq!(sample.e_quad.to_egui_margin(), egui::Margin { left: 4, right: 2, top: 1, bottom: 3 });

        assert_eq!(sample.s_px, SizeVal::Px(240.0));
        assert_eq!(sample.s_fill, SizeVal::Fill);
        assert_eq!(sample.s_auto, SizeVal::Auto);
    }

    #[test]
    fn layout_tree_parses_two_row_steam_shell_and_library() {
        let layout_toml = r##"
[[shell]]
type = "panel"
side = "top"
size = 66
drag_window = true
style = { bg = "$chrome", border_bottom = { width = 1, color = "$border" }, padding = [0, 8] }

  [[shell.children]]
  type = "col"
  style = { gap = 0 }

    [[shell.children.children]]
    type = "row"
    justify = "space_between"
    align = "center"
    style = { height = 28 }

      [[shell.children.children.children]]
      type = "row"
      align = "center"
      style = { gap = 4 }

        [[shell.children.children.children.children]]
        type = "logo"
        size = 16

        [[shell.children.children.children.children]]
        type = "menu"
        label = "Steam"
        items = [
          { label = "Settings", action = "nav:settings" },
          "separator",
          { label = "Exit", action = "quit" },
        ]

      [[shell.children.children.children]]
      type = "row"
      align = "center"
      style = { gap = 6 }

        [[shell.children.children.children.children]]
        type = "notifications"
        size = 24
        highlight_unread_bg = "$play"

        [[shell.children.children.children.children]]
        type = "account"
        show_avatar = true
        show_name = true
        show_wallet = true
        avatar_size = 20

        [[shell.children.children.children.children]]
        type = "window_controls"
        variant = "windows"
        height = 24

    [[shell.children.children]]
    type = "row"
    align = "center"
    style = { height = 38, gap = 8 }

      [[shell.children.children.children]]
      type = "back_forward"
      size = 26

      [[shell.children.children.children]]
      type = "nav_tabs"
      direction = "horizontal"
      show_icons = false
      uppercase = true
      profile_as_username = true
      indicator = "underline"
      indicator_height = 2.5
      tab_gap = 16

[[shell]]
type = "panel"
side = "bottom"
size = 32
style = { bg = "$chrome", border_top = { width = 1, color = "$border" }, padding = [0, 12] }

  [[shell.children]]
  type = "row"
  justify = "space_between"
  align = "center"

    [[shell.children.children]]
    type = "button"
    label = "Add a Game"
    icon = "plus"
    action = "add_non_steam"

    [[shell.children.children]]
    type = "downloads_bar"
    width = 280

    [[shell.children.children]]
    type = "friends_button"
    size = 24

[[shell]]
type = "panel"
side = "center"
style = { bg = "$bg" }

  [[shell.children]]
  type = "page_content"

[[library]]
type = "panel"
side = "left"
size = 272
resizable = true
style = { bg = "$chrome", border_right = { width = 1, color = "$border" }, padding = [8, 8] }

  [[library.children]]
  type = "col"
  style = { gap = 6 }

    [[library.children.children]]
    type = "library_filters"
    variant = "compact_steam"

    [[library.children.children]]
    type = "library_search"
    placeholder = "Search by name"
    show_filter_button = true

    [[library.children.children]]
    type = "library_list"
    row_height = 24
    icon_size = 18
    group_headers = true
    show_counts = true

[[library]]
type = "panel"
side = "center"
style = { bg = "$bg" }

  [[library.children]]
  type = "library_home_or_detail"

    [[library.children.children]]
    type = "scroll"
    direction = "vertical"

      [[library.children.children.children]]
      type = "whats_new_shelf"
      title = "What's New"
      card_width = 240
      card_height = 135
      card_radius = 0
      date_above = true
      max_items = 5

      [[library.children.children.children]]
      type = "recent_games_shelf"
      title = "Recent Games"
      featured_first = true
      featured_width = 360
      capsule_width = 156
      capsule_height = 234
      card_radius = 0

      [[library.children.children.children]]
      type = "all_games_grid"
      title = "All Games"
      show_count = true
      show_sort = true
      card_aspect = "2:3"
      card_radius = 0
"##;

        let tree = LayoutTree::parse(layout_toml).expect("parse 2-row steam layout");
        assert!(!tree.is_empty());
        assert_eq!(tree.shell.len(), 3);
        assert_eq!(tree.library.len(), 2);
        match &tree.shell[0] {
            Node::Panel { side, size, drag_window, children, .. } => {
                assert_eq!(*side, PanelSide::Top);
                assert_eq!(*size, Some(66.0));
                assert!(*drag_window);
                assert_eq!(children.len(), 1);
            }
            other => panic!("expected top Panel, got {other:?}"),
        }
    }

    #[test]
    fn layout_tree_parses_left_sidebar_shell() {
        let layout_toml = r##"
[[shell]]
type = "panel"
side = "left"
size = 216
style = { bg = "$chrome", border_right = { width = 1, color = "$border" }, padding = [12, 10] }

  [[shell.children]]
  type = "col"
  style = { gap = 8 }

    [[shell.children.children]]
    type = "row"
    align = "center"
    style = { gap = 8 }

      [[shell.children.children.children]]
      type = "logo"
      size = 22

      [[shell.children.children.children]]
      type = "text"
      value = "Aurora"
      style = { font_size = 16, font_weight = "bold", color = "$text" }

    [[shell.children.children]]
    type = "divider"
    orientation = "horizontal"
    color = "$border"
    thickness = 1

    [[shell.children.children]]
    type = "nav_tabs"
    direction = "vertical"
    show_icons = true
    indicator = "left_bar"
    indicator_height = 3

    [[shell.children.children]]
    type = "spacer"
    flex = true

    [[shell.children.children]]
    type = "account"
    show_avatar = true
    show_name = true
    show_wallet = true

[[shell]]
type = "panel"
side = "center"
style = { bg = "$bg" }

  [[shell.children]]
  type = "page_content"
"##;
        let tree = LayoutTree::parse(layout_toml).expect("parse left sidebar layout");
        assert_eq!(tree.shell.len(), 2);
        assert!(tree.library.is_empty());
        match &tree.shell[0] {
            Node::Panel { side, size, .. } => {
                assert_eq!(*side, PanelSide::Left);
                assert_eq!(*size, Some(216.0));
            }
            other => panic!("expected left Panel, got {other:?}"),
        }
    }

    #[test]
    fn install_zip_flat_nested_traversal_and_invalid_layout() {
        let root = temp_dir("zip");
        let themes_dir = root.join("themes");
        let valid_manifest = br##"
[meta]
id = "steam-decl"
name = "Steam Declarative"
version = "1.0.0"

[theme]
base = "Midnight"
accent = "#1A9FFF"
weight_preset = "balanced"
font_scale = 0.96
"##;
        let valid_layout = br#"
[[shell]]
type = "panel"
side = "center"
"#;

        // 1. Flat zip
        let flat = root.join("flat.zip");
        make_zip(&flat, &[("theme.toml", valid_manifest), ("layout.toml", valid_layout)]);
        let meta = install_zip(&flat, &themes_dir).expect("install flat zip");
        assert_eq!(meta.id, "steam-decl");
        assert!(themes_dir.join("steam-decl/layout.toml").is_file());

        // 2. GitHub-style nested zip
        let nested_manifest = br#"
[meta]
id = "nested-decl"
name = "Nested Declarative"
"#;
        let nested = root.join("nested.zip");
        make_zip(
            &nested,
            &[
                ("repo-main/theme.toml", nested_manifest),
                ("repo-main/layout.toml", valid_layout),
            ],
        );
        let meta2 = install_zip(&nested, &themes_dir).expect("install nested zip");
        assert_eq!(meta2.id, "nested-decl");

        // 3. Path traversal rejection
        let evil = root.join("evil.zip");
        make_zip(
            &evil,
            &[
                ("theme.toml", valid_manifest),
                ("../escape.txt", b"nope"),
            ],
        );
        assert!(matches!(install_zip(&evil, &themes_dir), Err(Error::UnsafePath(_))));

        // 4. Invalid layout.toml rejection (must not install)
        let bad_layout = root.join("bad-layout.zip");
        make_zip(
            &bad_layout,
            &[
                ("theme.toml", b"[meta]\nid = \"bad-layout\"\nname = \"Bad\"\n"),
                ("layout.toml", b"[[shell]]\ntype = \"nonexistent_widget\"\n"),
            ],
        );
        assert!(matches!(install_zip(&bad_layout, &themes_dir), Err(Error::InvalidLayout(_))));
        assert!(!themes_dir.join("bad-layout").exists());

        // 5. ThemeManager resolves active theme + option layout_file switching
        let data_dir = root.join("theme-data");
        let mut mgr = ThemeManager::new(themes_dir.clone(), data_dir);
        mgr.scan();
        assert!(mgr.broken.is_empty());
        mgr.activate(Some("steam-decl")).expect("activate steam-decl");
        let res = mgr.resolved.as_ref().expect("resolved");
        assert_eq!(res.theme.weight_preset, ui::WeightPreset::Balanced);
        assert!((res.theme.font_scale - 0.96).abs() < 1e-5);
        assert_eq!(res.layout.shell.len(), 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rhai_script_sandbox_and_draw_cmds() {
        let src = r##"
fn paint_background(w, h, time, theme) {
    [
        #{ type: "gradient", x: 0, y: 0, w: w, h: h, from: theme.accent, to: "#00000000", horizontal: false },
        #{ type: "rect_stroke", x: 2, y: 2, w: w - 4, h: h - 4, radius: 2, width: 1, color: "#FFFFFF40" },
    ]
}
"##;
        let mut script = ThemeScript::compile(src).expect("compile");
        let theme = ui::Theme::nevko();
        let cmds = script.paint_background(800.0, 600.0, 0.0, &theme);
        assert_eq!(cmds.len(), 2);
        assert!(script.error().is_none());

        let mut loop_script =
            ThemeScript::compile("fn paint_background(w, h, t, th) { loop {} }").unwrap();
        let out = loop_script.paint_background(100.0, 100.0, 0.0, &theme);
        assert!(out.is_empty());
        assert!(loop_script.is_disabled());
    }
}

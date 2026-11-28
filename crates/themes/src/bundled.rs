//! Built-in declarative themes bundled directly in the Aurora binary.
//!
//! These are sourced from `docs/examples/` at compile time so there is a single
//! source of truth, and synced into `<config_dir>/themes/<id>/` on startup so
//! every user — whether on a fresh install or updating from an earlier release,
//! across all packaging formats — has Adwaita and Aurora Desktop ready to use.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::install::MAX_TOML_BYTES;
use crate::manifest::{ThemeManifest, safe_rel_path, validate_id};

/// A single static file belonging to a bundled theme package.
#[derive(Debug, Clone, Copy)]
pub struct BundledFile {
    /// Relative path inside the theme directory (forward-slash separated).
    pub path: &'static str,
    pub bytes: &'static [u8],
}

/// A declarative theme package embedded in the Aurora binary.
#[derive(Debug, Clone, Copy)]
pub struct BundledTheme {
    pub id: &'static str,
    pub files: &'static [BundledFile],
}

impl BundledTheme {
    /// Return the embedded `theme.toml` contents as UTF-8.
    pub fn manifest_toml(&self) -> Option<&'static str> {
        self.files
            .iter()
            .find(|f| f.path == "theme.toml")
            .and_then(|f| std::str::from_utf8(f.bytes).ok())
    }

    /// Parse and return the bundled theme's declared version from `theme.toml`.
    pub fn version(&self) -> Result<String, Error> {
        let raw = self.manifest_toml().ok_or(Error::MissingManifest)?;
        let manifest = ThemeManifest::parse(raw)?;
        Ok(manifest.meta.version)
    }
}

/// All themes bundled with Aurora.
pub const BUNDLED_THEMES: &[BundledTheme] = &[
    BundledTheme {
        id: "adwaita",
        files: &[
            BundledFile {
                path: "README.md",
                bytes: include_bytes!("../../../docs/examples/adwaita/README.md"),
            },
            BundledFile {
                path: "UPSTREAM-LICENSE.txt",
                bytes: include_bytes!("../../../docs/examples/adwaita/UPSTREAM-LICENSE.txt"),
            },
            BundledFile {
                path: "theme.toml",
                bytes: include_bytes!("../../../docs/examples/adwaita/theme.toml"),
            },
            BundledFile {
                path: "layout.toml",
                bytes: include_bytes!("../../../docs/examples/adwaita/layout.toml"),
            },
            BundledFile {
                path: "assets/fonts/AdwaitaSans-Regular.ttf",
                bytes: include_bytes!("../../../docs/examples/adwaita/assets/fonts/AdwaitaSans-Regular.ttf"),
            },
            BundledFile {
                path: "assets/fonts/AdwaitaSans-Bold.ttf",
                bytes: include_bytes!("../../../docs/examples/adwaita/assets/fonts/AdwaitaSans-Bold.ttf"),
            },
            BundledFile {
                path: "assets/fonts/OFL.txt",
                bytes: include_bytes!("../../../docs/examples/adwaita/assets/fonts/OFL.txt"),
            },
        ],
    },
    BundledTheme {
        id: "steam-replica",
        files: &[
            BundledFile {
                path: "README.md",
                bytes: include_bytes!("../../../docs/examples/steam-replica/README.md"),
            },
            BundledFile {
                path: "theme.toml",
                bytes: include_bytes!("../../../docs/examples/steam-replica/theme.toml"),
            },
            BundledFile {
                path: "layout.toml",
                bytes: include_bytes!("../../../docs/examples/steam-replica/layout.toml"),
            },
            BundledFile {
                path: "theme.rhai",
                bytes: include_bytes!("../../../docs/examples/steam-replica/theme.rhai"),
            },
        ],
    },
    BundledTheme {
        id: "metro",
        files: &[
            BundledFile {
                path: "README.md",
                bytes: include_bytes!("../../../docs/examples/metro/README.md"),
            },
            BundledFile {
                path: "theme.toml",
                bytes: include_bytes!("../../../docs/examples/metro/theme.toml"),
            },
            BundledFile {
                path: "layout.toml",
                bytes: include_bytes!("../../../docs/examples/metro/layout.toml"),
            },
            BundledFile {
                path: "assets/fonts/Selawik-Regular.ttf",
                bytes: include_bytes!("../../../docs/examples/metro/assets/fonts/Selawik-Regular.ttf"),
            },
            BundledFile {
                path: "assets/fonts/Selawik-Semibold.ttf",
                bytes: include_bytes!("../../../docs/examples/metro/assets/fonts/Selawik-Semibold.ttf"),
            },
            BundledFile {
                path: "assets/fonts/OFL.txt",
                bytes: include_bytes!("../../../docs/examples/metro/assets/fonts/OFL.txt"),
            },
        ],
    },
    BundledTheme {
        id: "square",
        files: &[
            BundledFile {
                path: "README.md",
                bytes: include_bytes!("../../../docs/examples/square/README.md"),
            },
            BundledFile {
                path: "theme.toml",
                bytes: include_bytes!("../../../docs/examples/square/theme.toml"),
            },
        ],
    },
];

/// Returns `true` if `id` is one of Aurora's bundled themes.
pub fn is_bundled_theme(id: &str) -> bool {
    BUNDLED_THEMES.iter().any(|t| t.id == id)
}

/// Summary of actions taken by [`sync_bundled_themes`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BundledSyncReport {
    /// Bundled theme IDs newly installed on disk.
    pub installed: Vec<&'static str>,
    /// Bundled theme IDs upgraded from an older on-disk version.
    pub updated: Vec<&'static str>,
}

const STATE_FILE_NAME: &str = ".bundled-themes.json";

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BundledState {
    /// Highest bundled version already seeded or observed per theme id.
    #[serde(default)]
    seeded_versions: BTreeMap<String, String>,
    /// Bundled theme ids explicitly uninstalled by the user.
    #[serde(default)]
    dismissed: BTreeSet<String>,
}

fn load_state(data_dir: &Path) -> BundledState {
    let path = data_dir.join(STATE_FILE_NAME);
    let Ok(raw) = fs::read_to_string(&path) else {
        return BundledState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_state(data_dir: &Path, state: &BundledState) -> Result<(), Error> {
    fs::create_dir_all(data_dir)?;
    let path = data_dir.join(STATE_FILE_NAME);
    let tmp = data_dir.join(format!("{STATE_FILE_NAME}.tmp"));
    let content = serde_json::to_string_pretty(state)?;
    fs::write(&tmp, content)?;
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Mark a bundled theme as explicitly uninstalled so future launches do not
/// silently re-install it.
pub(crate) fn mark_dismissed(data_dir: &Path, id: &str) -> Result<(), Error> {
    if !is_bundled_theme(id) {
        return Ok(());
    }
    let mut state = load_state(data_dir);
    if state.dismissed.insert(id.to_string()) {
        save_state(data_dir, &state)?;
    }
    Ok(())
}

/// Clear a dismissed flag when the user manually reinstalls a bundled theme.
pub(crate) fn clear_dismissed(data_dir: &Path, id: &str) -> Result<(), Error> {
    if !is_bundled_theme(id) {
        return Ok(());
    }
    let mut state = load_state(data_dir);
    if state.dismissed.remove(id) {
        save_state(data_dir, &state)?;
    }
    Ok(())
}

/// Return the IDs of bundled themes that are not currently present in `themes_dir`.
pub fn missing_bundled_themes(themes_dir: &Path) -> Vec<&'static str> {
    BUNDLED_THEMES
        .iter()
        .filter(|t| !themes_dir.join(t.id).join("theme.toml").is_file())
        .map(|t| t.id)
        .collect()
}

/// Ensure all bundled themes are installed into `themes_dir` on first launch or
/// upgrade, upgrading older on-disk copies while respecting explicit uninstalls,
/// local edits at the current version, and user-installed newer versions.
pub fn sync_bundled_themes(themes_dir: &Path, data_dir: &Path) -> Result<BundledSyncReport, Error> {
    sync_bundled_list(themes_dir, data_dir, BUNDLED_THEMES)
}

/// Re-install any missing bundled themes (clearing any prior dismissal), leaving
/// already-installed themes untouched.
pub fn restore_missing_bundled_themes(themes_dir: &Path, data_dir: &Path) -> Result<Vec<&'static str>, Error> {
    let mut state = load_state(data_dir);
    let mut restored = Vec::new();
    let mut state_dirty = false;

    for bundled in BUNDLED_THEMES {
        let bundled_ver = bundled.version()?;
        let dir = themes_dir.join(bundled.id);
        let exists = fs::symlink_metadata(&dir).is_ok();
        if exists {
            if state.dismissed.remove(bundled.id) {
                state_dirty = true;
            }
            continue;
        }
        write_bundled_theme(themes_dir, bundled)?;
        state.dismissed.remove(bundled.id);
        state
            .seeded_versions
            .insert(bundled.id.to_string(), bundled_ver);
        state_dirty = true;
        restored.push(bundled.id);
    }

    if state_dirty {
        save_state(data_dir, &state)?;
    }
    Ok(restored)
}

fn sync_bundled_list(
    themes_dir: &Path,
    data_dir: &Path,
    bundled_themes: &[BundledTheme],
) -> Result<BundledSyncReport, Error> {
    let mut state = load_state(data_dir);
    let mut state_dirty = false;
    let mut report = BundledSyncReport::default();

    for bundled in bundled_themes {
        let bundled_ver = bundled.version()?;
        let dir = themes_dir.join(bundled.id);

        match fs::symlink_metadata(&dir) {
            Ok(meta) => {
                // Never follow or replace symlinks or non-directory entries.
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    continue;
                }
                // User has this theme on disk; clear any stale dismissal.
                if state.dismissed.remove(bundled.id) {
                    state_dirty = true;
                }

                let manifest_path = dir.join("theme.toml");
                if !manifest_path.exists() {
                    continue;
                }

                let Ok(raw) = crate::read_bounded_text(&manifest_path, MAX_TOML_BYTES) else {
                    continue;
                };
                let Ok(installed_manifest) = ThemeManifest::parse(&raw) else {
                    // Leave broken/work-in-progress manifests alone so the user sees the warning.
                    continue;
                };

                if is_older_version(&installed_manifest.meta.version, &bundled_ver) {
                    write_bundled_theme(themes_dir, bundled)?;
                    record_seeded(&mut state, bundled.id, &bundled_ver, &mut state_dirty);
                    report.updated.push(bundled.id);
                } else {
                    record_seeded(&mut state, bundled.id, &bundled_ver, &mut state_dirty);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if state.dismissed.contains(bundled.id) {
                    continue;
                }
                if state.seeded_versions.contains_key(bundled.id) {
                    // Previously seeded and later removed manually from disk;
                    // do not resurrect on every startup.
                    continue;
                }
                write_bundled_theme(themes_dir, bundled)?;
                record_seeded(&mut state, bundled.id, &bundled_ver, &mut state_dirty);
                report.installed.push(bundled.id);
            }
            Err(e) => return Err(Error::Io(e)),
        }
    }

    if state_dirty {
        save_state(data_dir, &state)?;
    }

    Ok(report)
}

fn record_seeded(state: &mut BundledState, id: &str, version: &str, dirty: &mut bool) {
    let should_update = match state.seeded_versions.get(id) {
        None => true,
        Some(prev) => prev != version && is_older_version(prev, version),
    };
    if should_update {
        state
            .seeded_versions
            .insert(id.to_string(), version.to_string());
        *dirty = true;
    }
}

fn write_bundled_theme(themes_dir: &Path, bundled: &BundledTheme) -> Result<(), Error> {
    validate_id(bundled.id)?;
    fs::create_dir_all(themes_dir)?;

    let final_dir = themes_dir.join(bundled.id);
    if let Ok(meta) = fs::symlink_metadata(&final_dir)
        && (meta.file_type().is_symlink() || !meta.is_dir())
    {
        return Err(Error::UnsafePath(final_dir.display().to_string()));
    }

    let tmp_dir = themes_dir.join(format!(".tmp-bundled-{}", bundled.id));
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)?;
    }
    fs::create_dir_all(&tmp_dir)?;

    let write_res = (|| -> Result<(), Error> {
        for file in bundled.files {
            let out_path = safe_rel_path(&tmp_dir, file.path)
                .ok_or_else(|| Error::UnsafePath(file.path.to_string()))?;
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&out_path, file.bytes)?;
        }
        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    crate::install::commit_tmp_dir(&tmp_dir, &final_dir)?;
    Ok(())
}

/// Returns `true` if `installed` is strictly older than `bundled`.
/// Empty installed versions are treated as older; unparseable non-empty custom
/// version strings are conservatively treated as not older.
pub(crate) fn is_older_version(installed: &str, bundled: &str) -> bool {
    let installed = installed.trim();
    let bundled = bundled.trim();
    if installed.is_empty() {
        return !bundled.is_empty();
    }
    match (parse_version(installed), parse_version(bundled)) {
        (Some(a), Some(b)) => cmp_parsed_version(&a, &b) == Ordering::Less,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedVersion {
    core: Vec<u64>,
    pre: Option<Vec<String>>,
}

fn parse_version(s: &str) -> Option<ParsedVersion> {
    let s = s.trim();
    let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    // Drop optional +build metadata.
    let s = s.split('+').next()?;
    let (core_str, pre_str) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    if core_str.is_empty() {
        return None;
    }
    let mut core = Vec::new();
    for part in core_str.split('.') {
        core.push(part.parse::<u64>().ok()?);
    }
    let pre = pre_str.map(|p| p.split('.').map(String::from).collect());
    Some(ParsedVersion { core, pre })
}

fn cmp_parsed_version(a: &ParsedVersion, b: &ParsedVersion) -> Ordering {
    let max_len = a.core.len().max(b.core.len());
    for i in 0..max_len {
        let av = a.core.get(i).copied().unwrap_or(0);
        let bv = b.core.get(i).copied().unwrap_or(0);
        match av.cmp(&bv) {
            Ordering::Equal => {}
            ord => return ord,
        }
    }
    match (&a.pre, &b.pre) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some(ap), Some(bp)) => {
            for (ai, bi) in ap.iter().zip(bp.iter()) {
                let ord = match (ai.parse::<u64>(), bi.parse::<u64>()) {
                    (Ok(an), Ok(bn)) => an.cmp(&bn),
                    _ => ai.cmp(bi),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            ap.len().cmp(&bp.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aurora-bundled-themes-test-{}-{}-{n}",
            std::process::id(),
            label
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn collect_rel_files(root: &Path, cur: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(cur).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                collect_rel_files(root, &path, out);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }

    #[test]
    fn bundled_themes_match_docs_examples_directories() {
        let examples_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples");
        for bundled in BUNDLED_THEMES {
            let src_dir = examples_root.join(bundled.id);
            let mut on_disk = BTreeMap::new();
            collect_rel_files(&src_dir, &src_dir, &mut on_disk);

            let mut embedded = BTreeMap::new();
            for f in bundled.files {
                let prev = embedded.insert(f.path.to_string(), f.bytes.to_vec());
                assert!(prev.is_none(), "duplicate bundled file {} in {}", f.path, bundled.id);
            }

            let disk_keys: Vec<&String> = on_disk.keys().collect();
            let emb_keys: Vec<&String> = embedded.keys().collect();
            assert_eq!(
                emb_keys, disk_keys,
                "bundled file list for '{}' must stay in sync with docs/examples/{}",
                bundled.id, bundled.id
            );
            for (k, v) in &on_disk {
                assert_eq!(embedded.get(k).unwrap(), v, "byte mismatch for {}/{}", bundled.id, k);
            }
        }
    }

    #[test]
    fn bundled_themes_parse_and_activate_cleanly() {
        let root = temp_dir("fresh-install");
        let themes_dir = root.join("themes");
        let data_dir = root.join("theme-data");

        let mut mgr = crate::ThemeManager::new(themes_dir.clone(), data_dir.clone());
        let report = mgr.sync_bundled_themes().unwrap();
        assert_eq!(report.installed, vec!["adwaita", "steam-replica", "metro", "square"]);
        assert!(report.updated.is_empty());

        mgr.scan();
        assert!(mgr.broken.is_empty(), "broken themes: {:?}", mgr.broken);
        assert!(mgr.get("adwaita").is_some());
        assert!(mgr.get("steam-replica").is_some());

        // Second sync is a no-op.
        let report2 = mgr.sync_bundled_themes().unwrap();
        assert!(report2.installed.is_empty());
        assert!(report2.updated.is_empty());

        // Both bundled themes activate and resolve cleanly.
        mgr.activate(Some("adwaita")).unwrap();
        assert_eq!(mgr.resolved.as_ref().unwrap().meta.id, "adwaita");
        assert!(mgr.resolved.as_ref().unwrap().custom_fonts.is_some());

        mgr.activate(Some("steam-replica")).unwrap();
        assert_eq!(mgr.resolved.as_ref().unwrap().meta.id, "steam-replica");
        assert!(mgr.resolved.as_ref().unwrap().script.is_some());

        mgr.activate(Some("metro")).unwrap();
        assert_eq!(mgr.resolved.as_ref().unwrap().meta.id, "metro");
        assert!(mgr.resolved.as_ref().unwrap().custom_fonts.is_some());

        mgr.activate(Some("square")).unwrap();
        let square = mgr.resolved.as_ref().unwrap();
        assert_eq!(square.meta.id, "square");
        assert_eq!(square.theme.radius, 0);
        assert!(square.layout.shell.is_empty());

        let manifest = mgr.active_manifest().unwrap();
        let palette_opt = manifest.options.get("palette").unwrap();
        for choice in &palette_opt.choices {
            let mut opts = BTreeMap::new();
            opts.insert("palette".into(), crate::OptionValue::Text(choice.value.clone()));
            let parts = manifest.resolve_tokens(&opts);
            assert_eq!(parts.theme.radius, 0, "palette '{}' must keep radius = 0", choice.value);
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn manifestless_theme_survives_sync_and_failed_publication() {
        let root = tempfile::tempdir().unwrap();
        let themes = root.path().join("themes");
        let existing = themes.join(BUNDLED_THEMES[0].id);
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("notes"), b"draft").unwrap();
        let report = sync_bundled_list(&themes, root.path(), &BUNDLED_THEMES[..1]).unwrap();
        assert!(report.installed.is_empty());
        assert!(!existing.join("theme.toml").exists());
        assert!(crate::install::commit_tmp_dir(&themes.join("missing"), &existing).is_err());
        assert_eq!(fs::read(existing.join("notes")).unwrap(), b"draft");
    }

    #[test]
    fn upgrade_replaces_older_version_and_preserves_user_options() {
        let root = temp_dir("upgrade-older");
        let themes_dir = root.join("themes");
        let data_dir = root.join("theme-data");

        // Simulate a user who previously installed Adwaita 1.1.0 manually and saved options.
        let old_dir = themes_dir.join("adwaita");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(
            old_dir.join("theme.toml"),
            "[meta]\nid = \"adwaita\"\nname = \"Adwaita\"\nversion = \"1.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(&data_dir).unwrap();
        let saved_opts = r#"{"appearance":"dark","accent":"purple"}"#;
        fs::write(data_dir.join("adwaita.json"), saved_opts).unwrap();

        let report = sync_bundled_themes(&themes_dir, &data_dir).unwrap();
        assert_eq!(report.installed, vec!["steam-replica", "metro", "square"]);
        assert_eq!(report.updated, vec!["adwaita"]);

        let updated_manifest = ThemeManifest::parse(
            &fs::read_to_string(themes_dir.join("adwaita/theme.toml")).unwrap(),
        )
        .unwrap();
        assert_eq!(updated_manifest.meta.version, BUNDLED_THEMES[0].version().unwrap());
        assert_eq!(fs::read_to_string(data_dir.join("adwaita.json")).unwrap(), saved_opts);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sync_does_not_overwrite_same_or_newer_version() {
        let root = temp_dir("preserve-newer");
        let themes_dir = root.join("themes");
        let data_dir = root.join("theme-data");

        // User installed a custom newer version of steam-replica.
        let custom_dir = themes_dir.join("steam-replica");
        fs::create_dir_all(&custom_dir).unwrap();
        let custom_toml =
            "[meta]\nid = \"steam-replica\"\nname = \"My Custom Desktop\"\nversion = \"9.9.0\"\n";
        fs::write(custom_dir.join("theme.toml"), custom_toml).unwrap();

        let report = sync_bundled_themes(&themes_dir, &data_dir).unwrap();
        assert_eq!(report.installed, vec!["adwaita", "metro", "square"]);
        assert!(report.updated.is_empty());
        assert_eq!(
            fs::read_to_string(custom_dir.join("theme.toml")).unwrap(),
            custom_toml
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn uninstall_prevents_resurrection_and_restore_brings_it_back() {
        let root = temp_dir("uninstall-restore");
        let themes_dir = root.join("themes");
        let data_dir = root.join("theme-data");

        let mut mgr = crate::ThemeManager::new(themes_dir.clone(), data_dir.clone());
        mgr.sync_bundled_themes().unwrap();
        mgr.scan();
        assert!(mgr.missing_bundled_themes().is_empty());

        mgr.uninstall("adwaita").unwrap();
        mgr.scan();
        assert!(mgr.get("adwaita").is_none());
        assert_eq!(mgr.missing_bundled_themes(), vec!["adwaita"]);

        // Subsequent startup sync must NOT resurrect the uninstalled theme.
        let report = mgr.sync_bundled_themes().unwrap();
        assert!(report.installed.is_empty());
        assert!(report.updated.is_empty());
        assert!(!themes_dir.join("adwaita").exists());

        // Even a simulated future bundled version bump respects explicit uninstall.
        static FUTURE_FILES: &[BundledFile] = &[BundledFile {
            path: "theme.toml",
            bytes: b"[meta]\nid = \"adwaita\"\nname = \"Adwaita\"\nversion = \"9.0.0\"\n",
        }];
        let future = [BundledTheme {
            id: "adwaita",
            files: FUTURE_FILES,
        }];
        let report_future = sync_bundled_list(&themes_dir, &data_dir, &future).unwrap();
        assert!(report_future.installed.is_empty());
        assert!(!themes_dir.join("adwaita").exists());

        // Explicit restore brings it back.
        let restored = mgr.restore_bundled_themes().unwrap();
        assert_eq!(restored, vec!["adwaita"]);
        assert!(mgr.get("adwaita").is_some());
        assert!(mgr.missing_bundled_themes().is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn version_comparison_handles_semver_and_prereleases() {
        assert!(is_older_version("1.2.0", "1.2.1"));
        assert!(is_older_version("1.2", "1.2.1"));
        assert!(is_older_version("1.2.1-rc1", "1.2.1"));
        assert!(is_older_version("1.2.1-rc1", "1.2.1-rc2"));
        assert!(is_older_version("1.9.0", "1.10.0"));
        assert!(is_older_version("", "1.0.0"));
        assert!(!is_older_version("1.2.1", "1.2.1"));
        assert!(!is_older_version("1.3.0", "1.2.1"));
        assert!(!is_older_version("custom-dev", "1.2.1"));
    }
}

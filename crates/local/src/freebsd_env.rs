//! FreeBSD runtime dependency and kernel/sysctl requirement checks.
//!
//! Checks both user-space packages needed to run Windows and Linux games on
//! FreeBSD (Wine + 32-bit WoW64 runtime, `linux-steam-utils`, `linux_base-rl9`,
//! Steam, `xdg-utils`, `ffmpeg`) and kernel prerequisites required by
//! Linuxulator / `linux-steam-utils` (`kern.features.linux*`,
//! `security.bsd.unprivileged_chroot`, `vfs.usermount`,
//! `compat.linux.use_real_ifnames`, and `nullfs.ko`).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    Dependency,
    Requirement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: &'static str,
    pub kind: CheckKind,
    pub title: String,
    pub detail: String,
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn dependencies(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.kind == CheckKind::Dependency)
    }

    pub fn requirements(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.kind == CheckKind::Requirement)
    }

    /// All suggested shell commands joined into a copyable script.
    pub fn all_commands(&self) -> String {
        let mut lines = Vec::new();
        for issue in &self.issues {
            for cmd in &issue.commands {
                if !lines.iter().any(|existing: &String| existing == cmd) {
                    lines.push(cmd.clone());
                }
            }
        }
        lines.join("\n")
    }
}

/// Snapshot of system state evaluated by [`evaluate`].
#[derive(Debug, Clone, Default)]
pub struct Probe {
    pub os_major: Option<u32>,
    pub wine_bin: Option<PathBuf>,
    /// True when `wine_bin` is FreeBSD's wrapper script that requires
    /// `~/.i386-wine-pkg` and that 32-bit binary is missing.
    pub wine_missing_i386: bool,
    pub has_lsu: bool,
    pub has_linux_base: bool,
    pub has_steam_root: bool,
    pub steam_has_client_so: bool,
    pub has_xdg_open: bool,
    pub has_ffmpeg: bool,
    pub linux32: bool,
    pub linux64: bool,
    pub unprivileged_chroot: bool,
    pub usermount: bool,
    pub use_real_ifnames: bool,
    pub nullfs_loaded: bool,
}

/// Evaluate a [`Probe`] into a structured [`Report`] of missing dependencies
/// and unsatisfied FreeBSD system requirements.
pub fn evaluate(probe: &Probe) -> Report {
    let mut issues = Vec::new();
    let is_fbsd15_or_newer = probe.os_major.is_some_and(|v| v >= 15);

    // 1. Wine (Windows games)
    match &probe.wine_bin {
        None => {
            let pkg32_cmd = if is_fbsd15_or_newer {
                "/usr/local/share/wine/pkg32.sh --old install -r FreeBSD-ports wine mesa-dri"
            } else {
                "/usr/local/share/wine/pkg32.sh install wine mesa-dri"
            };
            issues.push(Issue {
                id: "wine",
                kind: CheckKind::Dependency,
                title: "Wine is not installed".into(),
                detail: "Required to run Windows games on FreeBSD (install Wine and its 32-bit WoW64 package).".into(),
                commands: vec!["sudo pkg install wine".into(), pkg32_cmd.into()],
            });
        }
        Some(bin) if probe.wine_missing_i386 => {
            let is_proton = bin.to_string_lossy().contains("wine-proton");
            let (detail, commands) = if is_fbsd15_or_newer && is_proton {
                (
                    "FreeBSD's wine-proton wrapper requires 32-bit packages (~/.i386-wine-pkg), which are not built for FreeBSD:15:i386. Install emulators/wine with FreeBSD 14's i386 repo instead.".into(),
                    vec![
                        "sudo pkg install wine".into(),
                        "/usr/local/share/wine/pkg32.sh --old install -r FreeBSD-ports wine mesa-dri".into(),
                    ],
                )
            } else if is_fbsd15_or_newer {
                (
                    "FreeBSD's Wine wrapper requires the 32-bit WoW64 runtime (~/.i386-wine-pkg) to launch games.".into(),
                    vec!["/usr/local/share/wine/pkg32.sh --old install -r FreeBSD-ports wine mesa-dri".into()],
                )
            } else if is_proton {
                (
                    "FreeBSD's wine-proton wrapper requires the 32-bit WoW64 runtime (~/.i386-wine-pkg) to launch games.".into(),
                    vec!["/usr/local/wine-proton/bin/pkg32.sh install wine-proton mesa-dri".into()],
                )
            } else {
                (
                    "FreeBSD's Wine wrapper requires the 32-bit WoW64 runtime (~/.i386-wine-pkg) to launch games.".into(),
                    vec!["/usr/local/share/wine/pkg32.sh install wine mesa-dri".into()],
                )
            };
            issues.push(Issue {
                id: "wine-i386",
                kind: CheckKind::Dependency,
                title: "32-bit Wine runtime is not installed".into(),
                detail,
                commands,
            });
        }
        Some(_) => {}
    }

    // 2. linux-steam-utils
    if !probe.has_lsu {
        issues.push(Issue {
            id: "linux-steam-utils",
            kind: CheckKind::Dependency,
            title: "linux-steam-utils is not installed".into(),
            detail: "Required to run Steam (for SteamStub DRM games) and Linux-native games in an LSU chroot.".into(),
            commands: vec!["sudo pkg install linux-steam-utils".into()],
        });
    }

    // 3. Linuxulator base (linux_base-rl9)
    if !probe.has_linux_base {
        issues.push(Issue {
            id: "linux-base",
            kind: CheckKind::Dependency,
            title: "Linux compatibility base (linux_base-rl9) is not installed".into(),
            detail: "Required in /compat/linux for Linuxulator, Steam, and Linux-native games.".into(),
            commands: vec!["sudo pkg install linux_base-rl9".into()],
        });
    }

    // 4. Steam client installation & first-run update
    if !probe.has_steam_root {
        issues.push(Issue {
            id: "steam",
            kind: CheckKind::Dependency,
            title: "Steam is not installed".into(),
            detail: "Required for SteamStub DRM games (steamhost) and Steam Linux Runtime games.".into(),
            commands: vec![
                "/usr/local/steam-utils/bin/lsu-bootstrap".into(),
                "/usr/local/steam-utils/bin/steam".into(),
            ],
        });
    } else if !probe.steam_has_client_so {
        issues.push(Issue {
            id: "steam-update",
            kind: CheckKind::Dependency,
            title: "Steam has not finished its first-run update".into(),
            detail: "Run Steam once to download linux64/steamclient.so and the Steam runtime.".into(),
            commands: vec!["/usr/local/steam-utils/bin/steam".into()],
        });
    }

    // 5. Desktop helper tools
    if !probe.has_xdg_open {
        issues.push(Issue {
            id: "xdg-utils",
            kind: CheckKind::Dependency,
            title: "xdg-utils is not installed".into(),
            detail: "Used by Aurora to open links and game folders.".into(),
            commands: vec!["sudo pkg install xdg-utils".into()],
        });
    }

    if !probe.has_ffmpeg {
        issues.push(Issue {
            id: "ffmpeg",
            kind: CheckKind::Dependency,
            title: "ffmpeg is not installed".into(),
            detail: "Used by Aurora to play store trailers and video clips.".into(),
            commands: vec!["sudo pkg install ffmpeg".into()],
        });
    }

    // 6. Kernel & sysctl requirements (matching linux-steam-utils preflight checks)
    if !probe.linux32 || !probe.linux64 {
        issues.push(Issue {
            id: "linux-emul",
            kind: CheckKind::Requirement,
            title: "32-bit and 64-bit Linux emulation must be enabled".into(),
            detail: "Enable the FreeBSD Linuxulator service so Steam and Linux games can run.".into(),
            commands: vec![
                "sudo sysrc linux_enable=\"YES\"".into(),
                "sudo service linux start".into(),
            ],
        });
    }

    if !probe.unprivileged_chroot {
        issues.push(Issue {
            id: "unprivileged-chroot",
            kind: CheckKind::Requirement,
            title: "Unprivileged chroot must be enabled".into(),
            detail: "Add security.bsd.unprivileged_chroot=1 to /etc/sysctl.conf to persist it.".into(),
            commands: vec![
                "sudo sysctl security.bsd.unprivileged_chroot=1".into(),
                "echo 'security.bsd.unprivileged_chroot=1' | sudo tee -a /etc/sysctl.conf".into(),
            ],
        });
    }

    if !probe.usermount {
        issues.push(Issue {
            id: "usermount",
            kind: CheckKind::Requirement,
            title: "Unprivileged mounts must be enabled".into(),
            detail: "Add vfs.usermount=1 to /etc/sysctl.conf to persist it.".into(),
            commands: vec![
                "sudo sysctl vfs.usermount=1".into(),
                "echo 'vfs.usermount=1' | sudo tee -a /etc/sysctl.conf".into(),
            ],
        });
    }

    if !probe.use_real_ifnames {
        issues.push(Issue {
            id: "use-real-ifnames",
            kind: CheckKind::Requirement,
            title: "FreeBSD interface names must be enabled".into(),
            detail: "Add compat.linux.use_real_ifnames=1 to /etc/sysctl.kld.d/linux.conf to persist it.".into(),
            commands: vec![
                "sudo sysctl compat.linux.use_real_ifnames=1".into(),
                "sudo mkdir -p /etc/sysctl.kld.d && echo 'compat.linux.use_real_ifnames=1' | sudo tee -a /etc/sysctl.kld.d/linux.conf".into(),
            ],
        });
    }

    if !probe.nullfs_loaded {
        issues.push(Issue {
            id: "nullfs",
            kind: CheckKind::Requirement,
            title: "nullfs kernel module must be loaded".into(),
            detail: "Required by linux-steam-utils to mount game and runtime directories.".into(),
            commands: vec![
                "sudo kldload nullfs".into(),
                "sudo sysrc kld_list+=nullfs".into(),
            ],
        });
    }

    Report { issues }
}

/// Detect whether `wine_bin` is FreeBSD's WoW64 shell wrapper whose 32-bit
/// companion binary under `i386_root` is missing.
pub fn wine_wrapper_missing_i386(wine_bin: &Path, i386_root: &Path) -> bool {
    let Ok(bytes) = std::fs::read(wine_bin) else {
        return false;
    };
    if !bytes.starts_with(b"#!") {
        return false;
    }
    let text = String::from_utf8_lossy(&bytes);
    if !text.contains("I386_ROOT") && !text.contains("pkg32.sh") {
        return false;
    }
    let real = std::fs::canonicalize(wine_bin).unwrap_or_else(|_| wine_bin.to_path_buf());
    let rel = real.strip_prefix("/").unwrap_or(&real);
    !i386_root.join(rel).is_file()
}

/// Probe the live FreeBSD host and return a [`Report`] of any missing
/// dependencies or unsatisfied kernel/sysctl requirements.
#[cfg(target_os = "freebsd")]
pub fn check() -> Report {
    let path_dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();

    let has_in_path = |name: &str| {
        use std::os::unix::fs::PermissionsExt;
        path_dirs.iter().filter(|d| d.is_absolute()).any(|dir| {
            dir.join(name)
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    };

    let sysctl_one = |name: &str| {
        std::process::Command::new("/sbin/sysctl")
            .args(["-nq", name])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
    };

    let os_major = std::process::Command::new("/usr/bin/uname")
        .arg("-r")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .split('.')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
        });

    let wine_tools = crate::compat::discover_wine(Path::new("/usr/local"), &path_dirs);
    let wine_bin = wine_tools.into_iter().find_map(|t| t.wine_bin);

    let i386_root = std::env::var_os("WINE_i386_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".i386-wine-pkg")))
        .unwrap_or_else(|| PathBuf::from("/.i386-wine-pkg"));
    let wine_missing_i386 = wine_bin.as_deref().is_some_and(|bin| wine_wrapper_missing_i386(bin, &i386_root));

    let has_lsu = crate::compat::find_lsu_run_in_chroot().is_some()
        || Path::new("/usr/local/steam-utils/bin/steam").is_file();

    let emul_path = std::process::Command::new("/sbin/sysctl")
        .args(["-nq", "compat.linux.emul_path"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/compat/linux"));
    let has_linux_base = emul_path.join("bin/sh").exists() || emul_path.join("etc/os-release").exists();

    let steam_root = crate::find_steam_root().or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        [home.join(".steam/steam"), home.join(".local/share/Steam")]
            .into_iter()
            .find(|p| p.is_dir())
    });
    let steam_has_client_so = steam_root
        .as_ref()
        .is_some_and(|r| r.join("linux64/steamclient.so").is_file());

    let nullfs_loaded = std::process::Command::new("/sbin/kldstat")
        .args(["-q", "-m", "nullfs"])
        .status()
        .is_ok_and(|s| s.success());

    let probe = Probe {
        os_major,
        wine_bin,
        wine_missing_i386,
        has_lsu,
        has_linux_base,
        has_steam_root: steam_root.is_some(),
        steam_has_client_so,
        has_xdg_open: has_in_path("xdg-open"),
        has_ffmpeg: has_in_path("ffmpeg"),
        linux32: sysctl_one("kern.features.linux"),
        linux64: sysctl_one("kern.features.linux64"),
        unprivileged_chroot: sysctl_one("security.bsd.unprivileged_chroot"),
        usermount: sysctl_one("vfs.usermount"),
        use_real_ifnames: sysctl_one("compat.linux.use_real_ifnames"),
        nullfs_loaded,
    };
    evaluate(&probe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy_probe() -> Probe {
        Probe {
            os_major: Some(15),
            wine_bin: Some(PathBuf::from("/usr/local/bin/wine")),
            wine_missing_i386: false,
            has_lsu: true,
            has_linux_base: true,
            has_steam_root: true,
            steam_has_client_so: true,
            has_xdg_open: true,
            has_ffmpeg: true,
            linux32: true,
            linux64: true,
            unprivileged_chroot: true,
            usermount: true,
            use_real_ifnames: true,
            nullfs_loaded: true,
        }
    }

    #[test]
    fn healthy_system_reports_no_issues() {
        let report = evaluate(&healthy_probe());
        assert!(report.is_empty(), "expected empty report, got {:?}", report.issues);
    }

    #[test]
    fn missing_interface_names_matches_lsu_guidance() {
        let mut p = healthy_probe();
        p.use_real_ifnames = false;
        let report = evaluate(&p);
        assert_eq!(report.issues.len(), 1);
        let issue = &report.issues[0];
        assert_eq!(issue.kind, CheckKind::Requirement);
        assert_eq!(issue.id, "use-real-ifnames");
        assert!(issue.title.contains("FreeBSD interface names"));
        assert!(issue.commands.iter().any(|c| c.contains("compat.linux.use_real_ifnames=1")));
        assert!(issue.detail.contains("/etc/sysctl.kld.d/linux.conf"));
    }

    #[test]
    fn wine_proton_missing_i386_on_freebsd_15_suggests_wine_pkg32_old() {
        let mut p = healthy_probe();
        p.os_major = Some(15);
        p.wine_bin = Some(PathBuf::from("/usr/local/wine-proton/bin/wine"));
        p.wine_missing_i386 = true;
        let report = evaluate(&p);
        assert_eq!(report.issues.len(), 1);
        let issue = &report.issues[0];
        assert_eq!(issue.id, "wine-i386");
        assert!(issue.commands.iter().any(|c| c.contains("pkg32.sh --old")));
    }

    #[test]
    fn missing_everything_reports_all_deps_and_reqs() {
        let report = evaluate(&Probe { os_major: Some(15), ..Probe::default() });
        let ids: Vec<&str> = report.issues.iter().map(|i| i.id).collect();
        for expected in [
            "wine",
            "linux-steam-utils",
            "linux-base",
            "steam",
            "xdg-utils",
            "ffmpeg",
            "linux-emul",
            "unprivileged-chroot",
            "usermount",
            "use-real-ifnames",
            "nullfs",
        ] {
            assert!(ids.contains(&expected), "missing expected issue {expected} in {ids:?}");
        }
        assert!(report.dependencies().count() >= 5);
        assert_eq!(report.requirements().count(), 5);
        assert!(!report.all_commands().is_empty());
    }

    #[test]
    fn detects_freebsd_wine_wrapper_missing_i386_companion() {
        let root = std::env::temp_dir().join(format!("aurora-freebsd-env-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let wrapper = root.join("wine");
        std::fs::write(&wrapper, "#!/bin/sh\nI386_ROOT=\"$HOME/.i386-wine-pkg\"\n").unwrap();
        let i386_root = root.join("i386");
        assert!(wine_wrapper_missing_i386(&wrapper, &i386_root));

        let real = std::fs::canonicalize(&wrapper).unwrap();
        let companion = i386_root.join(real.strip_prefix("/").unwrap());
        std::fs::create_dir_all(companion.parent().unwrap()).unwrap();
        std::fs::write(&companion, b"ELF").unwrap();
        assert!(!wine_wrapper_missing_i386(&wrapper, &i386_root));
        let _ = std::fs::remove_dir_all(&root);
    }
}

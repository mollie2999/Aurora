//! Desktop Screenshots Manager (`View -> Screenshots`).
//!
//! Scans both Aurora-captured screenshots and local Steam `userdata/*/760/remote/*/screenshots`
//! folders. Decodes images manually with strict dimension and memory caps (refusing symlinks)
//! without installing an egui `file://` image loader.

use client::ingame::LocalScreenshot;
use egui::{Align, Color32, CornerRadius, Layout, Margin, Rect, Sense, Stroke, TextureHandle, TextureOptions, Ui, vec2};
use extensions::{Ctx, Extension, Manifest, PageSpec};
use i18n::t;
use model::AppId;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use ui::{icons, widgets as w};

const MAX_SHOT_DIM: u32 = 8192;
const MAX_SHOT_ALLOC: u64 = 64 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ShotSort {
    #[default]
    Newest,
    Oldest,
    Name,
}

#[derive(Clone)]
struct ShotEntry {
    app_id: AppId,
    name: String,
    path: PathBuf,
    thumb: Option<PathBuf>,
    modified: u64,
    from_steam_userdata: bool,
}

enum DecodeResult {
    Thumb { key: PathBuf, image: Result<egui::ColorImage, String> },
    Full { key: PathBuf, image: Result<egui::ColorImage, String> },
}

pub struct Screenshots {
    rt: tokio::runtime::Handle,
    ctx: egui::Context,
    tx: mpsc::UnboundedSender<DecodeResult>,
    rx: mpsc::UnboundedReceiver<DecodeResult>,
    shots: Vec<ShotEntry>,
    loaded_once: bool,
    filter_app: Option<AppId>,
    sort: ShotSort,
    thumbs: HashMap<PathBuf, Option<TextureHandle>>,
    in_flight: HashSet<PathBuf>,
    viewer: Option<PathBuf>,
    viewer_tex: Option<(PathBuf, TextureHandle)>,
    viewer_err: Option<String>,
}

impl Screenshots {
    pub fn new(rt: tokio::runtime::Handle, ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            rt,
            ctx,
            tx,
            rx,
            shots: Vec::new(),
            loaded_once: false,
            filter_app: None,
            sort: ShotSort::Newest,
            thumbs: HashMap::new(),
            in_flight: HashSet::new(),
            viewer: None,
            viewer_tex: None,
            viewer_err: None,
        }
    }

    fn reload(&mut self, ctx: &Ctx) {
        self.loaded_once = true;
        let mut all = Vec::new();
        let mut seen = HashSet::new();
        for (app_id, s) in client::ingame::list_all_screenshots() {
            if seen.insert(s.path.clone()) {
                all.push(ShotEntry {
                    app_id,
                    name: s.name,
                    path: s.path,
                    thumb: s.thumb,
                    modified: s.modified,
                    from_steam_userdata: false,
                });
            }
        }
        if !ctx.local.steam_root.as_os_str().is_empty() {
            for (app_id, s) in scan_steam_userdata_screenshots(&ctx.local.steam_root, ctx.state.steam_id, &ctx.local.accounts) {
                if seen.insert(s.path.clone()) {
                    all.push(ShotEntry {
                        app_id,
                        name: s.name,
                        path: s.path,
                        thumb: s.thumb,
                        modified: s.modified,
                        from_steam_userdata: true,
                    });
                }
            }
        }
        self.shots = all;
    }

    fn decode_image_capped(path: &Path, max_thumb: Option<(u32, u32)>) -> Result<egui::ColorImage, String> {
        let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("Refused non-regular file or symlink".into());
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_SHOT_DIM);
        limits.max_image_height = Some(MAX_SHOT_DIM);
        limits.max_alloc = Some(MAX_SHOT_ALLOC);
        let reader = image::ImageReader::open(path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut reader = reader;
        reader.limits(limits);
        let mut img = reader.decode().map_err(|e| e.to_string())?;
        if let Some((w, h)) = max_thumb {
            if img.width() > w || img.height() > h {
                img = img.thumbnail(w, h);
            }
        }
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        Ok(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
    }

    fn request_thumb(&mut self, key: &Path, src: &Path) {
        if self.thumbs.contains_key(key) || !self.in_flight.insert(key.to_path_buf()) {
            return;
        }
        let key_owned = key.to_path_buf();
        let src_owned = src.to_path_buf();
        let tx = self.tx.clone();
        let egui_ctx = self.ctx.clone();
        self.rt.spawn_blocking(move || {
            let image = Self::decode_image_capped(&src_owned, Some((420, 260)));
            let _ = tx.send(DecodeResult::Thumb { key: key_owned, image });
            egui_ctx.request_repaint();
        });
    }

    fn request_full(&mut self, path: &Path) {
        let key_owned = path.to_path_buf();
        self.viewer_tex = None;
        self.viewer_err = None;
        let tx = self.tx.clone();
        let egui_ctx = self.ctx.clone();
        self.rt.spawn_blocking(move || {
            let image = Self::decode_image_capped(&key_owned, Some((1920, 1200)));
            let _ = tx.send(DecodeResult::Full { key: key_owned, image });
            egui_ctx.request_repaint();
        });
    }

    fn drain_decoded(&mut self, egui_ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                DecodeResult::Thumb { key, image } => {
                    self.in_flight.remove(&key);
                    match image {
                        Ok(ci) => {
                            let h = egui_ctx.load_texture(format!("shot-thumb:{}", key.display()), ci, TextureOptions::LINEAR);
                            self.thumbs.insert(key, Some(h));
                        }
                        Err(_) => {
                            self.thumbs.insert(key, None);
                        }
                    }
                }
                DecodeResult::Full { key, image } => {
                    if self.viewer.as_ref() == Some(&key) {
                        match image {
                            Ok(ci) => {
                                let h = egui_ctx.load_texture(format!("shot-full:{}", key.display()), ci, TextureOptions::LINEAR);
                                self.viewer_tex = Some((key, h));
                                self.viewer_err = None;
                            }
                            Err(e) => {
                                self.viewer_err = Some(e);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn scan_steam_userdata_screenshots(
    steam_root: &Path,
    steam_id: model::SteamId,
    accounts: &[model::LocalAccount],
) -> Vec<(AppId, LocalScreenshot)> {
    let mut account_ids = Vec::new();
    if steam_id != 0 {
        account_ids.push(model::account_id(steam_id));
    }
    for a in accounts {
        let id = model::account_id(a.steam_id);
        if id != 0 && !account_ids.contains(&id) {
            account_ids.push(id);
        }
    }
    let mut out = Vec::new();
    for acct in account_ids {
        let remote = steam_root.join("userdata").join(acct.to_string()).join("760").join("remote");
        let Ok(app_dirs) = std::fs::read_dir(&remote) else { continue };
        for app_entry in app_dirs.flatten() {
            let Ok(ft) = app_entry.file_type() else { continue };
            if ft.is_symlink() || !ft.is_dir() {
                continue;
            }
            let Some(app_id) = app_entry.file_name().to_str().and_then(|s| s.parse::<AppId>().ok()) else {
                continue;
            };
            let shot_dir = app_entry.path().join("screenshots");
            if let Ok(meta) = std::fs::symlink_metadata(&shot_dir) {
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    continue;
                }
            } else {
                continue;
            }
            let Ok(files) = std::fs::read_dir(&shot_dir) else { continue };
            for f in files.flatten() {
                let p = f.path();
                let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
                if m.file_type().is_symlink() || !m.is_file() {
                    continue;
                }
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
                    continue;
                }
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let thumb_cand = shot_dir.join("thumbnails").join(&name);
                let thumb = std::fs::symlink_metadata(&thumb_cand)
                    .ok()
                    .is_some_and(|tm| !tm.file_type().is_symlink() && tm.is_file())
                    .then_some(thumb_cand);
                let modified = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                out.push((app_id, LocalScreenshot { path: p, thumb, name, modified }));
            }
        }
    }
    out
}

impl Extension for Screenshots {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "screenshots",
            name: "Screenshots",
            author: "Aurora",
            description: "View, manage, and organize game screenshots",
            core: true,
            ..Manifest::DEFAULT
        }
    }

    fn pages(&self) -> Vec<PageSpec> {
        vec![PageSpec::new("screenshots", "Screenshots", icons::CAMERA).hidden().order(68)]
    }

    fn on_navigate(&mut self, _page: &str, arg: Option<&str>) {
        self.filter_app = arg.and_then(|a| a.parse::<AppId>().ok());
        self.loaded_once = false;
    }

    fn page(&mut self, _page: &str, ui: &mut Ui, ctx: &mut Ctx) {
        let t = ctx.theme.clone();
        self.drain_decoded(ui.ctx());
        if !self.loaded_once {
            self.reload(ctx);
        }

        let mut open_shot: Option<PathBuf> = None;
        let mut delete_shot: Option<PathBuf> = None;
        let mut reload_now = false;

        egui::Frame::new().inner_margin(Margin::same(22)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(w::text_bold(format!("{}  {}", icons::CAMERA, t!("Screenshots")), 20.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::button(ui, &t, Some(icons::REFRESH), t!("Refresh"), w::Kind::Primary, w::Size::Sm).clicked() {
                        reload_now = true;
                    }
                });
            });
            ui.add_space(10.0);

            let mut app_counts: BTreeMap<AppId, usize> = BTreeMap::new();
            for s in &self.shots {
                *app_counts.entry(s.app_id).or_insert(0) += 1;
            }
            ui.horizontal(|ui| {
                let selected_label = self
                    .filter_app
                    .map(|id| {
                        ctx.state
                            .app_name(id)
                            .map(String::from)
                            .unwrap_or_else(|| format!("App {id}"))
                    })
                    .unwrap_or_else(|| t!("All games").to_string());
                egui::ComboBox::from_id_salt("shot-app-filter")
                    .selected_text(w::text(selected_label, 13.0, t.text))
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(self.filter_app.is_none(), t!("All games")).clicked() {
                            self.filter_app = None;
                        }
                        for (app_id, count) in app_counts {
                            let name = ctx
                                .state
                                .app_name(app_id)
                                .map(String::from)
                                .unwrap_or_else(|| format!("App {app_id}"));
                            if ui.selectable_label(self.filter_app == Some(app_id), format!("{name} ({count})")).clicked() {
                                self.filter_app = Some(app_id);
                            }
                        }
                    });
                ui.add_space(8.0);
                if w::chip(ui, &t, t!("Newest"), self.sort == ShotSort::Newest).clicked() {
                    self.sort = ShotSort::Newest;
                }
                if w::chip(ui, &t, t!("Oldest"), self.sort == ShotSort::Oldest).clicked() {
                    self.sort = ShotSort::Oldest;
                }
                if w::chip(ui, &t, t!("Name"), self.sort == ShotSort::Name).clicked() {
                    self.sort = ShotSort::Name;
                }
            });
            ui.add_space(12.0);

            let mut visible: Vec<ShotEntry> = self
                .shots
                .iter()
                .filter(|s| self.filter_app.is_none_or(|id| id == s.app_id))
                .cloned()
                .collect();
            match self.sort {
                ShotSort::Newest => visible.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.name.cmp(&a.name))),
                ShotSort::Oldest => visible.sort_by(|a, b| a.modified.cmp(&b.modified).then_with(|| a.name.cmp(&b.name))),
                ShotSort::Name => visible.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            }

            if visible.is_empty() {
                w::empty(ui, &t, icons::CAMERA, t!("No screenshots yet"), t!("Take screenshots in-game with F12 or from the Shift+Tab overlay."));
                return;
            }

            let cols = ((ui.available_width() + 12.0) / 252.0).floor().max(1.0) as usize;
            let card_w = ((ui.available_width() - 12.0 * (cols as f32 - 1.0)) / cols as f32).clamp(180.0, 280.0);
            let img_h = (card_w * 9.0 / 16.0).round();

            egui::ScrollArea::vertical().id_salt("shots-grid").auto_shrink([false, false]).show(ui, |ui| {
                for row in visible.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        for shot in row {
                            let src = shot.thumb.as_deref().unwrap_or(&shot.path);
                            self.request_thumb(&shot.path, src);
                            let game_name = ctx
                                .state
                                .app_name(shot.app_id)
                                .map(String::from)
                                .unwrap_or_else(|| format!("App {}", shot.app_id));
                            let frame_resp = egui::Frame::new()
                                .fill(t.surface)
                                .stroke(Stroke::new(1.0, t.border))
                                .corner_radius(t.corner_small())
                                .inner_margin(Margin::same(8))
                                .show(ui, |ui| {
                                    ui.set_width(card_w - 16.0);
                                    let (img_rect, img_resp) = ui.allocate_exact_size(vec2(card_w - 16.0, img_h), Sense::click());
                                    match self.thumbs.get(&shot.path).and_then(|o| o.as_ref()) {
                                        Some(tex) => {
                                            ui.painter().rect_filled(img_rect, CornerRadius::same(4), t.raised);
                                            let ts = tex.size_vec2();
                                            let scale = (img_rect.width() / ts.x.max(1.0)).min(img_rect.height() / ts.y.max(1.0)).min(1.0);
                                            let dst = Rect::from_center_size(img_rect.center(), ts * scale);
                                            ui.painter().image(
                                                tex.id(),
                                                dst,
                                                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                                Color32::WHITE,
                                            );
                                        }
                                        None => {
                                            w::skeleton(ui, &t, img_rect, CornerRadius::same(4));
                                        }
                                    }
                                    if img_resp.on_hover_cursor(egui::CursorIcon::ZoomIn).clicked() {
                                        open_shot = Some(shot.path.clone());
                                    }
                                    ui.add_space(6.0);
                                    ui.add(egui::Label::new(w::text_semibold(&game_name, 13.0, t.text)).truncate());
                                    ui.horizontal(|ui| {
                                        let when = if shot.modified > 0 { ui::date(shot.modified) } else { shot.name.clone() };
                                        ui.add(egui::Label::new(w::text(when, 11.5, t.text_dim)).truncate());
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if !shot.from_steam_userdata && w::icon_button(ui, &t, icons::TRASH, t!("Delete"), false, 0).clicked() {
                                                delete_shot = Some(shot.path.clone());
                                            }
                                            if w::icon_button(ui, &t, icons::FOLDER, t!("Open folder"), false, 0).clicked() {
                                                if let Some(dir) = shot.path.parent() {
                                                    let _ = open::that_detached(dir);
                                                }
                                            }
                                        });
                                    });
                                })
                                .response;
                            if frame_resp.hovered() {
                                ui.painter().rect_stroke(frame_resp.rect, t.corner_small(), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                            }
                        }
                    });
                    ui.add_space(12.0);
                }
            });
        });

        if reload_now {
            self.thumbs.clear();
            self.in_flight.clear();
            self.reload(ctx);
        }
        if let Some(path) = open_shot {
            self.viewer = Some(path.clone());
            self.request_full(&path);
        }
        if let Some(path) = delete_shot {
            if let Err(e) = client::ingame::delete_screenshot(&path) {
                ctx.toast(e);
            } else {
                self.shots.retain(|s| s.path != path);
                self.thumbs.remove(&path);
                if self.viewer.as_ref() == Some(&path) {
                    self.viewer = None;
                }
            }
        }

        // Lightbox preview modal
        if let Some(view_path) = self.viewer.clone() {
            let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
            ui::widgets::modal(ui.ctx(), egui::Id::unique("screenshot-viewer-modal")).show(ui.ctx(), |ui| {
                ui.set_width(820.0);
                ui.horizontal(|ui| {
                    let name = view_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    ui.label(w::text_semibold(name, 14.5, t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &t, Some(icons::X), t!("Close"), w::Kind::Secondary, w::Size::Sm).clicked() {
                            close = true;
                        }
                        if w::button(ui, &t, Some(icons::COPY), t!("Copy path"), w::Kind::Secondary, w::Size::Sm).clicked() {
                            ui.ctx().copy_text(view_path.display().to_string());
                        }
                        if w::button(ui, &t, Some(icons::FOLDER), t!("Open folder"), w::Kind::Primary, w::Size::Sm).clicked() {
                            if let Some(dir) = view_path.parent() {
                                let _ = open::that_detached(dir);
                            }
                        }
                    });
                });
                ui.add_space(8.0);
                let (rect, _) = ui.allocate_exact_size(vec2(820.0, 480.0), Sense::hover());
                match &self.viewer_tex {
                    Some((p, tex)) if *p == view_path => {
                        let ts = tex.size_vec2();
                        let scale = (rect.width() / ts.x.max(1.0)).min(rect.height() / ts.y.max(1.0)).min(1.0);
                        let sz = ts * scale;
                        let dst = Rect::from_center_size(rect.center(), sz);
                        ui.painter().image(
                            tex.id(),
                            dst,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    _ => {
                        if let Some(err) = &self.viewer_err {
                            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, err, w::font(13.0), t.danger);
                        } else {
                            ui.put(Rect::from_center_size(rect.center(), vec2(32.0, 32.0)), egui::Spinner::new().size(28.0).color(t.accent));
                        }
                    }
                }
            });
            if close {
                self.viewer = None;
                self.viewer_tex = None;
            }
        }
    }
}

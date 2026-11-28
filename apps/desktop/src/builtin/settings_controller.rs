//! Settings → Controller: detected controllers, live input tester,
//! calibration & deadzones, global Steam Input family toggles, and per-game
//! Steam Input bindings inspector/selector.

use super::settings::switch;
use client::{
    BindingChoice, BindingOrigin, BuiltInTemplate, Command, CommunityCursor, CommunityLayout, ConnectionKind, Control,
    ControllerFamily, ControllerSettings, DetectedController, InputConfigReport, LayoutDocument, LayoutRequest, Output,
    PadCalibration, TemplateChoice,
};
use egui::{Align, Align2, CornerRadius, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use extensions::{Action, Ctx};
use i18n::t;
use model::AppId;
use std::path::PathBuf;
use std::time::Duration;
use ui::{Theme, icons, widgets as w};

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum BindingsTab {
    #[default]
    Configuration,
    Community,
    Editor,
}

#[derive(Default)]
pub struct ControllerPage {
    selected_controller: Option<String>,
    selected_app: Option<AppId>,
    inspected_app: Option<AppId>,
    inspected_choice: Option<BindingChoice>,
    inspector_loading: bool,
    report: Option<InputConfigReport>,
    tab: BindingsTab,
    next_req_id: u64,
    inspect_req: Option<LayoutRequest>,
    community_app: Option<AppId>,
    community_req: Option<LayoutRequest>,
    community_loading: bool,
    community_error: Option<String>,
    community_items: Vec<CommunityLayout>,
    community_next: Option<CommunityCursor>,
    downloading_req: Option<(LayoutRequest, u64)>,
    editor_app: Option<AppId>,
    editor_doc: Option<LayoutDocument>,
    editor_title: String,
    editor_selected: Option<Control>,
    editor_dirty: bool,
}

impl ControllerPage {
    fn next_request(&mut self, account: u32, app_id: AppId) -> LayoutRequest {
        self.next_req_id = self.next_req_id.wrapping_add(1).max(1);
        LayoutRequest { account, app_id, request_id: self.next_req_id }
    }

    /// Open with an optional navigation argument (`"controller"` or `"controller:<app_id>"`).
    pub fn open_arg(&mut self, arg: &str) {
        if let Some(id) = arg.strip_prefix("controller:").and_then(|s| s.parse::<AppId>().ok()) {
            self.selected_app = Some(id);
            self.inspected_app = None;
            self.community_app = None;
        }
    }

    /// Receive async inspection, community layout, and download results from the Steam engine.
    pub fn on_event(&mut self, event: &client::Event) {
        match event {
            client::Event::InputConfigInspected { key, report }
                if self.inspect_req == Some(*key) && self.selected_app == Some(key.app_id) =>
            {
                self.report = Some(report.clone());
                self.inspector_loading = false;
            }
            client::Event::ControllerLayouts { key, result }
                if self.community_req == Some(*key) && self.selected_app == Some(key.app_id) =>
            {
                self.community_loading = false;
                match result {
                    Ok(page) => {
                        self.community_error = None;
                        for item in &page.items {
                            if !self.community_items.iter().any(|existing| existing.id == item.id) {
                                self.community_items.push(item.clone());
                            }
                        }
                        self.community_next = page.next.clone();
                    }
                    Err(err) => {
                        self.community_error = Some(err.clone());
                    }
                }
            }
            client::Event::ControllerLayoutDownloaded { key, result }
                if self.downloading_req.is_some_and(|(req, _)| req == *key) =>
            {
                self.downloading_req = None;
                match result {
                    Ok(choice) => {
                        self.community_error = None;
                        client::set_app_binding(key.app_id, choice.clone());
                        if self.selected_app == Some(key.app_id) {
                            self.inspected_app = None;
                        }
                    }
                    Err(err) => {
                        self.community_error = Some(err.clone());
                    }
                }
            }
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        w::page_title(
            ui,
            t,
            t!("Controller"),
            Some(t!("Detected controllers, live input testing, deadzones, Steam Input toggles and per-game bindings.")),
        );

        let mut cfg = client::controller_settings();
        let before = cfg.clone();

        let snapshot = client::poll_controllers();
        if !snapshot.controllers.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }

        self.detected_controllers_ui(ui, t, &snapshot, &cfg);
        ui.add_space(12.0);
        self.test_inputs_ui(ui, t, &snapshot, &cfg.calibration);
        ui.add_space(12.0);
        steam_input_families_ui(ui, t, &mut cfg);
        ui.add_space(12.0);
        calibration_ui(ui, t, &mut cfg.calibration);

        if cfg != before {
            client::set_controller_settings(cfg);
        }

        ui.add_space(12.0);
        self.bindings_inspector_ui(ui, t, ctx);
    }

    fn detected_controllers_ui(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        snapshot: &client::DetectionSnapshot,
        cfg: &ControllerSettings,
    ) {
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(w::text_semibold(t!("Detected controllers"), 16.0, t.text));
                if !snapshot.backend.is_empty() {
                    w::pill(ui, snapshot.backend, t.text_dim, t.raised);
                }
                let vpad_name = client::virtual_pad_backend();
                if client::virtual_pad_available() {
                    w::pill(ui, &format!("{vpad_name} virtual pad"), t.play, t.play.gamma_multiply(0.14));
                } else if vpad_name != "unsupported" {
                    w::pill(ui, &format!("{vpad_name} unavailable"), t.away, t.away.gamma_multiply(0.14));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::button(ui, t, Some(icons::REFRESH), t!("Refresh"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        client::refresh_controllers();
                    }
                });
            });
            ui.add_space(6.0);

            if snapshot.controllers.is_empty() {
                w::panel(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(w::text(icons::CONTROLLER, 26.0, t.text_faint));
                        ui.vertical(|ui| {
                            ui.label(w::text_medium(t!("No controllers detected"), 14.0, t.text));
                            ui.add(
                                egui::Label::new(w::text(
                                    t!("Connect an Xbox, PlayStation, Switch Pro or generic gamepad via USB or Bluetooth."),
                                    12.5,
                                    t.text_dim,
                                ))
                                .wrap(),
                            );
                        });
                    });
                });
                if let Some(note) = &snapshot.note {
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(w::text(i18n::tr_dyn(note), 12.0, t.away)).wrap());
                }
                self.selected_controller = None;
                return;
            }

            if self
                .selected_controller
                .as_ref()
                .is_none_or(|id| !snapshot.controllers.iter().any(|c| &c.id == id))
            {
                self.selected_controller = snapshot.controllers.first().map(|c| c.id.clone());
            }

            for c in &snapshot.controllers {
                let selected = self.selected_controller.as_deref() == Some(c.id.as_str());
                let fill = if selected { w::active_fill(t) } else { t.raised };
                let stroke = if selected {
                    Stroke::new(1.5, t.accent)
                } else {
                    Stroke::new(1.0, t.border)
                };
                let frame = egui::Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(t.corner_small())
                    .inner_margin(Margin::symmetric(12, 10));
                let resp = frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(w::text(icons::CONTROLLER, 22.0, if selected { t.accent } else { t.text_dim }));
                        ui.vertical(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(w::text_semibold(&c.name, 14.0, t.text));
                                let fam_label = family_label(c.family);
                                w::pill(ui, fam_label, t.accent, t.accent.gamma_multiply(0.14));
                                if cfg.families.is_enabled(c.family) {
                                    w::pill(ui, t!("Steam Input"), t.play, t.play.gamma_multiply(0.14));
                                } else {
                                    w::pill(ui, t!("Off globally"), t.text_faint, t.surface);
                                }
                            });
                            let status = controller_status_line(c);
                            ui.label(w::text(status, 12.0, t.text_dim));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if c.has_rumble
                                && cfg.calibration.rumble_enabled
                                && w::button(ui, t, None, t!("Vibrate"), w::Kind::Secondary, w::Size::Sm).clicked()
                            {
                                client::trigger_test_rumble(&c.id);
                            }
                        });
                    });
                });
                if resp.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.selected_controller = Some(c.id.clone());
                }
                ui.add_space(6.0);
            }

            if let Some(note) = &snapshot.note {
                ui.add(egui::Label::new(w::text(i18n::tr_dyn(note), 12.0, t.away)).wrap());
            }
        });
    }

    fn test_inputs_ui(
        &self,
        ui: &mut Ui,
        t: &Theme,
        snapshot: &client::DetectionSnapshot,
        cal: &PadCalibration,
    ) {
        let selected = self
            .selected_controller
            .as_deref()
            .and_then(|id| snapshot.controllers.iter().find(|c| c.id == id))
            .or_else(|| snapshot.controllers.first());

        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(w::text_semibold(t!("Test Device Inputs"), 16.0, t.text));
                if let Some(c) = selected {
                    ui.label(w::text(format!("·  {}", c.name), 13.0, t.text_dim));
                }
            });
            ui.add_space(6.0);

            let Some(c) = selected else {
                ui.label(w::text(t!("Select a connected controller above to inspect live sticks, triggers, buttons and motion sensors."), 13.0, t.text_dim));
                return;
            };

            let live = &c.live;
            let b = |mask: u16| live.buttons & mask != 0;
            let nintendo = cal.nintendo_layout;

            ui.columns(3, |cols| {
                // Left stick + LB + LT
                cols[0].vertical_centered(|ui| {
                    trigger_bar(ui, t, "LT", live.raw_lt, live.lt, cal.trigger_threshold);
                    ui.add_space(6.0);
                    bumper_pill(ui, t, "LB", b(0x0100));
                    ui.add_space(10.0);
                    stick_pad(
                        ui,
                        t,
                        t!("Left stick"),
                        live.raw_lx,
                        live.raw_ly,
                        live.lx,
                        live.ly,
                        cal.left_deadzone,
                        b(0x0040),
                    );
                });

                // Center: D-pad, center buttons, face buttons diamond, extra buttons
                cols[1].vertical_centered(|ui| {
                    ui.label(w::caption_text(t, t!("BUTTONS & D-PAD")));
                    ui.add_space(6.0);
                    dpad_cross(ui, t, b(0x0001), b(0x0002), b(0x0004), b(0x0008));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let total_w = 172.0;
                        let pad = ((ui.available_width() - total_w) * 0.5).max(0.0);
                        ui.add_space(pad);
                        button_pill(ui, t, "View", b(0x0020));
                        button_pill(ui, t, "Guide", b(0x0400));
                        button_pill(ui, t, "Menu", b(0x0010));
                    });
                    ui.add_space(10.0);
                    let (south, east, west, north) = if nintendo {
                        ("B", "A", "Y", "X")
                    } else {
                        ("A", "B", "X", "Y")
                    };
                    face_diamond(ui, t, (south, b(0x1000)), (east, b(0x2000)), (west, b(0x4000)), (north, b(0x8000)));
                    if live.misc || live.touchpad || live.paddles.iter().any(|p| *p) {
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            if live.misc {
                                button_pill(ui, t, "Misc", true);
                            }
                            if live.touchpad {
                                button_pill(ui, t, "Touchpad", true);
                            }
                            for (i, on) in live.paddles.iter().enumerate() {
                                if *on {
                                    button_pill(ui, t, &format!("P{}", i + 1), true);
                                }
                            }
                        });
                    }
                });

                // Right stick + RB + RT
                cols[2].vertical_centered(|ui| {
                    trigger_bar(ui, t, "RT", live.raw_rt, live.rt, cal.trigger_threshold);
                    ui.add_space(6.0);
                    bumper_pill(ui, t, "RB", b(0x0200));
                    ui.add_space(10.0);
                    stick_pad(
                        ui,
                        t,
                        t!("Right stick"),
                        live.raw_rx,
                        live.raw_ry,
                        live.rx,
                        live.ry,
                        cal.right_deadzone,
                        b(0x0080),
                    );
                });
            });

            ui.add_space(10.0);
            w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !c.has_gyro {
                    ui.label(w::text(t!("Motion sensors (gyro / accelerometer): not available on this controller."), 12.5, t.text_dim));
                } else if !cal.gyro_enabled {
                    ui.label(w::text(t!("Motion sensors are disabled in Calibration & Deadzones below."), 12.5, t.away));
                } else {
                    ui.label(w::text_medium(t!("Motion sensors"), 13.0, t.text));
                    ui.add_space(4.0);
                    ui.columns(2, |cols| {
                        motion_axis_group(
                            &mut cols[0],
                            t,
                            t!("Gyroscope (rad/s)"),
                            live.gyro,
                            8.0,
                        );
                        motion_axis_group(
                            &mut cols[1],
                            t,
                            t!("Accelerometer (m/s²)"),
                            live.accel,
                            20.0,
                        );
                    });
                }
            });
        });
    }

    fn bindings_inspector_ui(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let games = available_games(ctx);
        if self.selected_app.is_none() {
            self.selected_app = games.first().map(|(id, _, _)| *id);
        }

        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::text_semibold(t!("Per-game bindings"), 16.0, t.text));
            ui.add(
                egui::Label::new(w::text(
                    t!("Inspect Steam Input action sets or choose a controller configuration file for a game."),
                    12.5,
                    t.text_dim,
                ))
                .wrap(),
            );
            ui.add_space(8.0);

            if games.is_empty() {
                ui.label(w::text(t!("No games found in your library yet."), 13.0, t.text_dim));
                return;
            }

            let selected_id = self.selected_app.unwrap_or(games[0].0);
            let selected_entry = games.iter().find(|(id, _, _)| *id == selected_id).cloned().unwrap_or_else(|| games[0].clone());
            let (app_id, app_name, install_dir) = selected_entry;
            self.selected_app = Some(app_id);

            let current_choice = client::app_binding(app_id);
            let need_refresh = self.inspected_app != Some(app_id)
                || self.inspected_choice.as_ref() != Some(&current_choice);

            if need_refresh {
                self.inspect_now(ctx, app_id, &install_dir, &current_choice);
            }

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                egui::ComboBox::from_id_salt("ctrl-bindings-game")
                    .selected_text(&app_name)
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (id, name, _) in &games {
                            ui.selectable_value(&mut self.selected_app, Some(*id), name);
                        }
                    });
                if editor_action_button(ui, t, Some(icons::REFRESH), t!("Refresh"), false).clicked() {
                    self.inspect_now(ctx, app_id, &install_dir, &current_choice);
                    if self.tab == BindingsTab::Community {
                        self.query_community(ctx, app_id, true);
                    }
                }
                if editor_action_button(ui, t, Some(icons::SLIDERS), t!("Game Properties…"), false).clicked() {
                    ctx.act(Action::Navigate { page: "library".into(), arg: Some(format!("props:{app_id}")) });
                }
                if editor_action_button(ui, t, Some(icons::FOLDER), t!("Aurora bindings folder"), false).clicked() {
                    let account = (ctx.state.steam_id & 0xFFFF_FFFF) as u32;
                    let dir = client::managed_bindings_dir(account, app_id);
                    let _ = std::fs::create_dir_all(&dir);
                    local::open_folder(&dir);
                }
            });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                let tabs = [
                    (BindingsTab::Configuration, icons::CONTROLLER, t!("Configuration")),
                    (BindingsTab::Community, icons::USERS, t!("Community Layouts")),
                    (BindingsTab::Editor, icons::SLIDERS, t!("Edit Layout")),
                ];
                for (tab, icon, label) in tabs {
                    let kind = if self.tab == tab { w::Kind::Primary } else { w::Kind::Secondary };
                    if w::button(ui, t, Some(icon), label, kind, w::Size::Sm).clicked() {
                        self.tab = tab;
                    }
                }
            });

            ui.add_space(8.0);
            match self.tab {
                BindingsTab::Configuration => {
                    self.bindings_config_tab(ui, t, ctx, app_id, &install_dir, &current_choice);
                }
                BindingsTab::Community => {
                    self.bindings_community_tab(ui, t, ctx, app_id, &current_choice);
                }
                BindingsTab::Editor => {
                    self.bindings_editor_tab(ui, t, ctx, app_id, &install_dir);
                }
            }
        });
    }

    fn bindings_config_tab(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        ctx: &mut Ctx,
        app_id: AppId,
        install_dir: &PathBuf,
        current_choice: &BindingChoice,
    ) {
        let Some(report) = self.report.clone() else { return };

        let mut picked = current_choice.clone();
        let chosen_label = choice_label(&picked, &report);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.label(w::text_medium(t!("Configuration:"), 13.0, t.text));
            egui::ComboBox::from_id_salt(("ctrl-bindings-cfg", app_id))
                .selected_text(chosen_label)
                .width(290.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut picked, BindingChoice::Auto, t!("Automatic (default)"));
                    let has_templates = report.available.iter().any(|o| o.origin == BindingOrigin::Template);
                    if has_templates {
                        ui.separator();
                        ui.label(w::caption_text(t, t!("TEMPLATES")));
                        for opt in report.available.iter().filter(|o| o.origin == BindingOrigin::Template) {
                            ui.selectable_value(&mut picked, opt.choice.clone(), option_label(opt));
                        }
                    }
                    let has_others = report.available.iter().any(|o| o.origin != BindingOrigin::Template);
                    if has_others {
                        ui.separator();
                        ui.label(w::caption_text(t, t!("GAME & SAVED LAYOUTS")));
                        for opt in report.available.iter().filter(|o| o.origin != BindingOrigin::Template) {
                            ui.selectable_value(&mut picked, opt.choice.clone(), option_label(opt));
                        }
                    }
                });
            if w::button(ui, t, Some(icons::FOLDER), t!("Browse .vdf…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("Steam Input VDF", &["vdf"]).pick_file() {
                    if client::read_layout(&path).is_ok() {
                        picked = BindingChoice::LocalFile(path);
                    } else {
                        ctx.toast(t!("That file does not look like a Steam Input controller configuration."));
                    }
                }
            }
            if w::button(ui, t, Some(icons::SLIDERS), t!("Edit Layout"), w::Kind::Secondary, w::Size::Sm).clicked() {
                self.load_editor_from_active(app_id, &report);
                self.tab = BindingsTab::Editor;
            }
            if let Some(path) = report.active_path.as_ref() {
                if w::button(ui, t, Some(icons::EXTERNAL), t!("Open"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    let _ = open::that(path);
                }
            } else if let Some(dir) = report.config_dir.as_ref() {
                if w::button(ui, t, Some(icons::FOLDER), t!("Open folder"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    let _ = std::fs::create_dir_all(dir);
                    local::open_folder(dir);
                }
            }
            if *current_choice != BindingChoice::Auto
                && w::button(ui, t, Some(icons::RESTORE), t!("Reset"), w::Kind::Ghost, w::Size::Sm).clicked()
            {
                picked = BindingChoice::Auto;
            }
        });

        let binding_changed = picked != *current_choice;

        ui.add_space(8.0);
        w::panel(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                if self.inspector_loading {
                    ui.spinner();
                }
                if report.active_source.is_empty() {
                    ui.label(w::text(
                        t!("No native Steam Input manifest found — choose a Template, Community Layout, or create a custom binding."),
                        13.0,
                        t.text_dim,
                    ));
                } else {
                    ui.label(w::text_medium(t!("Active source: {source}", source = &report.active_source), 13.0, t.text));
                }
                if !report.active_controller_type.is_empty() {
                    w::pill(ui, &report.active_controller_type, t.accent, t.accent.gamma_multiply(0.14));
                }
            });

            if let Some(doc) = &report.document {
                let legacy = doc.legacy_map();
                if !legacy.0.is_empty() {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(t.surface)
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(w::text_semibold(t!("Controller remappings"), 13.5, t.text));
                                w::pill(ui, t!("Legacy / Template"), t.text_dim, t.raised);
                            });
                            ui.add_space(4.0);
                            for (ctrl, outs) in legacy.0.iter().take(20) {
                                let labels: Vec<String> = outs.iter().map(Output::display_label).collect();
                                ui.horizontal(|ui| {
                                    ui.label(w::text(ctrl.name(), 12.0, t.text_dim).monospace());
                                    ui.label(w::text("→", 12.0, t.text_faint));
                                    ui.label(w::text(labels.join(", "), 12.0, t.text));
                                });
                            }
                            if legacy.0.len() > 20 {
                                ui.label(w::text(t!("and {n} more bindings", n = legacy.0.len() - 20), 11.5, t.text_faint));
                            }
                        });
                }
            }

            if !report.action_sets.is_empty() {
                ui.add_space(8.0);
                for set in &report.action_sets {
                    egui::Frame::new()
                        .fill(t.surface)
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let heading = if set.title.is_empty() || set.title.starts_with('#') {
                                    &set.name
                                } else {
                                    &set.title
                                };
                                ui.label(w::text_semibold(heading, 13.5, t.text));
                                let badge = if set.is_layer { t!("Action layer") } else { t!("Action set") };
                                w::pill(ui, badge, t.text_dim, t.raised);
                            });
                            if set.bindings.is_empty() {
                                ui.label(w::text(t!("No mapped game actions in this set."), 12.0, t.text_faint));
                            } else {
                                ui.add_space(4.0);
                                for (source, input, acts) in set.bindings.iter().take(16) {
                                    ui.horizontal(|ui| {
                                        ui.label(w::text(format!("{source} · {input}"), 12.0, t.text_dim).monospace());
                                        ui.label(w::text("→", 12.0, t.text_faint));
                                        ui.label(w::text(acts.join(", "), 12.0, t.text));
                                    });
                                }
                                if set.bindings.len() > 16 {
                                    ui.label(w::text(t!("and {n} more bindings", n = set.bindings.len() - 16), 11.5, t.text_faint));
                                }
                            }
                        });
                    ui.add_space(6.0);
                }
            }
        });

        if binding_changed {
            client::set_app_binding(app_id, picked.clone());
            self.inspect_now(ctx, app_id, install_dir, &picked);
        }
    }

    fn query_community(&mut self, ctx: &Ctx, app_id: AppId, reset: bool) {
        if reset {
            self.community_items.clear();
            self.community_next = None;
            self.community_error = None;
        }
        self.community_app = Some(app_id);
        if ctx.state.phase != client::state::Phase::LoggedIn || ctx.state.link != client::Link::Online {
            self.community_loading = false;
            self.community_error = Some(t!("Sign in and go online to browse Community Layouts.").to_string());
            return;
        }
        let account = (ctx.state.steam_id & 0xFFFF_FFFF) as u32;
        let key = self.next_request(account, app_id);
        let cursor = if reset { None } else { self.community_next.clone() };
        self.community_req = Some(key);
        self.community_loading = true;
        ctx.send(Command::QueryControllerLayouts { key, cursor });
    }

    fn bindings_community_tab(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        ctx: &mut Ctx,
        app_id: AppId,
        current_choice: &BindingChoice,
    ) {
        if self.community_app != Some(app_id) {
            self.query_community(ctx, app_id, true);
        }

        editor_workspace_frame(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(w::text_semibold(t!("Community Layouts"), 14.0, t.text));
                if self.community_loading {
                    ui.spinner();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if editor_action_button(ui, t, Some(icons::REFRESH), t!("Refresh"), false).clicked() {
                        self.query_community(ctx, app_id, true);
                    }
                });
            });
            ui.add_space(8.0);

            if let Some(err) = &self.community_error {
                ui.add(egui::Label::new(w::text(err, 12.5, t.away)).wrap());
                ui.add_space(6.0);
            }

            if self.community_items.is_empty() {
                if !self.community_loading && self.community_error.is_none() {
                    ui.label(w::text(t!("No community controller layouts found for this game yet."), 13.0, t.text_dim));
                }
                return;
            }

            let mut download_id: Option<u64> = None;
            for item in &self.community_items {
                let active = *current_choice == BindingChoice::Community(item.id);
                let busy = self.downloading_req.is_some_and(|(_, id)| id == item.id);
                egui::Frame::new()
                    .fill(if active { w::active_fill(t) } else { t.surface })
                    .stroke(Stroke::new(1.0, if active { t.accent } else { editor_button_border(t) }))
                    .corner_radius(t.corner_small())
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal_wrapped(|ui| {
                                    let title = if item.title.is_empty() {
                                        t!("Workshop #{id}", id = item.id)
                                    } else {
                                        item.title.clone()
                                    };
                                    ui.label(w::text_semibold(title, 13.5, t.text));
                                    if !item.controller_type.is_empty() {
                                        w::pill(ui, &item.controller_type, t.accent, t.accent.gamma_multiply(0.14));
                                    }
                                });
                                let persona = ctx.state.persona(item.author);
                                let author = if persona.name.is_empty() {
                                    format!("#{}", item.author)
                                } else {
                                    persona.name
                                };
                                let stats = format!(
                                    "{author}  ·  ▲ {}  ▼ {}  ·  {}",
                                    item.votes_up,
                                    item.votes_down,
                                    t!("{n} uses", n = item.sessions),
                                );
                                ui.label(w::text(stats, 11.5, t.text_dim));
                                if !item.description.is_empty() {
                                    ui.add(egui::Label::new(w::text(&item.description, 12.0, t.text_dim)).wrap());
                                }
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if active {
                                    w::pill(ui, t!("Active"), t.play, t.play.gamma_multiply(0.14));
                                } else if busy {
                                    ui.spinner();
                                } else if w::button(ui, t, None, t!("Download & Apply"), w::Kind::Primary, w::Size::Sm).clicked() {
                                    download_id = Some(item.id);
                                }
                            });
                        });
                    });
                ui.add_space(6.0);
            }

            if self.community_next.is_some()
                && !self.community_loading
                && editor_action_button(ui, t, None, t!("Load more"), false).clicked()
            {
                self.query_community(ctx, app_id, false);
            }

            if let Some(id) = download_id {
                let account = (ctx.state.steam_id & 0xFFFF_FFFF) as u32;
                let key = self.next_request(account, app_id);
                self.downloading_req = Some((key, id));
                ctx.send(Command::DownloadControllerLayout { key, id });
            }
        });
    }

    fn load_editor_from_active(&mut self, app_id: AppId, report: &InputConfigReport) {
        let doc = report.document.clone().unwrap_or_else(LayoutDocument::automatic);
        self.editor_title = doc.title().unwrap_or("Custom Layout").to_string();
        self.editor_doc = Some(doc);
        self.editor_app = Some(app_id);
        self.editor_selected.get_or_insert(Control::A);
        self.editor_dirty = false;
    }

    fn bindings_editor_tab(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        ctx: &mut Ctx,
        app_id: AppId,
        install_dir: &PathBuf,
    ) {
        if self.editor_app != Some(app_id) || self.editor_doc.is_none() {
            if let Some(report) = self.report.clone() {
                self.load_editor_from_active(app_id, &report);
            } else {
                self.editor_doc = Some(LayoutDocument::automatic());
                self.editor_title = "Custom Layout".into();
                self.editor_app = Some(app_id);
                self.editor_selected = Some(Control::A);
                self.editor_dirty = false;
            }
        }

        let account = (ctx.state.steam_id & 0xFFFF_FFFF) as u32;
        editor_workspace_frame(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Start-from template strip
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                ui.label(w::text_semibold(t!("Start from:"), 12.5, t.text_dim));
                if editor_action_button(ui, t, None, t!("Automatic"), false).clicked() {
                    self.editor_doc = Some(LayoutDocument::automatic());
                    self.editor_title = "Custom Layout".into();
                    self.editor_dirty = true;
                }
                for tmpl in BuiltInTemplate::ALL {
                    let choice = BindingChoice::Template(TemplateChoice::BuiltIn(tmpl));
                    let label = builtin_template_short_label(tmpl);
                    if editor_action_button(ui, t, None, label, false).clicked() {
                        if let Some(doc) = client::binding_document(&choice, &ctx.local.steam_root, account, app_id) {
                            self.editor_title = format!("{} (Custom)", doc.title().unwrap_or(label));
                            self.editor_doc = Some(doc);
                            self.editor_dirty = true;
                        }
                    }
                }
                if let Some(report) = self.report.clone()
                    && report.document.is_some()
                    && editor_action_button(ui, t, Some(icons::RESTORE), t!("Active layout"), false).clicked()
                {
                    self.load_editor_from_active(app_id, &report);
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(w::text_medium(t!("Layout name:"), 13.0, t.text));
                if ui.add(egui::TextEdit::singleline(&mut self.editor_title).desired_width(280.0)).changed() {
                    self.editor_dirty = true;
                }
            });
            ui.add_space(12.0);

            let Some(mut doc) = self.editor_doc.clone() else { return };
            let legacy = doc.legacy_map();
            let native_actions = doc.native_actions();
            let selected = self.editor_selected.unwrap_or(Control::A);

            ui.columns(2, |cols| {
                // Left pane: all physical controller inputs at full height (no nested scroll)
                editor_section_card(t).show(&mut cols[0], |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(w::caption_text(t, t!("SELECT A CONTROL TO REMAP")));
                    ui.add_space(8.0);
                    let groups: &[(&str, &[Control])] = &[
                        (t!("Face buttons"), &[Control::A, Control::B, Control::X, Control::Y]),
                        (t!("Bumpers & Triggers"), &[Control::LB, Control::RB, Control::LT, Control::RT]),
                        (
                            t!("D-pad & Menu"),
                            &[Control::Up, Control::Down, Control::Left, Control::Right, Control::Start, Control::Back],
                        ),
                        (
                            t!("Left stick"),
                            &[
                                Control::LeftStick,
                                Control::L3,
                                Control::LeftStickUp,
                                Control::LeftStickDown,
                                Control::LeftStickLeft,
                                Control::LeftStickRight,
                            ],
                        ),
                        (
                            t!("Right stick"),
                            &[
                                Control::RightStick,
                                Control::R3,
                                Control::RightStickUp,
                                Control::RightStickDown,
                                Control::RightStickLeft,
                                Control::RightStickRight,
                            ],
                        ),
                        (t!("Back paddles & Gyro"), &[Control::L4, Control::R4, Control::L5, Control::R5, Control::Gyro]),
                    ];
                    for (idx, (heading, ctrls)) in groups.iter().enumerate() {
                        if idx > 0 {
                            ui.add_space(8.0);
                        }
                        ui.label(w::text_semibold(*heading, 12.0, t.text_dim));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for &c in *ctrls {
                                let is_sel = selected == c;
                                let mapped = legacy.0.get(&c).and_then(|v| v.first()).map(Output::display_label);
                                if editor_control_tile(ui, t, c.name(), mapped.as_deref(), is_sel).clicked() {
                                    self.editor_selected = Some(c);
                                }
                            }
                        });
                    }
                });

                // Right pane: output target picker for the selected control (full height, no nested scroll)
                editor_section_card(t).show(&mut cols[1], |ui| {
                    ui.set_width(ui.available_width());
                    let current_out = legacy
                        .0
                        .get(&selected)
                        .and_then(|v| v.first())
                        .cloned()
                        .unwrap_or_else(|| default_output_for(selected));
                    let has_override = legacy.0.contains_key(&selected);

                    ui.label(w::caption_text(t, t!("REMAP TARGET")));
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(w::active_fill(t))
                        .stroke(Stroke::new(1.0, t.accent.gamma_multiply(0.55)))
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(w::text_bold(
                                t!("{control} → {target}", control = selected.name(), target = current_out.display_label()),
                                14.5,
                                t.text,
                            ));
                        });
                    ui.add_space(8.0);

                    let mut chosen_out: Option<Output> = None;
                    let mut clear_override = false;

                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        if editor_action_button(
                            ui,
                            t,
                            None,
                            t!("Unassigned"),
                            current_out == Output::Unassigned && has_override,
                        )
                        .clicked()
                        {
                            chosen_out = Some(Output::Unassigned);
                        }
                        if editor_action_button(ui, t, Some(icons::RESTORE), t!("Default"), !has_override).clicked() {
                            clear_override = true;
                        }
                    });
                    ui.add_space(10.0);

                    if selected.vector() {
                        ui.label(w::text_semibold(t!("Analog / Pointer output"), 12.0, t.text_dim));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for out in [Output::Pad(Control::LeftStick), Output::Pad(Control::RightStick), Output::MouseMove] {
                                if editor_keycap(ui, t, &out.display_label(), current_out == out, 88.0).clicked() {
                                    chosen_out = Some(out);
                                }
                            }
                        });
                    } else {
                        ui.label(w::text_semibold(t!("Gamepad button / trigger"), 12.0, t.text_dim));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for &c in Control::ALL.iter().filter(|c| c.pad_output() && !c.vector()) {
                                let out = Output::Pad(c);
                                if editor_keycap(ui, t, c.name(), current_out == out, 42.0).clicked() {
                                    chosen_out = Some(out);
                                }
                            }
                        });
                        ui.add_space(10.0);

                        ui.label(w::text_semibold(t!("Mouse"), 12.0, t.text_dim));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for out in [
                                Output::MouseButton(1),
                                Output::MouseButton(2),
                                Output::MouseButton(3),
                                Output::MouseButton(4),
                                Output::MouseButton(5),
                                Output::Wheel(1),
                                Output::Wheel(-1),
                            ] {
                                if editor_keycap(ui, t, &out.display_label(), current_out == out, 64.0).clicked() {
                                    chosen_out = Some(out);
                                }
                            }
                        });
                        ui.add_space(10.0);

                        ui.label(w::text_semibold(t!("Keyboard"), 12.0, t.text_dim));
                        ui.add_space(4.0);
                        for (sub, keys) in [
                            (t!("Letters"), EDITOR_KEYS_LETTERS),
                            (t!("Modifiers & Navigation"), EDITOR_KEYS_MODIFIERS),
                            (t!("Numbers"), EDITOR_KEYS_NUMBERS),
                            (t!("Function keys"), EDITOR_KEYS_FUNCTION),
                        ] {
                            ui.label(w::text(sub, 11.0, t.text_faint));
                            ui.add_space(2.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                                for &vk in keys {
                                    let out = Output::Key(vk);
                                    if editor_keycap(ui, t, &client::controller_key_name(vk), current_out == out, 36.0)
                                        .clicked()
                                    {
                                        chosen_out = Some(out);
                                    }
                                }
                            });
                            ui.add_space(6.0);
                        }

                        if !native_actions.is_empty() {
                            ui.add_space(4.0);
                            ui.label(w::text_semibold(t!("Game actions"), 12.0, t.text_dim));
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                                for (set, act) in native_actions.iter().take(32) {
                                    let out = Output::Native(set.clone(), act.clone());
                                    if editor_keycap(ui, t, &format!("{set} · {act}"), current_out == out, 64.0).clicked() {
                                        chosen_out = Some(out);
                                    }
                                }
                            });
                        }
                    }

                    if clear_override {
                        doc.clear_binding(selected);
                        self.editor_dirty = true;
                    } else if let Some(out) = chosen_out {
                        doc.set_binding(selected, &out);
                        self.editor_dirty = true;
                    }
                });
            });

            doc.set_title(&self.editor_title);
            self.editor_doc = Some(doc.clone());

            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                if w::button(ui, t, Some(icons::CONTROLLER), t!("Save & Apply"), w::Kind::Primary, w::Size::Sm).clicked() {
                    match client::save_custom_layout(account, app_id, &doc) {
                        Ok(choice) => {
                            self.editor_dirty = false;
                            client::set_app_binding(app_id, choice.clone());
                            self.inspect_now(ctx, app_id, install_dir, &choice);
                            ctx.toast(t!("Custom controller layout saved and applied."));
                        }
                        Err(err) => {
                            ctx.toast(t!("Could not save layout: {err}", err = err));
                        }
                    }
                }
                if editor_action_button(ui, t, Some(icons::FOLDER), t!("Export .vdf…"), false).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("aurora_{app_id}.vdf"))
                        .add_filter("Steam Input VDF", &["vdf"])
                        .save_file()
                    {
                        match std::fs::write(&path, doc.write()) {
                            Ok(()) => ctx.toast(t!("Exported controller layout.")),
                            Err(err) => ctx.toast(t!("Could not export layout: {err}", err = err)),
                        }
                    }
                }
                if self.editor_dirty {
                    w::pill(ui, t!("Unsaved changes"), t.away, t.away.gamma_multiply(0.14));
                }
            });
        });
    }

    fn inspect_now(&mut self, ctx: &Ctx, app_id: AppId, install_dir: &PathBuf, choice: &BindingChoice) {
        let account = (ctx.state.steam_id & 0xFFFF_FFFF) as u32;
        self.report = Some(client::inspect_input_config_local(&ctx.local.steam_root, account, app_id, install_dir));
        self.inspected_app = Some(app_id);
        self.inspected_choice = Some(choice.clone());
        if ctx.state.phase == client::state::Phase::LoggedIn && ctx.state.link == client::Link::Online {
            let key = self.next_request(account, app_id);
            self.inspect_req = Some(key);
            self.inspector_loading = true;
            ctx.send(Command::InspectInputConfig { key, install_dir: install_dir.clone() });
        } else {
            self.inspect_req = None;
            self.inspector_loading = false;
        }
    }
}

fn steam_input_families_ui(ui: &mut Ui, t: &Theme, cfg: &mut ControllerSettings) {
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(w::text_semibold(t!("Steam Input"), 16.0, t.text));
        ui.add(
            egui::Label::new(w::text(
                t!("Choose which controller families Aurora handles through Steam Input and translates for games that expect Xbox controllers."),
                12.5,
                t.text_dim,
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        switch(
            ui,
            t,
            t!("Enable Steam Input for Xbox controllers"),
            t!("Use Aurora's Steam Input action mapping for Xbox controllers when a game uses Steam Input."),
            &mut cfg.families.xbox,
        );
        switch(
            ui,
            t,
            t!("Enable Steam Input for PlayStation controllers"),
            t!("Read DualShock and DualSense controllers (including gyro) and expose them to games."),
            &mut cfg.families.playstation,
        );
        switch(
            ui,
            t,
            t!("Enable Steam Input for Switch Pro controllers"),
            t!("Read Nintendo Switch Pro and Joy-Con controllers and expose them to games."),
            &mut cfg.families.switch_pro,
        );
        switch(
            ui,
            t,
            t!("Enable Steam Input for generic controllers"),
            t!("Read DirectInput and third-party gamepads as Xbox-style controllers."),
            &mut cfg.families.generic,
        );
    });
}

fn calibration_ui(ui: &mut Ui, t: &Theme, cal: &mut PadCalibration) {
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(w::text_semibold(t!("Calibration & Deadzones"), 16.0, t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::RESTORE), t!("Reset to defaults"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    *cal = PadCalibration::default();
                }
            });
        });
        ui.add_space(8.0);

        egui::Grid::new("controller-calibration-grid")
            .num_columns(2)
            .spacing(vec2(24.0, 10.0))
            .show(ui, |ui| {
                ui.label(w::text_medium(t!("Left stick deadzone"), 13.5, t.text));
                ui.add(
                    egui::Slider::new(&mut cal.left_deadzone, 0.0..=0.50)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.end_row();

                ui.label(w::text_medium(t!("Right stick deadzone"), 13.5, t.text));
                ui.add(
                    egui::Slider::new(&mut cal.right_deadzone, 0.0..=0.50)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.end_row();

                ui.label(w::text_medium(t!("Trigger threshold"), 13.5, t.text));
                ui.add(
                    egui::Slider::new(&mut cal.trigger_threshold, 0.0..=0.50)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.end_row();

                ui.label(w::text_medium(t!("Gyro sensitivity"), 13.5, t.text));
                ui.add_enabled(
                    cal.gyro_enabled,
                    egui::Slider::new(&mut cal.gyro_sensitivity, 0.1..=4.0)
                        .step_by(0.05)
                        .custom_formatter(|v, _| format!("{v:.2}×")),
                );
                ui.end_row();
            });

        ui.add_space(10.0);
        switch(
            ui,
            t,
            t!("Enable gyroscope & motion sensors"),
            t!("Forward controller gyro and accelerometer data to games that request motion."),
            &mut cal.gyro_enabled,
        );
        switch(
            ui,
            t,
            t!("Controller vibration (rumble)"),
            t!("Allow games and input tests to trigger controller vibration motors."),
            &mut cal.rumble_enabled,
        );
        switch(
            ui,
            t,
            t!("Use Nintendo button layout"),
            t!("Swap A/B and X/Y face buttons to match Nintendo Switch controller labels."),
            &mut cal.nintendo_layout,
        );
    });
}

fn family_label(f: ControllerFamily) -> &'static str {
    match f {
        ControllerFamily::Xbox => t!("Xbox"),
        ControllerFamily::PlayStation => t!("PlayStation"),
        ControllerFamily::SwitchPro => t!("Switch Pro"),
        ControllerFamily::Generic => t!("Generic"),
    }
}

fn controller_status_line(c: &DetectedController) -> String {
    let slot = match c.slot {
        Some(s) => t!("Slot {n}", n = s + 1),
        None => t!("Standby").to_string(),
    };
    let conn = match c.connection {
        ConnectionKind::Wired => t!("Wired"),
        ConnectionKind::Wireless => t!("Wireless"),
        ConnectionKind::Unknown => t!("Connected"),
    };
    let mut parts = vec![slot, conn.to_string()];
    if let Some(pct) = c.battery_percent {
        parts.push(t!("Battery {pct}%", pct = pct));
    }
    if c.vendor_id != 0 || c.product_id != 0 {
        parts.push(format!("{:04X}:{:04X}", c.vendor_id, c.product_id));
    }
    parts.join("  ·  ")
}

const EDITOR_KEYS_LETTERS: &[u16] = &[
    b'W' as u16, b'A' as u16, b'S' as u16, b'D' as u16,
    b'E' as u16, b'R' as u16, b'F' as u16, b'Q' as u16,
    b'C' as u16, b'V' as u16, b'G' as u16, b'X' as u16,
    b'Z' as u16, b'T' as u16, b'I' as u16, b'M' as u16,
];

const EDITOR_KEYS_MODIFIERS: &[u16] = &[
    0x20, 0x10, 0x11, 0x12, 0x09, 0x0D, 0x1B, 0x08,
    0x26, 0x28, 0x25, 0x27,
];

const EDITOR_KEYS_NUMBERS: &[u16] = &[
    b'1' as u16, b'2' as u16, b'3' as u16, b'4' as u16, b'5' as u16,
    b'6' as u16, b'7' as u16, b'8' as u16, b'9' as u16, b'0' as u16,
];

const EDITOR_KEYS_FUNCTION: &[u16] = &[
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B,
];

fn editor_button_border(t: &Theme) -> egui::Color32 {
    w::mix(t.border, t.text_faint, 0.45)
}

fn editor_workspace_frame(t: &Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(t.chrome)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.corner_small())
        .inner_margin(Margin::same(14))
}

fn editor_section_card(t: &Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.corner_small())
        .inner_margin(Margin::same(12))
}

fn editor_action_button(ui: &mut Ui, t: &Theme, icon: Option<&str>, label: &str, selected: bool) -> egui::Response {
    let text = match icon {
        Some(i) => format!("{i}  {label}"),
        None => label.to_string(),
    };
    editor_keycap(ui, t, &text, selected, 0.0)
}

fn editor_keycap(ui: &mut Ui, t: &Theme, label: &str, selected: bool, min_width: f32) -> egui::Response {
    let font = if selected { w::font_semibold(12.5) } else { w::font_medium(12.5) };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, egui::Color32::PLACEHOLDER);
    let width = (galley.size().x + 20.0).max(min_width);
    let height = 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.10);
        let pressed = resp.is_pointer_button_down_on();
        let base_fill = w::mix(t.raised, t.surface_hover, 0.28);
        let mut fill = if selected {
            w::mix(t.accent, egui::Color32::WHITE, hover * 0.08)
        } else {
            w::mix(base_fill, t.surface_hover, hover)
        };
        if pressed {
            fill = w::mix(fill, egui::Color32::BLACK, 0.12);
        }
        let border = if selected {
            t.accent
        } else {
            w::mix(editor_button_border(t), t.accent, hover * 0.7)
        };
        let fg = if selected { t.on_accent } else { t.text };
        let radius = CornerRadius::same(6);
        let p = ui.painter();
        p.rect_filled(rect, radius, fill);
        p.rect_stroke(rect, radius, Stroke::new(if selected { 1.4 } else { 1.0 }, border), StrokeKind::Inside);
        p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn editor_control_tile(
    ui: &mut Ui,
    t: &Theme,
    control_name: &str,
    mapped: Option<&str>,
    selected: bool,
) -> egui::Response {
    let name_galley = ui.painter().layout_no_wrap(
        control_name.to_string(),
        w::font_semibold(12.5),
        egui::Color32::PLACEHOLDER,
    );
    let map_galley = mapped.map(|m| {
        ui.painter().layout_no_wrap(format!("→ {m}"), w::font_medium(11.5), egui::Color32::PLACEHOLDER)
    });
    let gap = if map_galley.is_some() { 8.0 } else { 0.0 };
    let content_w = name_galley.size().x + gap + map_galley.as_ref().map_or(0.0, |g| g.size().x + 12.0);
    let width = (content_w + 22.0).max(56.0);
    let height = 32.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.10);
        let pressed = resp.is_pointer_button_down_on();
        let has_map = map_galley.is_some();
        let base_fill = if has_map {
            w::mix(t.raised, t.accent, 0.10)
        } else {
            w::mix(t.raised, t.surface_hover, 0.28)
        };
        let mut fill = if selected {
            w::mix(t.accent, egui::Color32::WHITE, hover * 0.08)
        } else {
            w::mix(base_fill, t.surface_hover, hover)
        };
        if pressed {
            fill = w::mix(fill, egui::Color32::BLACK, 0.12);
        }
        let border = if selected {
            t.accent
        } else if has_map {
            w::mix(t.accent.gamma_multiply(0.55), t.accent, hover * 0.6)
        } else {
            w::mix(editor_button_border(t), t.accent, hover * 0.7)
        };
        let radius = CornerRadius::same(7);
        let p = ui.painter();
        p.rect_filled(rect, radius, fill);
        p.rect_stroke(rect, radius, Stroke::new(if selected { 1.5 } else { 1.0 }, border), StrokeKind::Inside);

        let name_fg = if selected { t.on_accent } else { t.text };
        if let Some(mg) = map_galley {
            let x = rect.left() + 11.0;
            p.galley(pos2(x, rect.center().y - name_galley.size().y / 2.0), name_galley, name_fg);
            let badge_left = x + content_w - (mg.size().x + 12.0);
            let badge_rect = Rect::from_min_size(
                pos2(badge_left, rect.center().y - 10.0),
                vec2(mg.size().x + 12.0, 20.0),
            );
            let badge_bg = if selected {
                egui::Color32::from_black_alpha(55)
            } else {
                t.accent.gamma_multiply(0.16)
            };
            let badge_fg = if selected { t.on_accent } else { t.accent };
            p.rect_filled(badge_rect, CornerRadius::same(5), badge_bg);
            p.galley(badge_rect.center() - mg.size() / 2.0, mg, badge_fg);
        } else {
            p.galley(rect.center() - name_galley.size() / 2.0, name_galley, name_fg);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn builtin_template_short_label(tmpl: BuiltInTemplate) -> &'static str {
    match tmpl {
        BuiltInTemplate::Gamepad => t!("Gamepad"),
        BuiltInTemplate::Camera => t!("Gamepad + Gyro"),
        BuiltInTemplate::Keyboard => t!("WASD & Mouse"),
        BuiltInTemplate::Precision => t!("Precision Mouse"),
    }
}

fn default_output_for(c: Control) -> Output {
    if c.pad_output() {
        Output::Pad(c)
    } else {
        Output::Unassigned
    }
}

fn choice_label(choice: &BindingChoice, report: &InputConfigReport) -> String {
    if let Some(opt) = report.available.iter().find(|o| &o.choice == choice) {
        return option_label(opt);
    }
    match choice {
        BindingChoice::Auto => t!("Automatic (default)").to_string(),
        BindingChoice::LocalFile(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        BindingChoice::Workshop(id) => t!("Workshop #{id}", id = id),
        BindingChoice::Template(TemplateChoice::BuiltIn(tmpl)) => builtin_template_short_label(*tmpl).to_string(),
        BindingChoice::Template(TemplateChoice::Installed(_)) => t!("Template").to_string(),
        BindingChoice::Community(id) => t!("Community #{id}", id = id),
        BindingChoice::Custom(_) => t!("Custom layout").to_string(),
    }
}

fn option_label(opt: &client::BindingOption) -> String {
    match opt.origin {
        BindingOrigin::UserFile => t!("User: {name}", name = opt.label.strip_prefix("User: ").unwrap_or(&opt.label)),
        BindingOrigin::UserWorkshop(id) => t!("Your Workshop #{id}", id = id),
        BindingOrigin::GameFile => t!("Game: {name}", name = opt.label.strip_prefix("Game: ").unwrap_or(&opt.label)),
        BindingOrigin::Official(id) if opt.controller_type.is_empty() => t!("Official Workshop #{id}", id = id),
        BindingOrigin::Official(id) => format!("{} ({})", t!("Official Workshop #{id}", id = id), opt.controller_type),
        BindingOrigin::CustomFile => t!("Custom: {name}", name = opt.label.strip_prefix("Custom: ").unwrap_or(&opt.label)),
        BindingOrigin::Template => t!("Template: {name}", name = &opt.label),
        BindingOrigin::Community(id) if opt.label.is_empty() => t!("Community #{id}", id = id),
        BindingOrigin::Community(_) => t!("Community: {name}", name = &opt.label),
        BindingOrigin::Custom => t!("Custom: {name}", name = &opt.label),
    }
}

fn available_games(ctx: &Ctx) -> Vec<(AppId, String, PathBuf)> {
    let mut list: Vec<(AppId, String, PathBuf, bool)> = Vec::new();
    for g in &ctx.local.games {
        list.push((g.app_id, g.name.clone(), g.install_dir.clone(), true));
    }
    if let Some(apps) = &ctx.state.apps {
        for a in apps.values().filter(|a| !a.is_dlc() && !a.name.is_empty()) {
            if !list.iter().any(|(id, _, _, _)| *id == a.app_id) {
                list.push((a.app_id, a.name.clone(), PathBuf::new(), false));
            }
        }
    }
    list.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    list.into_iter().map(|(id, name, dir, _)| (id, name, dir)).collect()
}

// ------------------------------------------------------------ Live visualizer widgets

fn stick_pad(ui: &mut Ui, t: &Theme, label: &str, raw_x: f32, raw_y: f32, cal_x: f32, cal_y: f32, deadzone: f32, clicked: bool) {
    ui.label(w::text_medium(label, 12.5, t.text_dim));
    let radius = 44.0;
    let (rect, _) = ui.allocate_exact_size(vec2(radius * 2.0 + 10.0, radius * 2.0 + 10.0), Sense::hover());
    let c = rect.center();
    let p = ui.painter();

    p.circle_filled(c, radius, t.raised);
    p.circle_stroke(c, radius, Stroke::new(1.5, if clicked { t.accent } else { t.border }));
    // Axes crosshair
    p.line_segment([pos2(c.x - radius, c.y), pos2(c.x + radius, c.y)], Stroke::new(1.0, t.border));
    p.line_segment([pos2(c.x, c.y - radius), pos2(c.x, c.y + radius)], Stroke::new(1.0, t.border));
    // Deadzone ring
    let dz_r = radius * deadzone.clamp(0.0, 0.95);
    if dz_r > 1.0 {
        p.circle_filled(c, dz_r, t.accent.gamma_multiply(0.10));
        p.circle_stroke(c, dz_r, Stroke::new(1.0, t.accent.gamma_multiply(0.55)));
    }
    // Raw stick dot (subtle)
    let raw_pos = pos2(c.x + raw_x.clamp(-1.0, 1.0) * radius, c.y - raw_y.clamp(-1.0, 1.0) * radius);
    p.circle_filled(raw_pos, 3.5, t.text_faint);
    // Calibrated stick dot
    let cal_pos = pos2(c.x + cal_x.clamp(-1.0, 1.0) * radius, c.y - cal_y.clamp(-1.0, 1.0) * radius);
    p.circle_filled(cal_pos, 6.0, if clicked { t.play } else { t.accent });

    ui.label(w::text(format!("{cal_x:+.2}, {cal_y:+.2}"), 11.5, t.text_dim).monospace());
    button_pill(ui, t, if label == t!("Left stick") { "LS" } else { "RS" }, clicked);
}

fn trigger_bar(ui: &mut Ui, t: &Theme, label: &str, raw: f32, cal: f32, threshold: f32) {
    ui.horizontal(|ui| {
        ui.label(w::text_medium(label, 12.0, t.text_dim));
        let (rect, _) = ui.allocate_exact_size(vec2(110.0, 14.0), Sense::hover());
        let p = ui.painter();
        let r = CornerRadius::same(4);
        p.rect_filled(rect, r, t.raised);
        if raw > 0.0 {
            let w_raw = rect.width() * raw.clamp(0.0, 1.0);
            p.rect_filled(Rect::from_min_size(rect.min, vec2(w_raw, rect.height())), r, t.text_faint.gamma_multiply(0.35));
        }
        if cal > 0.0 {
            let w_cal = rect.width() * cal.clamp(0.0, 1.0);
            p.rect_filled(Rect::from_min_size(rect.min, vec2(w_cal, rect.height())), r, t.accent);
        }
        let th_x = rect.left() + rect.width() * threshold.clamp(0.0, 0.95);
        p.line_segment([pos2(th_x, rect.top()), pos2(th_x, rect.bottom())], Stroke::new(1.5, t.away));
        p.rect_stroke(rect, r, Stroke::new(1.0, t.border), StrokeKind::Inside);
        ui.label(w::text(format!("{:.0}%", cal * 100.0), 11.5, t.text_dim).monospace());
    });
}

fn bumper_pill(ui: &mut Ui, t: &Theme, label: &str, active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(88.0, 22.0), Sense::hover());
    paint_key_box(ui.painter(), t, rect, label, active, CornerRadius::same(6));
}

fn button_pill(ui: &mut Ui, t: &Theme, label: &str, active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(52.0, 22.0), Sense::hover());
    paint_key_box(ui.painter(), t, rect, label, active, CornerRadius::same(5));
}

fn dpad_cross(ui: &mut Ui, t: &Theme, up: bool, down: bool, left: bool, right: bool) {
    let sz = 22.0;
    let (rect, _) = ui.allocate_exact_size(vec2(sz * 3.0, sz * 3.0), Sense::hover());
    let p = ui.painter();
    let cell = |gx: f32, gy: f32| Rect::from_min_size(pos2(rect.left() + gx * sz, rect.top() + gy * sz), vec2(sz - 2.0, sz - 2.0));
    paint_key_box(p, t, cell(1.0, 0.0), "▲", up, CornerRadius::same(4));
    paint_key_box(p, t, cell(1.0, 2.0), "▼", down, CornerRadius::same(4));
    paint_key_box(p, t, cell(0.0, 1.0), "◀", left, CornerRadius::same(4));
    paint_key_box(p, t, cell(2.0, 1.0), "▶", right, CornerRadius::same(4));
    p.rect_filled(cell(1.0, 1.0), CornerRadius::same(3), t.raised);
}

fn face_diamond(
    ui: &mut Ui,
    t: &Theme,
    south: (&str, bool),
    east: (&str, bool),
    west: (&str, bool),
    north: (&str, bool),
) {
    let sz = 26.0;
    let (rect, _) = ui.allocate_exact_size(vec2(sz * 3.0, sz * 3.0), Sense::hover());
    let p = ui.painter();
    let dot = |gx: f32, gy: f32, (lbl, on): (&str, bool)| {
        let c = pos2(rect.left() + (gx + 0.5) * sz, rect.top() + (gy + 0.5) * sz);
        let r = sz * 0.44;
        p.circle_filled(c, r, if on { t.accent } else { t.raised });
        p.circle_stroke(c, r, Stroke::new(1.2, if on { t.accent } else { t.border }));
        p.text(c, Align2::CENTER_CENTER, lbl, w::font_bold(11.5), if on { t.on_accent } else { t.text });
    };
    dot(1.0, 0.0, north);
    dot(2.0, 1.0, east);
    dot(0.0, 1.0, west);
    dot(1.0, 2.0, south);
}

fn paint_key_box(p: &egui::Painter, t: &Theme, rect: Rect, label: &str, active: bool, radius: CornerRadius) {
    p.rect_filled(rect, radius, if active { t.accent } else { t.raised });
    p.rect_stroke(rect, radius, Stroke::new(1.0, if active { t.accent } else { t.border }), StrokeKind::Inside);
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        w::font_semibold(11.0),
        if active { t.on_accent } else { t.text_dim },
    );
}

fn motion_axis_group(ui: &mut Ui, t: &Theme, title: &str, vals: [f32; 3], scale: f32) {
    ui.label(w::text_medium(title, 12.0, t.text_dim));
    for (name, val) in [("X", vals[0]), ("Y", vals[1]), ("Z", vals[2])] {
        ui.horizontal(|ui| {
            ui.label(w::text(name, 11.5, t.text_faint).monospace());
            let (rect, _) = ui.allocate_exact_size(vec2(140.0, 10.0), Sense::hover());
            let p = ui.painter();
            let r = CornerRadius::same(3);
            p.rect_filled(rect, r, t.surface);
            let norm = (val / scale).clamp(-1.0, 1.0);
            let mid = rect.center().x;
            let bar = if norm >= 0.0 {
                Rect::from_min_max(pos2(mid, rect.top()), pos2(mid + rect.width() * 0.5 * norm, rect.bottom()))
            } else {
                Rect::from_min_max(pos2(mid + rect.width() * 0.5 * norm, rect.top()), pos2(mid, rect.bottom()))
            };
            p.rect_filled(bar, r, t.accent);
            p.line_segment([pos2(mid, rect.top()), pos2(mid, rect.bottom())], Stroke::new(1.0, t.border));
            ui.label(w::text(format!("{val:+.2}"), 11.5, t.text_dim).monospace());
        });
    }
}

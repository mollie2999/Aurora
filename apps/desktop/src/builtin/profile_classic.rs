//! Aurora's classic desktop profile layout.

use super::{Profile, Section, badge_icon, level_badge, open_badge, panel};
use client::Command;
use egui::{Align, Align2, Color32, CornerRadius, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::{Action, Ctx};
use i18n::t;
use model::{SteamId, Visibility, art};
use ui::{Theme, icons, widgets as w};

impl Profile {
    pub(super) fn classic_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, open: bool) {
        let mine = id == ctx.state.steam_id;
        let persona = ctx.state.persona(id);
        let info = ctx.state.profiles.get(&id).cloned();
        let visibility = info.as_ref().map(|i| i.visibility).unwrap_or_default();
        let display_name = if mine && !ctx.state.persona_name.is_empty() { ctx.state.persona_name.clone() } else { persona.name.clone() };

        // Full-page background, like a Steam profile.
        let full = ui.max_rect();
        let bg_url = info.as_ref().and_then(|i| i.background.clone());
        let painted = bg_url.as_deref().is_some_and(|u| w::paint_cover(ui, full, u, CornerRadius::ZERO));
        if painted {
            ui.painter().rect_filled(full, CornerRadius::ZERO, Color32::from_black_alpha(120));
        } else {
            w::gradient(ui.painter(), Rect::from_min_size(full.min, vec2(full.width(), 340.0)), t.accent.gamma_multiply(0.30), t.accent_2.gamma_multiply(0.18), true);
            w::fade_down(ui, Rect::from_min_size(full.min, vec2(full.width(), 340.0)), t.bg, 0.0);
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(40.0);
            w::centered(ui, 1180.0, 32.0, |ui| {
                match self.section {
                    Section::Profile => {}
                    Section::Games => return self.games_page(ui, t, ctx, id, &display_name, mine),
                    Section::Edit => return self.edit_page(ui, t, ctx),
                    other => return self.list_page(ui, t, ctx, id, &display_name, mine, other),
                }
                // Identity header on a translucent panel.
                egui::Frame::new()
                    .fill(t.chrome.gamma_multiply(0.82))
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(t.corner())
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(164.0, 164.0), Sense::hover());
                            let img = r.shrink(14.0);
                            let still = art::avatar_full(persona.avatar_hash.as_deref());
                            let avatar_url = info.as_ref().and_then(|i| i.animated_avatar.as_deref()).unwrap_or(&still);
                            if !w::paint_cover(ui, img, avatar_url, CornerRadius::same(6)) {
                                w::skeleton(ui, t, img, CornerRadius::same(6));
                            }
                            if let Some(frame) = info.as_ref().and_then(|i| i.avatar_frame.as_deref()) {
                                w::paint_cover(ui, r, frame, CornerRadius::ZERO);
                            }
                            ui.add_space(12.0);
                            ui.vertical(|ui| {
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.label(w::text_bold(&display_name, 30.0, Color32::WHITE));
                                    if mine {
                                        if let Some(level) = ctx.state.level {
                                            level_badge(ui, t, level);
                                        }
                                    }
                                });
                                if let Some(i) = &info {
                                    let mut sub = Vec::new();
                                    if !i.real_name.is_empty() {
                                        sub.push(i.real_name.clone());
                                    }
                                    if !i.location.is_empty() {
                                        sub.push(i.location.clone());
                                    }
                                    if !sub.is_empty() {
                                        ui.label(w::text(sub.join("  ·  "), 14.0, t.text_dim));
                                    }
                                } else {
                                    let (r, _) = ui.allocate_exact_size(vec2(220.0, 14.0), Sense::hover());
                                    w::skeleton(ui, t, r, CornerRadius::same(4));
                                }
                                let r = ui.add(egui::Label::new(w::text_medium(crate::shell::status_text(ctx.state, &persona), 14.0, crate::shell::status_color(t, &persona))).sense(Sense::click()));
                                if let Some(game) = persona.game_app_id.filter(|_| persona.in_game()) {
                                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        ctx.act(crate::shell::game_page(ctx.state, game));
                                    }
                                }
                                ui.add_space(10.0);
                                match info.as_ref().map(|i| i.summary.trim().to_string()) {
                                    Some(s) if !s.is_empty() => {
                                        w::markup_label(ui, &s, 14.0, t.text);
                                    }
                                    Some(_) => {
                                        ui.label(w::text(t!("No profile summary yet."), 13.5, t.text_faint));
                                    }
                                    None => {
                                        for wdt in [380.0, 300.0] {
                                            let (r, _) = ui.allocate_exact_size(vec2(wdt, 12.0), Sense::hover());
                                            w::skeleton(ui, t, r, CornerRadius::same(4));
                                        }
                                    }
                                }
                                if let Some(since) = info.as_ref().and_then(|i| i.member_since) {
                                    ui.add_space(6.0);
                                    ui.label(w::text(t!("Member since {date}", date = ui::date(since)), 12.5, t.text_faint)).on_hover_text(ui::ago(since));
                                }
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    if !mine && w::button(ui, t, Some(icons::CHAT), t!("Send a message"), w::Kind::Primary, w::Size::Sm).clicked() {
                                        ctx.act(Action::OpenChat(id));
                                    }
                                    if mine {
                                        if w::button(ui, t, Some(icons::NOTE), t!("Edit profile"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                            ctx.navigate("profile", Some("edit".into()));
                                        }
                                    } else if w::button(ui, t, Some(icons::EXTERNAL), t!("View on Steam"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                        crate::links::open_http(&format!("https://steamcommunity.com/profiles/{id}"));
                                    }
                                    if w::button(ui, t, Some(icons::COPY), t!("Copy SteamID"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                        ui.ctx().copy_text(id.to_string());
                                        ctx.toast(t!("SteamID copied"));
                                    }
                                });
                            });
                        });
                    });
                ui.add_space(16.0);

                // Two columns, like Steam: showcases on the left, links and
                // friends on the right.
                let side = 320.0;
                let gap = 18.0;
                let main_w = ui.available_width() - side - gap;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.allocate_ui_with_layout(vec2(main_w, 0.0), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(main_w);
                        if persona.in_game() {
                            let game = persona.game_name.clone().or_else(|| persona.game_app_id.and_then(|g| ctx.state.app_name(g).map(String::from))).unwrap_or_default();
                            panel(ui, t, |ui| {
                                ui.horizontal(|ui| {
                                    if let Some(g) = persona.game_app_id {
                                        let (r, _) = ui.allocate_exact_size(vec2(184.0, 69.0), Sense::hover());
                                        w::image_or_skeleton(ui, t, r, &art::header(g), t.corner_small());
                                    }
                                    ui.vertical(|ui| {
                                        ui.label(w::text_semibold(w::sentence_case(t!("CURRENTLY IN GAME")), 12.0, t.in_game));
                                        ui.label(w::text_semibold(game, 18.0, t.text));
                                    });
                                });
                            });
                            ui.add_space(14.0);
                        }
                        if open {
                            self.recent_games(ui, t, ctx, id);
                            ui.add_space(14.0);
                            self.showcase(ui, t, ctx, id);
                            ui.add_space(14.0);
                            self.most_played(ui, t, ctx, id);
                            ui.add_space(14.0);
                            self.comments(ui, t, ctx, id, mine);
                        } else {
                            panel(ui, t, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(w::text(icons::LOCK_SIMPLE, 26.0, t.text_dim));
                                    ui.add_space(6.0);
                                    ui.vertical(|ui| {
                                        let title = if visibility == Visibility::FriendsOnly { t!("This profile is visible to friends only") } else { t!("This profile is private") };
                                        ui.label(w::text_bold(title, 16.0, t.text));
                                        ui.label(w::text(t!("Games, inventory and friends are hidden by its owner."), 13.0, t.text_dim));
                                    });
                                });
                            });
                        }
                    });
                    ui.allocate_ui_with_layout(vec2(side, 0.0), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(side);
                        self.badges(ui, t, ctx, id, open);
                        self.sidebar(ui, t, ctx, id, mine, open);
                    });
                });
            });
            ui.add_space(40.0);
        });
    }

    /// Level, XP and badges, like the top of Steam's profile sidebar.
    fn badges(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, open: bool) {
        if !open {
            return;
        }
        self.request_social(ctx, id);
        let Some(Ok(b)) = ctx.state.badges.get(&id).cloned() else { return };
        panel(ui, t, |ui| {
            ui.horizontal(|ui| {
                level_badge(ui, t, b.level);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(w::text_bold(t!("Level {n}", n = b.level), 16.0, t.text));
                    if !b.xp.is_empty() {
                        ui.label(w::text(&b.xp, 12.0, t.text_dim));
                    }
                });
            });
            if !b.next.is_empty() {
                ui.add_space(4.0);
                let width = ui.available_width();
                w::progress(ui, t, b.progress, width);
                ui.label(w::text(&b.next, 11.5, t.text_faint));
            }
            if !b.items.is_empty() {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(w::text_bold(t!("Badges"), 14.0, t.text));
                    ui.label(w::text(b.items.len().to_string(), 13.0, t.text_dim));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.link(w::text(t!("See all"), 12.5, t.accent)).clicked() {
                            ctx.navigate("profile", Some(format!("{id}/{}", Section::Badges.slug())));
                        }
                    });
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for badge in b.items.iter().take(8) {
                        if badge_icon(ui, t, badge, 54.0) {
                            open_badge(ctx, id, badge);
                        }
                    }
                });
            }
        });
        ui.add_space(12.0);
    }

    /// A few items from the Steam inventory, like Steam's item showcase.
    fn showcase(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId) {
        if self.classic_inventory_requested.insert(id) {
            ctx.send(Command::LoadInventory { steam_id: id, app_id: 753, context_id: 6, language: i18n::language().steam_name().into() });
        }
        let Some(inv) = ctx.state.inventories.get(&(id, 753, 6)) else { return };
        if inv.items.is_empty() || inv.error.is_some() {
            return;
        }
        // Prefer backgrounds and emoticons, then anything colorful.
        let mut items: Vec<&model::InventoryItem> = inv.items.iter().collect();
        items.sort_by_key(|i| std::cmp::Reverse((i.kind.to_lowercase().contains("background") as u8, i.marketable as u8)));
        panel(ui, t, |ui| {
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("Item showcase"), 17.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.link(w::text(t!("Open inventory"), 12.5, t.accent)).clicked() {
                        ctx.navigate("inventory", Some(id.to_string()));
                    }
                });
            });
            ui.add_space(8.0);
            let size = 84.0;
            let cols = (((ui.available_width() + 8.0) / (size + 8.0)).floor() as usize).max(1);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for item in items.iter().take(cols) {
                    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
                    ui.painter().rect_filled(r, t.corner_small(), t.raised);
                    if !w::paint_contain(ui, r.shrink(6.0), &art::econ(&item.icon, 192), Align2::CENTER_CENTER) {
                        w::skeleton(ui, t, r.shrink(6.0), CornerRadius::same(4));
                    }
                    if let Some(c) = crate::builtin::inventory::rarity_color(item) {
                        let bar = Rect::from_min_max(pos2(r.left() + 8.0, r.bottom() - 5.0), pos2(r.right() - 8.0, r.bottom() - 2.5));
                        ui.painter().rect_filled(bar, CornerRadius::same(2), c);
                    }
                    if resp.on_hover_text(format!("{}\n{}", item.name, item.kind)).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        ctx.navigate("inventory", Some(format!("{id}/753")));
                    }
                }
            });
        });
    }

    /// Most played games by total hours.
    fn most_played(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId) {
        let mut games: Vec<(model::AppId, String, u64)> = match ctx.state.player_games.get(&id) {
            Some(Ok(g)) if !g.is_empty() => g.iter().map(|g| (g.app_id, g.name.clone(), g.playtime_minutes)).collect(),
            _ if id == ctx.state.steam_id => ctx.local.games.iter().map(|g| (g.app_id, g.name.clone(), g.playtime_minutes.unwrap_or(0))).collect(),
            _ => return,
        };
        games.retain(|g| g.2 > 0);
        if games.is_empty() {
            return;
        }
        games.sort_by_key(|g| std::cmp::Reverse(g.2));
        let max = games[0].2.max(1) as f32;
        panel(ui, t, |ui| {
            ui.label(w::text_bold(t!("Most played"), 17.0, t.text));
            ui.add_space(6.0);
            for (app, name, minutes) in games.iter().take(6) {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(68.0, 32.0), Sense::hover());
                    w::image_or_skeleton(ui, t, r, &art::header(*app), CornerRadius::same(4));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(w::text_medium(name, 13.5, t.text)).truncate());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(w::text(ui::playtime(*minutes), 12.5, t.text_dim));
                            });
                        });
                        let width = ui.available_width();
                        w::progress(ui, t, *minutes as f32 / max, width);
                    });
                });
                ui.add_space(4.0);
            }
        });
    }
}

//! A profile's badges, reviews, groups and card collections, each as its own
//! list (like the games list), from the profile's pages on steamcommunity.com.

use super::{Profile, Section, badge_icon, card_set_of, level_badge, open_badge, panel};
use client::Command;
use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Ui, pos2, vec2};
use extensions::Ctx;
use i18n::t;
use model::SteamId;
use ui::{Theme, icons, widgets as w};

/// Reviews per page on Steam.
const REVIEWS_PER_PAGE: u32 = 10;

impl Profile {
    /// Badges, reviews or groups, with a way back to the profile.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn list_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool, section: Section) {
        ui.horizontal(|ui| {
            // A card collection is reached from the badges.
            let (label, to) = if section == Section::Cards { (t!("Back to badges"), format!("{id}/{}", Section::Badges.slug())) } else { (t!("Back to profile"), id.to_string()) };
            if w::button(ui, t, Some(icons::ARROW_LEFT), label, w::Kind::Ghost, w::Size::Sm).clicked() {
                ctx.navigate("profile", Some(to));
            }
        });
        ui.add_space(8.0);
        panel(ui, t, |ui| match section {
            Section::Badges => self.badges_list(ui, t, ctx, id, name, mine),
            Section::Reviews => self.reviews_list(ui, t, ctx, id, name, mine),
            Section::Groups | Section::CommonGroups => self.groups_list(ui, t, ctx, id, name, mine),
            Section::Cards => self.card_set(ui, t, ctx, id),
            Section::Profile | Section::Games | Section::Edit => {}
        });
    }

    fn badges_list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool) {
        self.request_social(ctx, id);
        let title = if mine { t!("Your badges").to_string() } else { t!("{name}'s badges", name = name) };
        ui.label(w::text_bold(title, 24.0, t.text));
        let b = match ctx.state.badges.get(&id).cloned() {
            None => return skeleton_rows(ui, t, 70.0),
            Some(Err(e)) => return hidden(ui, t, &e, t!("These badges are hidden")),
            Some(Ok(b)) => b,
        };
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            level_badge(ui, t, b.level);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(w::text_bold(t!("Level {n}", n = b.level), 16.0, t.text));
                let sub = [b.xp.as_str(), b.next.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                ui.label(w::text(sub, 12.5, t.text_dim));
            });
        });
        if !b.next.is_empty() {
            ui.add_space(4.0);
            let width = ui.available_width().min(420.0);
            w::progress(ui, t, b.progress, width);
        }
        ui.add_space(12.0);
        ui.label(w::text(t!("{n} badges", n = b.items.len()), 13.5, t.text_dim));
        ui.add_space(6.0);
        if b.items.is_empty() {
            ui.label(w::text(t!("No badges yet."), 13.5, t.text_faint));
        }
        for badge in &b.items {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            ui.painter().rect_filled(r, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface_hover, hover));
            let mut icon_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(60.0, 60.0))));
            let icon_clicked = badge_icon(&mut icon_ui, t, badge, 60.0);
            let x = r.left() + 80.0;
            let p = ui.painter();
            p.text(pos2(x, r.top() + 20.0), Align2::LEFT_CENTER, &badge.title, w::font_semibold(14.0), t.text);
            p.text(pos2(x, r.top() + 38.0), Align2::LEFT_CENTER, &badge.detail, w::font(12.5), t.accent);
            p.text(pos2(x, r.top() + 55.0), Align2::LEFT_CENTER, &badge.unlocked, w::font(11.5), t.text_faint);
            let cards = card_set_of(&badge.url).is_some();
            if cards {
                p.text(pos2(r.right() - 12.0, r.center().y), Align2::RIGHT_CENTER, format!("{}  {}", t!("Card collection"), icons::CARET_RIGHT), w::font_medium(12.5), t.text_dim);
            }
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let resp = if cards || badge.url.is_empty() { resp } else { resp.on_hover_text(t!("Opens in your browser")) };
            if resp.clicked() || icon_clicked {
                open_badge(ctx, id, badge);
            }
        }
    }

    /// A game's trading cards on this profile, like Steam's badge page:
    /// the badge so far, then every card, the missing ones dimmed.
    fn card_set(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId) {
        let (app, foil) = self.cards;
        if self.lists_requested.insert((id, Section::Cards, app * 2 + u32::from(foil))) {
            ctx.send(Command::LoadCardSet { steam_id: id, app_id: app, foil });
        }
        let set = match ctx.state.card_sets.get(&(id, app, foil)).cloned() {
            None => {
                ui.label(w::text_bold(t!("Card collection"), 24.0, t.text));
                return skeleton_rows(ui, t, 80.0);
            }
            Some(Err(e)) => return hidden(ui, t, &e, t!("This card collection is hidden")),
            Some(Ok(s)) => s,
        };
        ui.horizontal(|ui| {
            let (img, resp) = ui.allocate_exact_size(vec2(184.0, 69.0), Sense::click());
            w::image_or_skeleton(ui, t, img, &model::art::header(app), t.corner_small());
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                let owned = ctx.state.apps.as_ref().is_some_and(|a| a.contains_key(&app));
                ctx.navigate(if owned { "library" } else { "store" }, Some(if owned { app.to_string() } else { format!("app:{app}") }));
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(w::text_bold(&set.title, 20.0, t.text));
                let sub = [set.playtime.as_str(), set.drops.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                if !sub.is_empty() {
                    ui.label(w::text(sub, 13.0, t.text_dim));
                }
                let mut kind = foil;
                w::segmented(ui, t, &mut kind, &[(false, &t!("Normal")), (true, &t!("Foil"))]);
                if kind != foil {
                    ctx.navigate("profile", Some(format!("{id}/cards/{app}{}", if kind { "/foil" } else { "" })));
                }
            });
        });
        ui.add_space(12.0);
        if let Some(b) = &set.badge {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                match &b.icon {
                    Some(u) => {
                        if !w::paint_contain(ui, r, u, Align2::CENTER_CENTER) {
                            w::skeleton(ui, t, r, CornerRadius::same(6));
                        }
                    }
                    None => w::placeholder(ui, t, r, "", CornerRadius::same(6)),
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(w::text_bold(&b.name, 15.0, t.text));
                    ui.label(w::text(&b.level, 13.0, t.accent));
                    ui.label(w::text(&b.unlocked, 12.0, t.text_faint));
                });
            });
        } else {
            ui.label(w::text(t!("No badge crafted yet."), 13.5, t.text_faint));
        }
        let have = set.cards.iter().filter(|c| c.count > 0).count();
        let total = set.cards.len();
        ui.add_space(12.0);
        ui.label(w::text_bold(t!("Collected {n} of {total} cards", n = have, total = total), 15.0, t.text));
        ui.add_space(4.0);
        let width = ui.available_width().min(420.0);
        w::progress(ui, t, have as f32 / total.max(1) as f32, width);
        if have == total && total > 0 {
            ui.label(w::text(t!("Every card is collected: the next badge level can be crafted on Steam."), 12.5, t.online));
        }
        ui.add_space(14.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(14.0, 14.0);
            for c in &set.cards {
                card_tile(ui, t, c);
            }
        });
    }

    fn reviews_list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool) {
        let page = self.review_page.max(1);
        if self.lists_requested.insert((id, Section::Reviews, page)) {
            ctx.send(Command::LoadReviews { steam_id: id, page });
        }
        let title = if mine { t!("Your reviews").to_string() } else { t!("{name}'s reviews", name = name) };
        ui.label(w::text_bold(title, 24.0, t.text));
        let reviews = match ctx.state.reviews.get(&(id, page)).cloned() {
            None => return skeleton_rows(ui, t, 120.0),
            Some(Err(e)) => return hidden(ui, t, &e, t!("These reviews are hidden")),
            Some(Ok(r)) => r,
        };
        ui.label(w::text(t!("{n} reviews", n = reviews.total), 13.5, t.text_dim));
        ui.add_space(10.0);
        if reviews.items.is_empty() {
            w::empty(ui, t, icons::THUMBS_UP, t!("No reviews yet"), "");
            return;
        }
        for r in &reviews.items {
            review_card(ui, t, ctx, r);
            ui.add_space(8.0);
        }
        let pages = reviews.total.div_ceil(REVIEWS_PER_PAGE).max(1);
        if pages > 1 {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled_ui(page > 1, |ui| w::button(ui, t, Some(icons::CARET_LEFT), t!("Previous"), w::Kind::Secondary, w::Size::Sm)).inner.clicked() {
                    self.review_page = page - 1;
                }
                ui.label(w::text(t!("Page {n} of {total}", n = page, total = pages), 13.0, t.text_dim));
                if ui.add_enabled_ui(page < pages, |ui| w::button(ui, t, Some(icons::CARET_RIGHT), t!("Next"), w::Kind::Secondary, w::Size::Sm)).inner.clicked() {
                    self.review_page = page + 1;
                }
            });
        }
    }

    fn groups_list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool) {
        if self.lists_requested.insert((id, Section::Groups, 0)) {
            ctx.send(Command::LoadGroups { steam_id: id });
        }
        let common = self.section == Section::CommonGroups;
        let title = if common { t!("Groups in Common").to_string() } else if mine { t!("Your groups").to_string() } else { t!("{name}'s groups", name = name) };
        ui.label(w::text_bold(title, 24.0, t.text));
        let data = if common { self.page_data(ctx, id).and_then(|p| p.ok()).and_then(|p| p.common_groups) } else { ctx.state.groups.get(&id).cloned() };
        let groups = match data {
            None => return skeleton_rows(ui, t, 64.0),
            Some(Err(e)) if common => { ui.label(w::text(t!("Could not load groups in common"), 14.0, t.text)); ui.label(w::text(&e, 12.5, t.text_dim)); return; }
            Some(Err(e)) => return hidden(ui, t, &e, t!("These groups are hidden")),
            Some(Ok(g)) => g,
        };
        ui.label(w::text(t!("{n} groups", n = groups.len()), 13.5, t.text_dim));
        ui.add_space(10.0);
        if groups.is_empty() {
            w::empty(ui, t, icons::USERS_THREE, t!("Not in any groups"), "");
            return;
        }
        for g in &groups {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            ui.painter().rect_filled(r, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface_hover, hover));
            let img = Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(52.0, 52.0));
            match &g.avatar {
                Some(u) => w::image_or_skeleton(ui, t, img, u, CornerRadius::same(4)),
                None => w::placeholder(ui, t, img, "", CornerRadius::same(4)),
            }
            let x = img.right() + 12.0;
            let sub = [g.members.as_str(), g.online.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let p = ui.painter();
            p.text(pos2(x, r.top() + 22.0), Align2::LEFT_CENTER, &g.name, w::font_semibold(14.5), t.text);
            p.text(pos2(x, r.top() + 42.0), Align2::LEFT_CENTER, sub, w::font(12.5), t.text_dim);
            p.text(pos2(r.right() - 12.0, r.center().y), Align2::RIGHT_CENTER, icons::EXTERNAL, w::font(12.0), t.text_faint);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("Opens in your browser")).clicked() {
                crate::links::open_http(&g.url);
            }
        }
    }
}

/// One review: the game's capsule (opens it), the verdict, playtime, text.
fn review_card(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, r: &model::UserReview) {
    egui::Frame::new().fill(t.surface.gamma_multiply(0.7)).corner_radius(t.corner_small()).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (img, resp) = ui.allocate_exact_size(vec2(184.0, 69.0), Sense::click());
            match &r.capsule {
                Some(u) => w::image_or_skeleton(ui, t, img, u, t.corner_small()),
                None => w::image_or_skeleton(ui, t, img, &model::art::header(r.app_id), t.corner_small()),
            }
            let owned = ctx.state.apps.as_ref().is_some_and(|a| a.contains_key(&r.app_id));
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                if owned {
                    ctx.navigate("library", Some(r.app_id.to_string()));
                } else {
                    ctx.navigate("store", Some(format!("app:{}", r.app_id)));
                }
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                let (icon, verdict, color) = if r.recommended { (icons::THUMBS_UP, t!("Recommended"), t.online) } else { (icons::THUMBS_DOWN, t!("Not Recommended"), t.danger) };
                ui.horizontal(|ui| {
                    ui.label(w::text(icon, 18.0, color));
                    ui.label(w::text_bold(verdict, 16.0, t.text));
                    if let Some(name) = ctx.state.app_name(r.app_id) {
                        ui.label(w::text(format!("·  {name}"), 13.5, t.text_dim));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !r.url.is_empty() && w::icon_button(ui, t, icons::EXTERNAL, t!("Open on Steam"), false, 0).clicked() {
                            crate::links::open_http(&r.url);
                        }
                    });
                });
                if !r.hours.is_empty() {
                    ui.label(w::text(&r.hours, 12.5, t.text_dim));
                }
                if !r.posted.is_empty() {
                    ui.label(w::text(&r.posted, 12.0, t.text_faint));
                }
            });
        });
        if !r.text.is_empty() {
            ui.add_space(8.0);
            ui.add(egui::Label::new(w::text(&r.text, 13.5, t.text)).wrap());
        }
        if !r.helpful.is_empty() {
            ui.add_space(6.0);
            ui.label(w::text(&r.helpful, 12.0, t.text_faint));
        }
    });
}

fn skeleton_rows(ui: &mut Ui, t: &Theme, height: f32) {
    ui.add_space(10.0);
    for _ in 0..5 {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        w::skeleton(ui, t, r.shrink2(vec2(0.0, 4.0)), t.corner_small());
    }
}

/// Steam hides the list (private profile), or it couldn't load.
fn hidden(ui: &mut Ui, t: &Theme, error: &str, title: &str) {
    let body = if error == "private" { t!("Its owner has chosen to keep it private.").to_string() } else { error.to_string() };
    ui.add_space(10.0);
    w::empty(ui, t, icons::LOCK_SIMPLE, title, &body);
}

/// One trading card; missing ones dimmed, extra copies counted.
fn card_tile(ui: &mut Ui, t: &Theme, c: &model::Card) {
    let (r, resp) = ui.allocate_exact_size(vec2(124.0, 196.0), Sense::hover());
    let img = Rect::from_min_size(r.min, vec2(124.0, 145.0));
    w::image_or_skeleton(ui, t, img, &c.image, CornerRadius::same(6));
    let p = ui.painter().with_clip_rect(r);
    if c.count == 0 {
        p.rect_filled(img, CornerRadius::same(6), t.bg.gamma_multiply(0.65));
    }
    if c.count > 1 {
        let tag = Rect::from_min_size(pos2(img.right() - 34.0, img.top() + 6.0), vec2(28.0, 20.0));
        p.rect_filled(tag, CornerRadius::same(10), t.accent);
        p.text(tag.center(), Align2::CENTER_CENTER, format!("×{}", c.count), w::font_bold(12.0), t.on_accent);
    }
    let color = if c.count > 0 { t.text } else { t.text_faint };
    p.text(pos2(r.left(), img.bottom() + 14.0), Align2::LEFT_CENTER, &c.name, w::font_semibold(12.5), color);
    p.text(pos2(r.left(), img.bottom() + 32.0), Align2::LEFT_CENTER, &c.series, w::font(11.0), t.text_faint);
    let tip = if c.count == 0 { format!("{}\n{}", c.name, t!("Not collected yet")) } else { c.name.clone() };
    resp.on_hover_text(tip);
}

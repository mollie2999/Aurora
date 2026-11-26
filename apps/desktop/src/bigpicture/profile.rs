//! Native Big Picture profile pages: profile view, games, badges, card sets,
//! reviews, groups, and full profile editor (feature parity with desktop).

use super::focus::{Dir, Place, Scroller};
use super::osk;
use super::paint::{self, Kind};
use super::screens::{button, empty, heading};
use super::{BigPicture, ChatTarget, Frame, Page, Tab};
use crate::builtin::profile::{card_set_of, edit::community_badge_id, themes};
use client::{Command, Event};
use egui::{Align2, Color32, CornerRadius, Id, Rect, Stroke, Ui, pos2, vec2};
use extensions::Action;
use i18n::t;
use model::{AppId, ProfileEdit, ProfileItemChoice, ProfilePrivacy, ProfileSave, Relationship, SteamId, Visibility, art};
use std::collections::{HashMap, HashSet};
use ui::{Theme, icons, widgets as w};

const REVIEWS_PER_PAGE: u32 = 10;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) enum ProfileGameSort {
    #[default]
    Recent,
    Playtime,
    Name,
}

impl ProfileGameSort {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Recent => Self::Playtime,
            Self::Playtime => Self::Name,
            Self::Name => Self::Recent,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Recent => t!("Recently played"),
            Self::Playtime => t!("Playtime"),
            Self::Name => t!("Name"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) enum EditSection {
    #[default]
    General,
    Avatar,
    Background,
    MiniProfile,
    Theme,
    Badge,
    Privacy,
}

impl EditSection {
    pub(crate) const ALL: [Self; 7] = [
        Self::General,
        Self::Avatar,
        Self::Background,
        Self::MiniProfile,
        Self::Theme,
        Self::Badge,
        Self::Privacy,
    ];

    pub(crate) fn from_arg(s: &str) -> Self {
        match s {
            "avatar" => Self::Avatar,
            "background" => Self::Background,
            "miniprofile" => Self::MiniProfile,
            "theme" => Self::Theme,
            "badge" | "favoritebadge" => Self::Badge,
            "privacy" | "settings" => Self::Privacy,
            _ => Self::General,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::General => t!("General"),
            Self::Avatar => t!("Avatar"),
            Self::Background => t!("Profile background"),
            Self::MiniProfile => t!("Mini profile"),
            Self::Theme => t!("Theme"),
            Self::Badge => t!("Featured badge"),
            Self::Privacy => t!("Privacy settings"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::General => icons::USER_CIRCLE,
            Self::Avatar | Self::Background | Self::MiniProfile => icons::IMAGE,
            Self::Theme => icons::PALETTE,
            Self::Badge => icons::MEDAL,
            Self::Privacy => icons::EYE_SLASH,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EditField {
    PersonaName,
    RealName,
    CustomUrl,
    Summary,
    Filter,
}

#[derive(Default)]
pub(crate) struct EditState {
    pub section: EditSection,
    pub draft: Option<ProfileEdit>,
    asked: bool,
    reloading: bool,
    pending: usize,
    after: Vec<Command>,
    asked_locations: HashSet<(Option<String>, Option<String>)>,
    pub active_field: Option<EditField>,
    pub filter: String,
    see_all: [bool; 2],
    badges_asked: bool,
    pub rail_scroll: Scroller,
    pub body_scroll: Scroller,
}

impl EditState {
    fn saving(&self) -> bool {
        self.pending > 0
    }
}

#[derive(Default)]
pub(crate) struct ProfileState {
    requested: HashSet<SteamId>,
    games_requested: HashSet<SteamId>,
    /// (steam_id, list_tag, key): tag 1=Badges, 2=Reviews, 3=Groups, 4=Cards.
    lists_requested: HashSet<(SteamId, u8, u32)>,
    showcase_requested: HashSet<SteamId>,
    social_requested: HashSet<SteamId>,
    pub games_search: String,
    pub games_search_open: bool,
    pub games_sort: ProfileGameSort,
    pub review_page: u32,
    pub comment_draft: String,
    pub comment_open: bool,
    comment_start: HashMap<SteamId, u32>,
    pub edit: EditState,
}

impl ProfileState {
    pub(crate) fn active_text_open(&self, page: &Page) -> bool {
        match page {
            Page::Profile(_) => self.comment_open,
            Page::ProfileGames(_) => self.games_search_open,
            Page::ProfileEdit => self.edit.active_field.is_some(),
            _ => false,
        }
    }

    pub(crate) fn active_target_id(&self, page: &Page) -> Option<Id> {
        match page {
            Page::Profile(id) if self.comment_open => Some(Id::unique(("bp-osk-prof-c", *id))),
            Page::ProfileGames(id) if self.games_search_open => Some(Id::unique(("bp-osk-prof-g", *id))),
            Page::ProfileEdit => self.edit.active_field.map(|f| Id::unique(("bp-osk-prof-e", f as usize))),
            _ => None,
        }
    }

    pub(crate) fn active_text_mut(&mut self, page: &Page) -> Option<&mut String> {
        match page {
            Page::Profile(_) if self.comment_open => Some(&mut self.comment_draft),
            Page::ProfileGames(_) if self.games_search_open => Some(&mut self.games_search),
            Page::ProfileEdit => match self.edit.active_field? {
                EditField::Filter => Some(&mut self.edit.filter),
                EditField::PersonaName => self.edit.draft.as_mut().map(|d| &mut d.persona_name),
                EditField::RealName => self.edit.draft.as_mut().map(|d| &mut d.real_name),
                EditField::CustomUrl => self.edit.draft.as_mut().map(|d| &mut d.custom_url),
                EditField::Summary => self.edit.draft.as_mut().map(|d| &mut d.summary),
            },
            _ => None,
        }
    }

    pub(crate) fn cycle_edit_field(&mut self, dir: i8) -> bool {
        if dir == 0 || self.edit.draft.is_none() {
            return false;
        }
        const GENERAL_FIELDS: [EditField; 4] = [
            EditField::PersonaName,
            EditField::RealName,
            EditField::CustomUrl,
            EditField::Summary,
        ];
        let Some(cur) = self.edit.active_field else {
            return false;
        };
        let Some(idx) = GENERAL_FIELDS.iter().position(|f| *f == cur) else {
            return false;
        };
        let len = GENERAL_FIELDS.len();
        let next = if dir > 0 { (idx + 1) % len } else { (idx + len - 1) % len };
        self.edit.active_field = Some(GENERAL_FIELDS[next]);
        true
    }

    pub(crate) fn close_osk(&mut self, page: &Page) -> bool {
        match page {
            Page::Profile(_) if self.comment_open => {
                self.comment_open = false;
                true
            }
            Page::ProfileGames(_) if self.games_search_open => {
                self.games_search_open = false;
                true
            }
            Page::ProfileEdit if self.edit.active_field.is_some() => {
                self.edit.active_field = None;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn on_open_page(&mut self, page: &Page) {
        match page {
            Page::Profile(_) => {
                self.comment_open = false;
            }
            Page::ProfileGames(_) => {
                self.games_search.clear();
                self.games_search_open = false;
            }
            Page::ProfileReviews(_) => {
                self.review_page = 1;
            }
            Page::ProfileEdit => {
                self.edit.active_field = None;
                self.edit.filter.clear();
            }
            _ => {}
        }
    }

    pub(crate) fn on_event(&mut self, event: &Event, client: &client::Client, my_id: SteamId, actions: &mut Vec<Action>) {
        match event {
            Event::Profile(p) => {
                self.requested.remove(&p.steam_id);
            }
            Event::PlayerGames { steam_id, .. } => {
                self.games_requested.remove(steam_id);
            }
            Event::Reviews { steam_id, page, .. } => {
                self.lists_requested.remove(&(*steam_id, 2, *page));
            }
            Event::CommentPosted { result: Ok(()), .. } => {
                self.comment_draft.clear();
                self.comment_open = false;
            }
            Event::ProfileEdit(res) => {
                if self.edit.reloading || self.edit.draft.is_none() {
                    self.edit.reloading = false;
                    if let Ok(e) = res {
                        self.edit.draft = Some(e.clone());
                    }
                }
            }
            Event::ProfileSaved { what, result } => {
                if *what == ProfileSave::Privacy && self.edit.pending == 0 {
                    // Privacy toast is handled by desktop Profile broadcast; reload draft on error.
                    if result.is_err() {
                        self.edit.reloading = true;
                        client.send(Command::LoadProfileEdit { language: lang() });
                    }
                    return;
                }
                if self.edit.pending == 0 {
                    return;
                }
                self.edit.pending = self.edit.pending.saturating_sub(1);
                match result {
                    Ok(()) => {
                        let after = std::mem::take(&mut self.edit.after);
                        self.edit.pending += after.len();
                        for c in after {
                            client.send(c);
                        }
                        if self.edit.pending > 0 {
                            return;
                        }
                        actions.push(Action::Toast(
                            match what {
                                ProfileSave::Info => t!("Profile saved"),
                                ProfileSave::Privacy => t!("Privacy settings saved"),
                                ProfileSave::Avatar => t!("Avatar changed"),
                                ProfileSave::Item | ProfileSave::BackgroundFlags | ProfileSave::Theme | ProfileSave::Badge => {
                                    t!("Profile updated")
                                }
                            }
                            .into(),
                        ));
                        self.edit.reloading = true;
                        client.send(Command::LoadProfileEdit { language: lang() });
                        if my_id != 0 {
                            client.send(Command::LoadProfile { steam_id: my_id });
                            self.requested.remove(&my_id);
                        }
                    }
                    Err(e) => {
                        self.edit.after.clear();
                        actions.push(Action::Toast(t!("Steam didn't save it: {error}", error = i18n::tr_dyn(e))));
                    }
                }
            }
            _ => {}
        }
    }

    fn send_saves(&mut self, client: &client::Client, now: Vec<Command>, after: Vec<Command>) {
        self.edit.pending += now.len();
        for c in now {
            client.send(c);
        }
        self.edit.after.extend(after);
    }

    fn request_social(&mut self, state: &client::State, client: &client::Client, id: SteamId) {
        let have_badges = state.badges.contains_key(&id);
        let have_comments = state.comments.contains_key(&id);
        if have_badges && have_comments {
            self.social_requested.remove(&id);
        } else if self.social_requested.insert(id) {
            if !have_badges {
                client.send(Command::LoadBadges { steam_id: id });
            }
            if !have_comments {
                client.send(Command::LoadComments { steam_id: id, start: 0 });
            }
        }
        if !state.reviews.contains_key(&(id, 1)) && self.lists_requested.insert((id, 2, 1)) {
            client.send(Command::LoadReviews { steam_id: id, page: 1 });
        }
        if self.lists_requested.insert((id, 3, 0)) {
            client.send(Command::LoadGroups { steam_id: id });
        }
    }
}

/// Section heading with an optional right-aligned pill button; guarantees no vertical or horizontal overlap.
#[allow(clippy::too_many_arguments)]
fn section_header_button(
    bp: &mut BigPicture,
    ui: &mut Ui,
    f: &Frame,
    left: f32,
    y: &mut f32,
    width: f32,
    title: &str,
    btn: Option<(Id, Option<&str>, &str)>,
) -> bool {
    let s = f.s;
    if let Some((id, icon, label)) = btn {
        let bw = paint::pill_width(ui, icon.is_some(), label, s);
        let (r, hit) = button(bp, ui, f, id, pos2(left + width - bw, *y), bw, icon, label, Kind::Secondary, None);
        paint::line(
            ui.painter(),
            pos2(left, r.center().y),
            Align2::LEFT_CENTER,
            title,
            w::font_bold(30.0 * s),
            f.t.text,
            (width - bw - 24.0 * s).max(80.0 * s),
        );
        *y = r.bottom() + 18.0 * s;
        hit.activated
    } else {
        heading(ui, f, y, title);
        false
    }
}

fn lang() -> String {
    i18n::language().steam_name().to_string()
}

/// Parse a desktop `"profile"` navigation argument into a native Big Picture page.
pub(crate) fn parse_route(my_id: SteamId, arg: Option<&str>) -> (Page, Option<EditSection>) {
    if let Some(rest) = arg.and_then(|a| a.strip_prefix("edit"))
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return (Page::ProfileEdit, Some(EditSection::from_arg(rest.trim_start_matches('/'))));
    }
    let mut parts = arg.unwrap_or_default().split('/').peekable();
    let parsed_id: Option<SteamId> = parts.peek().and_then(|a| a.parse().ok());
    if parsed_id.is_some() || parts.peek() == Some(&"") {
        parts.next();
    }
    let id = parsed_id.filter(|id| *id != 0).unwrap_or(my_id);
    let page = match parts.next() {
        Some("games") => Page::ProfileGames(id),
        Some("badges") => Page::ProfileBadges(id),
        Some("reviews") => Page::ProfileReviews(id),
        Some("groups") => Page::ProfileGroups(id),
        Some("cards") => {
            let app = parts.next().and_then(|a| a.parse().ok()).unwrap_or(0);
            let foil = parts.next() == Some("foil");
            Page::ProfileCards(id, app, foil)
        }
        _ => Page::Profile(id),
    };
    (page, None)
}

fn open_game(bp: &mut BigPicture, f: &Frame, app: AppId) {
    let owned = f.app.state.apps.as_ref().is_some_and(|a| a.contains_key(&app));
    bp.open(if owned { Page::Game(app) } else { Page::StoreApp(app) }, f.now);
}

fn open_badge(bp: &mut BigPicture, id: SteamId, badge: &model::Badge, now: f64) {
    match card_set_of(&badge.url) {
        Some((app, foil)) => bp.open(Page::ProfileCards(id, app, foil), now),
        None if !badge.url.is_empty() => {
            crate::links::open_http(&badge.url);
        }
        None => {}
    }
}

/// Steam-style level ring; color steps every 10 levels.
fn paint_level_badge(p: &egui::Painter, t: &Theme, center: egui::Pos2, level: u32, s: f32) {
    let colors = [0x9B9B9B, 0xC02942, 0xD95B43, 0xFECC23, 0x467A3C, 0x4E8DDB, 0x7652C9, 0xC252C9, 0x542437, 0x997C52];
    let c = colors[(level as usize / 10) % colors.len()];
    let color = Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
    p.circle_filled(center, 22.0 * s, t.chrome.gamma_multiply(0.85));
    p.circle_stroke(center, 22.0 * s, Stroke::new(3.0 * s, color));
    p.text(center, Align2::CENTER_CENTER, level.to_string(), w::font_bold(17.0 * s), t.text);
}

/// Strip Steam HTML/BBCode tags into readable plain text for Big Picture painter.
fn plain_markup(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in raw.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let low = tag.trim().to_ascii_lowercase();
                if low == "br" || low == "br/" || low == "br /" || low == "/p" || low == "/div" {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(ch),
            _ => out.push(ch),
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

fn ensure_profile_loaded(bp: &mut BigPicture, f: &Frame, id: SteamId) -> (bool, bool, Visibility) {
    let mine = id == f.app.state.steam_id;
    if id != 0 && !f.app.state.profiles.contains_key(&id) && bp.profile.requested.insert(id) {
        f.app.client.send(Command::LoadProfile { steam_id: id });
    }
    let friend = f.app.state.friends.get(&id) == Some(&Relationship::Friend);
    let visibility = f.app.state.profiles.get(&id).map(|i| i.visibility).unwrap_or_default();
    let open = mine
        || match visibility {
            Visibility::Public | Visibility::Unknown => true,
            Visibility::FriendsOnly => friend,
            Visibility::Private => false,
        };
    if id != 0 && open && !f.app.state.player_games.contains_key(&id) && bp.profile.games_requested.insert(id) {
        f.app.client.send(Command::LoadPlayerGames { steam_id: id, language: lang() });
    }
    (mine, open, visibility)
}

fn profile_display_name(state: &client::State, id: SteamId) -> String {
    if id == state.steam_id && !state.persona_name.is_empty() {
        state.persona_name.clone()
    } else {
        state.persona(id).name
    }
}

// ============================================================================
// Main Native Profile Page (`Page::Profile(SteamId)`)
// ============================================================================

pub(super) fn profile_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, raw_id: SteamId) -> f32 {
    let id = if raw_id == 0 { f.app.state.steam_id } else { raw_id };
    let (mine, open, visibility) = ensure_profile_loaded(bp, f, id);
    let s = f.s;
    let left = f.view.left() + f.pad;
    let width = (f.view.width() - 2.0 * f.pad).min(1380.0 * s);
    let mut y = top + 24.0 * s;

    let persona = f.app.state.persona(id);
    let info = f.app.state.profiles.get(&id).cloned();
    let name = profile_display_name(&f.app.state, id);
    let level = if mine {
        f.app.state.level.or_else(|| f.app.state.badges.get(&id).and_then(|r| r.as_ref().ok()).map(|b| b.level))
    } else {
        f.app.state.badges.get(&id).and_then(|r| r.as_ref().ok()).map(|b| b.level)
    };

    // ---- Hero Identity Card ----
    let summary_text = info.as_ref().map(|i| plain_markup(&i.summary));
    let summary_lines = summary_text
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| ((s.len() as f32 / 68.0).ceil() as usize).clamp(1, 3))
        .unwrap_or(1);
    let hero_h = (224.0 + (summary_lines.saturating_sub(1) as f32) * 24.0) * s;
    let hero_rect = Rect::from_min_size(pos2(left, y), vec2(width, hero_h));
    if hero_rect.intersects(ui.clip_rect()) {
        let p = ui.painter();
        p.rect_filled(hero_rect, paint::radius(18.0 * s), f.t.chrome.gamma_multiply(0.84));
        p.rect_stroke(hero_rect, paint::radius(18.0 * s), Stroke::new(1.0, Color32::from_white_alpha(26)), egui::StrokeKind::Inside);

        let av_outer = Rect::from_min_size(hero_rect.min + vec2(26.0 * s, 26.0 * s), vec2(156.0 * s, 156.0 * s));
        let av_inner = av_outer.shrink(12.0 * s);
        let still = art::avatar_full(persona.avatar_hash.as_deref());
        let avatar_url = info.as_ref().and_then(|i| i.animated_avatar.as_deref()).unwrap_or(&still);
        if !w::paint_cover(ui, av_inner, avatar_url, paint::radius(10.0 * s)) {
            paint::skeleton(ui, &f.t, av_inner, paint::radius(10.0 * s));
        }
        if let Some(frame) = info.as_ref().and_then(|i| i.avatar_frame.as_deref()) {
            w::paint_cover(ui, av_outer, frame, CornerRadius::ZERO);
        } else {
            ui.painter().rect_stroke(
                av_inner.expand(1.5 * s),
                paint::radius(11.0 * s),
                Stroke::new(3.0 * s, crate::shell::status_color(&f.t, &persona)),
                egui::StrokeKind::Outside,
            );
        }

        let tx = av_outer.right() + 24.0 * s;
        let tw = (hero_rect.right() - tx - 28.0 * s).max(120.0);
        let p = ui.painter();
        let name_font = w::font_bold(34.0 * s);
        let nw = paint::text_width(p, &name, name_font.clone()).min(tw - 60.0 * s);
        paint::line(p, pos2(tx, hero_rect.top() + 28.0 * s), Align2::LEFT_TOP, &name, name_font, Color32::WHITE, nw);
        if let Some(lvl) = level {
            paint_level_badge(p, &f.t, pos2(tx + nw + 34.0 * s, hero_rect.top() + 47.0 * s), lvl, s);
        }

        let mut ty = hero_rect.top() + 74.0 * s;
        if let Some(i) = &info {
            let sub: Vec<&str> = [i.real_name.as_str(), i.location.as_str()].into_iter().filter(|x| !x.is_empty()).collect();
            if !sub.is_empty() {
                paint::line(p, pos2(tx, ty), Align2::LEFT_TOP, &sub.join("  ·  "), w::font(18.0 * s), f.t.text_dim, tw);
                ty += 26.0 * s;
            }
        }
        let status_str = crate::shell::status_text(&f.app.state, &persona);
        let status_col = crate::shell::status_color(&f.t, &persona);
        paint::line(p, pos2(tx, ty), Align2::LEFT_TOP, &status_str, w::font_semibold(18.5 * s), status_col, tw);
        ty += 30.0 * s;

        match summary_text.as_deref() {
            Some(s_txt) if !s_txt.is_empty() => {
                let wr = paint::wrapped(p, pos2(tx, ty), s_txt, w::font(17.5 * s), f.t.text, tw, 3);
                ty = wr.bottom() + 8.0 * s;
            }
            Some(_) => {
                paint::line(p, pos2(tx, ty), Align2::LEFT_TOP, t!("No profile summary yet."), w::font(17.0 * s), f.t.text_faint, tw);
                ty += 24.0 * s;
            }
            None => {
                paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(tx, ty + 4.0 * s), vec2((tw * 0.65).min(420.0 * s), 14.0 * s)), paint::radius(4.0 * s));
                ty += 26.0 * s;
            }
        }
        if let Some(since) = info.as_ref().and_then(|i| i.member_since) {
            paint::line(
                ui.painter(),
                pos2(tx, ty.min(hero_rect.bottom() - 28.0 * s)),
                Align2::LEFT_TOP,
                &t!("Member since {date}", date = ui::date(since)),
                w::font(15.5 * s),
                f.t.text_faint,
                tw,
            );
        }
    }
    y += hero_h + 18.0 * s;

    // ---- Action buttons row ----
    let mut bx = left;
    let mut row_bottom = y;
    let first_id = if mine { Id::unique(("bp-prof-edit", id)) } else { Id::unique(("bp-prof-msg", id)) };
    bp.preferred.get_or_insert(first_id);
    if mine {
        let (r, hit) = button(bp, ui, f, first_id, pos2(bx, y), 0.0, Some(icons::NOTE), t!("Edit profile"), Kind::Primary, None);
        if hit.activated {
            bp.profile.edit.section = EditSection::General;
            bp.open(Page::ProfileEdit, f.now);
        }
        bx = r.right() + 14.0 * s;
        row_bottom = row_bottom.max(r.bottom());
    } else {
        let (r, hit) = button(bp, ui, f, first_id, pos2(bx, y), 0.0, Some(icons::CHAT), t!("Send a message"), Kind::Primary, None);
        if hit.activated {
            bp.open(Page::Chat(ChatTarget::Friend(id)), f.now);
        }
        bx = r.right() + 14.0 * s;
        row_bottom = row_bottom.max(r.bottom());
        let web_w = paint::pill_width(ui, true, t!("View on Steam"), s);
        if bx + web_w > left + width {
            bx = left;
            y = row_bottom + 14.0 * s;
        }
        let (r, hit) = button(bp, ui, f, Id::unique(("bp-prof-web", id)), pos2(bx, y), 0.0, Some(icons::EXTERNAL), t!("View on Steam"), Kind::Secondary, None);
        if hit.activated {
            crate::links::open_http(&format!("https://steamcommunity.com/profiles/{id}"));
        }
        bx = r.right() + 14.0 * s;
        row_bottom = row_bottom.max(r.bottom());
    }
    let copy_w = paint::pill_width(ui, true, t!("Copy SteamID"), s);
    if bx + copy_w > left + width {
        bx = left;
        y = row_bottom + 14.0 * s;
    }
    let (r, copy_hit) = button(bp, ui, f, Id::unique(("bp-prof-copy", id)), pos2(bx, y), 0.0, Some(icons::COPY), t!("Copy SteamID"), Kind::Secondary, None);
    if copy_hit.activated {
        ui.ctx().copy_text(id.to_string());
        f.actions.push(Action::Toast(t!("SteamID copied").into()));
    }
    y = row_bottom.max(r.bottom()) + 24.0 * s;

    // ---- Currently in game banner ----
    if persona.in_game() {
        let game = persona
            .game_name
            .clone()
            .or_else(|| persona.game_app_id.and_then(|g| f.app.state.app_name(g).map(String::from)))
            .unwrap_or_default();
        let rect = Rect::from_min_size(pos2(left, y), vec2(width, 98.0 * s));
        let hit = bp.focus.item(ui, Id::unique(("bp-prof-ingame", id)), rect, ui.clip_rect(), Place::Page);
        if rect.intersects(ui.clip_rect()) {
            let p = ui.painter();
            p.rect_filled(rect, paint::radius(14.0 * s), w::mix(f.t.chrome.gamma_multiply(0.84), f.t.raised, hit.glow));
            p.rect_stroke(rect, paint::radius(14.0 * s), Stroke::new(1.5 * s, f.t.in_game.gamma_multiply(0.55)), egui::StrokeKind::Inside);
            let mut tx = rect.left() + 20.0 * s;
            if let Some(app) = persona.game_app_id {
                let img = Rect::from_center_size(pos2(rect.left() + 106.0 * s, rect.center().y), vec2(170.0 * s, 72.0 * s));
                if !w::paint_cover(ui, img, &art::header(app), paint::radius(8.0 * s)) {
                    paint::skeleton(ui, &f.t, img, paint::radius(8.0 * s));
                }
                tx = img.right() + 22.0 * s;
            }
            let p = ui.painter();
            paint::line(p, pos2(tx, rect.top() + 22.0 * s), Align2::LEFT_TOP, &w::sentence_case(t!("CURRENTLY IN GAME")), w::font_semibold(15.5 * s), f.t.in_game, rect.right() - tx - 20.0 * s);
            paint::line(p, pos2(tx, rect.bottom() - 22.0 * s), Align2::LEFT_BOTTOM, &game, w::font_bold(24.0 * s), f.t.text, rect.right() - tx - 20.0 * s);
        }
        if hit.focused {
            bp.focus.ring_around(rect, 14.0 * s);
        }
        if hit.activated && let Some(app) = persona.game_app_id {
            open_game(bp, f, app);
        }
        y += 116.0 * s;
    }

    if !open {
        let title = if visibility == Visibility::FriendsOnly {
            t!("This profile is visible to friends only")
        } else {
            t!("This profile is private")
        };
        empty(bp, ui, f, y, icons::LOCK_SIMPLE, title, t!("Games, inventory and friends are hidden by its owner."), None);
        return y - top + 340.0 * s;
    }

    bp.profile.request_social(&f.app.state, &f.app.client, id);
    if bp.profile.showcase_requested.insert(id) {
        f.app.client.send(Command::LoadInventory { steam_id: id, app_id: 753, context_id: 6, language: lang() });
    }

    // ---- Profile Quick Links / Stats Grid ----
    y = profile_links_grid(bp, ui, f, left, y, width, id, mine);
    y += 28.0 * s;

    // ---- Level & Badges Preview ----
    y = profile_badges_strip(bp, ui, f, left, y, width, id);

    // ---- Recent Activity ----
    y = profile_recent_games(bp, ui, f, left, y, width, id);

    // ---- Item Showcase ----
    y = profile_showcase(bp, ui, f, left, y, width, id);

    // ---- Most Played ----
    y = profile_most_played(bp, ui, f, left, y, width, id);

    // ---- Friends Preview (own profile) ----
    if mine {
        y = profile_friends_preview(bp, ui, f, left, y, width);
    }

    // ---- Comments ----
    y = profile_comments(bp, ui, f, left, y, width, id, mine);

    y - top + 48.0 * s
}

fn profile_links_grid(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, id: SteamId, mine: bool) -> f32 {
    let s = f.s;
    let owned = f.app.state.apps.as_ref().map(|a| a.values().filter(|a| !a.is_dlc()).count());
    let games = match f.app.state.player_games.get(&id) {
        Some(Ok(g)) if mine && g.is_empty() => owned.map(|n| n.to_string()).unwrap_or_default(),
        Some(Ok(g)) => g.len().to_string(),
        Some(Err(_)) => "—".into(),
        None if mine => owned.map(|n| n.to_string()).unwrap_or_default(),
        None => String::new(),
    };
    let items: u64 = match f.app.state.inventory_apps.get(&id) {
        Some(Ok(apps)) => apps.iter().flat_map(|a| a.contexts.iter()).map(|c| c.2 as u64).sum(),
        _ => f.app.state.inventories.iter().filter(|(k, _)| k.0 == id).map(|(_, v)| v.total as u64).sum(),
    };
    let badges = match f.app.state.badges.get(&id) {
        Some(Ok(b)) if !b.items.is_empty() => b.items.len().to_string(),
        _ => String::new(),
    };
    let reviews = match f.app.state.reviews.get(&(id, 1)) {
        Some(Ok(r)) if r.total > 0 => r.total.to_string(),
        _ => String::new(),
    };
    let groups = match f.app.state.groups.get(&id) {
        Some(Ok(g)) if !g.is_empty() => g.len().to_string(),
        _ => String::new(),
    };
    let friends_count = if mine { f.app.state.friend_count().to_string() } else { String::new() };

    enum Target {
        Page(Page),
        Tab(Tab),
        Web(String),
    }

    let entries = [
        ("games", icons::BOOKS, t!("Games"), games, Target::Page(Page::ProfileGames(id))),
        ("badges", icons::MEDAL, t!("Badges"), badges, Target::Page(Page::ProfileBadges(id))),
        ("reviews", icons::THUMBS_UP, t!("Reviews"), reviews, Target::Page(Page::ProfileReviews(id))),
        ("groups", icons::USERS_THREE, t!("Groups"), groups, Target::Page(Page::ProfileGroups(id))),
        ("inventory", icons::BACKPACK, t!("Inventory"), if items > 0 { items.to_string() } else { String::new() }, Target::Page(Page::Embedded("inventory".into(), Some(id.to_string())))),
        ("friends", icons::USERS, t!("Friends"), friends_count, if mine { Target::Tab(Tab::Friends) } else { Target::Web(format!("https://steamcommunity.com/profiles/{id}/friends/")) }),
        ("screenshots", icons::CAMERA, t!("Screenshots"), String::new(), Target::Page(Page::Embedded("user_files".into(), Some(format!("{id}/screenshots"))))),
        ("workshop", icons::CUBE, t!("Workshop items"), String::new(), Target::Page(Page::Embedded("user_files".into(), Some(format!("{id}/workshop"))))),
        ("guides", icons::BOOKS, t!("Guides"), String::new(), Target::Page(Page::Embedded("user_files".into(), Some(format!("{id}/guides"))))),
    ];

    let cols = if width >= 920.0 * s { 3 } else { 2 };
    let gap = 14.0 * s;
    let cell_w = (width - gap * (cols - 1) as f32) / cols as f32;
    let cell_h = 66.0 * s;

    for row in entries.chunks(cols) {
        for (c, (key, icon, label, count, target)) in row.iter().enumerate() {
            let rect = Rect::from_min_size(pos2(left + c as f32 * (cell_w + gap), y), vec2(cell_w, cell_h));
            let hit = bp.focus.item(ui, Id::unique(("bp-prof-link", id, *key)), rect, ui.clip_rect(), Place::Page);
            if rect.intersects(ui.clip_rect()) {
                let p = ui.painter();
                p.rect_filled(rect, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.80), f.t.raised, hit.glow));
                p.rect_stroke(rect, paint::radius(12.0 * s), Stroke::new(1.0, Color32::from_white_alpha(20)), egui::StrokeKind::Inside);
                p.text(pos2(rect.left() + 24.0 * s, rect.center().y), Align2::CENTER_CENTER, *icon, w::font(22.0 * s), f.t.accent);
                let right_pad = if !count.is_empty() { 76.0 * s } else { 36.0 * s };
                paint::line(p, pos2(rect.left() + 48.0 * s, rect.center().y), Align2::LEFT_CENTER, label, w::font_semibold(19.5 * s), f.t.text, rect.width() - 48.0 * s - right_pad);
                if !count.is_empty() {
                    p.text(pos2(rect.right() - 18.0 * s, rect.center().y), Align2::RIGHT_CENTER, count, w::font_bold(19.0 * s), f.t.text_dim);
                } else if matches!(target, Target::Web(_)) {
                    p.text(pos2(rect.right() - 18.0 * s, rect.center().y), Align2::RIGHT_CENTER, icons::EXTERNAL, w::font(16.0 * s), f.t.text_faint);
                } else {
                    p.text(pos2(rect.right() - 18.0 * s, rect.center().y), Align2::RIGHT_CENTER, icons::CARET_RIGHT, w::font(16.0 * s), f.t.text_faint);
                }
            }
            if hit.focused {
                bp.focus.ring_around(rect, 12.0 * s);
            }
            if hit.activated {
                match target {
                    Target::Page(p) => bp.open(p.clone(), f.now),
                    Target::Tab(tab) => bp.switch_tab(*tab, f.now, true),
                    Target::Web(url) => {
                        crate::links::open_http(url);
                    }
                }
            }
        }
        y += cell_h + gap;
    }
    y
}

fn profile_badges_strip(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, id: SteamId) -> f32 {
    let Some(Ok(b)) = f.app.state.badges.get(&id).cloned() else {
        return y;
    };
    let s = f.s;
    if section_header_button(
        bp,
        ui,
        f,
        left,
        &mut y,
        width,
        t!("Badges"),
        Some((Id::unique(("bp-prof-all-badges", id)), None, t!("See all"))),
    ) {
        bp.open(Page::ProfileBadges(id), f.now);
    }

    // Level + XP progress card
    let card = Rect::from_min_size(pos2(left, y), vec2(width, 94.0 * s));
    if card.intersects(ui.clip_rect()) {
        let p = ui.painter();
        p.rect_filled(card, paint::radius(14.0 * s), f.t.chrome.gamma_multiply(0.82));
        paint_level_badge(p, &f.t, pos2(card.left() + 42.0 * s, card.center().y), b.level, s);
        let tx = card.left() + 82.0 * s;
        let sub = [b.xp.as_str(), b.next.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("  ·  ");
        paint::line(p, pos2(tx, card.top() + 18.0 * s), Align2::LEFT_TOP, &t!("Level {n}", n = b.level), w::font_bold(22.0 * s), f.t.text, card.right() - tx - 24.0 * s);
        if !sub.is_empty() {
            paint::line(p, pos2(tx, card.top() + 46.0 * s), Align2::LEFT_TOP, &sub, w::font(16.0 * s), f.t.text_dim, card.right() - tx - 24.0 * s);
        }
        if !b.next.is_empty() {
            let track = Rect::from_min_size(pos2(tx, card.bottom() - 18.0 * s), vec2((card.right() - tx - 28.0 * s).min(460.0 * s), 6.0 * s));
            paint::bar(ui, &f.t, track, b.progress, false);
        }
    }
    y += 108.0 * s;

    if !b.items.is_empty() {
        let gap = 14.0 * s;
        let tile = 92.0 * s;
        let cols = (((width + gap) / (tile + gap)).floor() as usize).clamp(4, 10);
        for (i, badge) in b.items.iter().take(cols).enumerate() {
            let r = Rect::from_min_size(pos2(left + i as f32 * (tile + gap), y), vec2(tile, tile));
            let hit = bp.focus.item(ui, Id::unique(("bp-prof-badge-top", id, i)), r, ui.clip_rect(), Place::Page);
            if r.intersects(ui.clip_rect()) {
                ui.painter().rect_filled(r, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.82), f.t.raised, hit.glow));
                if let Some(u) = &badge.icon {
                    if !w::paint_contain(ui, r.shrink(10.0 * s), u, Align2::CENTER_CENTER) {
                        paint::skeleton(ui, &f.t, r.shrink(10.0 * s), paint::radius(8.0 * s));
                    }
                } else {
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, icons::MEDAL, w::font(32.0 * s), f.t.accent);
                }
            }
            if hit.focused {
                bp.focus.ring_around(r, 12.0 * s);
            }
            if hit.activated {
                open_badge(bp, id, badge, f.now);
            }
        }
        y += tile + 32.0 * s;
    } else {
        y += 20.0 * s;
    }
    y
}

fn profile_recent_games(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, id: SteamId) -> f32 {
    let s = f.s;
    heading(ui, f, &mut y, t!("Recent activity"));
    let Some(result) = f.app.state.player_games.get(&id).cloned() else {
        for _ in 0..3 {
            paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width, 76.0 * s)), paint::radius(12.0 * s));
            y += 88.0 * s;
        }
        return y + 16.0 * s;
    };
    let Ok(mut games) = result else {
        return y;
    };
    if games.is_empty() && id == f.app.state.steam_id {
        games = f
            .app
            .local
            .games
            .iter()
            .map(|g| model::PlayerGame {
                app_id: g.app_id,
                name: g.name.clone(),
                playtime_minutes: g.playtime_minutes.unwrap_or(0),
                playtime_2weeks: 0,
                last_played: g.last_played,
                icon: None,
            })
            .collect();
    }
    let total_hours: u64 = games.iter().map(|g| g.playtime_minutes).sum::<u64>() / 60;
    let recent_minutes: u64 = games.iter().map(|g| g.playtime_2weeks).sum();
    let stats = t!(
        "{hours} hours past 2 weeks · {total} hours total",
        hours = format!("{:.1}", recent_minutes as f64 / 60.0),
        total = total_hours
    );
    paint::line(ui.painter(), pos2(left, y - 10.0 * s), Align2::LEFT_TOP, &stats, w::font(17.5 * s), f.t.text_dim, width);
    y += 26.0 * s;

    games.sort_by_key(|g| std::cmp::Reverse((g.playtime_2weeks, g.last_played)));
    let shown: Vec<_> = games.iter().filter(|g| g.last_played.is_some() || g.playtime_2weeks > 0).take(4).collect();
    if shown.is_empty() {
        paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, t!("No recent games."), w::font(18.0 * s), f.t.text_faint, width);
        y += 38.0 * s;
    } else {
        for g in shown {
            y = profile_game_row(bp, ui, f, left, y, width, g, "recent");
        }
    }
    let (all_r, all_hit) = button(bp, ui, f, Id::unique(("bp-prof-all-games", id)), pos2(left, y + 6.0 * s), 0.0, Some(icons::BOOKS), &t!("See all {n} games", n = games.len()), Kind::Secondary, None);
    if all_hit.activated {
        bp.open(Page::ProfileGames(id), f.now);
    }
    all_r.bottom() + 34.0 * s
}

fn profile_game_row(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, y: f32, width: f32, g: &model::PlayerGame, salt: &str) -> f32 {
    let s = f.s;
    let rect = Rect::from_min_size(pos2(left, y), vec2(width, 84.0 * s));
    let hit = bp.focus.item(ui, Id::unique(("bp-prof-game", salt, g.app_id)), rect, ui.clip_rect(), Place::Page);
    if rect.intersects(ui.clip_rect()) {
        ui.painter().rect_filled(rect, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.80), f.t.raised, hit.glow));
        let img = Rect::from_center_size(pos2(rect.left() + 86.0 * s, rect.center().y), vec2(142.0 * s, 66.0 * s));
        if !w::paint_cover(ui, img, &art::header(g.app_id), paint::radius(8.0 * s)) {
            paint::skeleton(ui, &f.t, img, paint::radius(8.0 * s));
        }
        let owned = f.app.state.apps.as_ref().is_some_and(|a| a.contains_key(&g.app_id));
        let tx = img.right() + 18.0 * s;
        let right_reserve = if owned { 210.0 * s } else { 24.0 * s };
        let p = ui.painter();
        paint::line(p, pos2(tx, rect.top() + 17.0 * s), Align2::LEFT_TOP, &g.name, w::font_semibold(20.5 * s), f.t.text, rect.right() - tx - right_reserve);
        let mut sub = t!("{time} on record", time = ui::playtime(g.playtime_minutes));
        if g.playtime_2weeks > 0 {
            sub = format!("{sub} · {}", t!("{time} past 2 weeks", time = ui::playtime(g.playtime_2weeks)));
        }
        if let Some(when) = g.last_played {
            sub = format!("{sub} · {}", t!("last played {when}", when = ui::ago(when)));
        }
        paint::line(p, pos2(tx, rect.bottom() - 17.0 * s), Align2::LEFT_BOTTOM, &sub, w::font(16.0 * s), f.t.text_dim, rect.right() - tx - right_reserve);
        if owned {
            p.text(
                pos2(rect.right() - 20.0 * s, rect.center().y),
                Align2::RIGHT_CENTER,
                format!("{}  {}", icons::CHECK, t!("In your library")),
                w::font_semibold(16.0 * s),
                f.t.online,
            );
        }
    }
    if hit.focused {
        bp.focus.ring_around(rect, 12.0 * s);
    }
    if hit.activated {
        open_game(bp, f, g.app_id);
    }
    y + 96.0 * s
}

fn profile_showcase(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, id: SteamId) -> f32 {
    let Some(inv) = f.app.state.inventories.get(&(id, 753, 6)).cloned() else {
        return y;
    };
    if inv.items.is_empty() || inv.error.is_some() {
        return y;
    }
    let s = f.s;
    if section_header_button(
        bp,
        ui,
        f,
        left,
        &mut y,
        width,
        t!("Item showcase"),
        Some((Id::unique(("bp-prof-inv-open", id)), Some(icons::BACKPACK), t!("Open inventory"))),
    ) {
        bp.open(Page::Embedded("inventory".into(), Some(id.to_string())), f.now);
    }

    let mut items: Vec<&model::InventoryItem> = inv.items.iter().collect();
    items.sort_by_key(|i| std::cmp::Reverse((i.kind.to_lowercase().contains("background") as u8, i.marketable as u8)));
    let size = 112.0 * s;
    let gap = 14.0 * s;
    let cols = (((width + gap) / (size + gap)).floor() as usize).clamp(3, 10);
    for (i, item) in items.iter().take(cols).enumerate() {
        let r = Rect::from_min_size(pos2(left + i as f32 * (size + gap), y), vec2(size, size));
        let hit = bp.focus.item(ui, Id::unique(("bp-prof-showcase", id, i)), r, ui.clip_rect(), Place::Page);
        if r.intersects(ui.clip_rect()) {
            ui.painter().rect_filled(r, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.82), f.t.raised, hit.glow));
            if !w::paint_contain(ui, r.shrink(10.0 * s), &art::econ(&item.icon, 192), Align2::CENTER_CENTER) {
                paint::skeleton(ui, &f.t, r.shrink(10.0 * s), paint::radius(8.0 * s));
            }
            if let Some(c) = crate::builtin::inventory::rarity_color(item) {
                let bar = Rect::from_min_max(pos2(r.left() + 12.0 * s, r.bottom() - 7.0 * s), pos2(r.right() - 12.0 * s, r.bottom() - 3.5 * s));
                ui.painter().rect_filled(bar, paint::radius(2.0 * s), c);
            }
        }
        if hit.focused {
            bp.focus.ring_around(r, 12.0 * s);
        }
        if hit.activated {
            bp.open(Page::Embedded("inventory".into(), Some(format!("{id}/753"))), f.now);
        }
    }
    y + size + 36.0 * s
}

fn profile_most_played(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, id: SteamId) -> f32 {
    let mut games: Vec<(AppId, String, u64)> = match f.app.state.player_games.get(&id) {
        Some(Ok(g)) if !g.is_empty() => g.iter().map(|g| (g.app_id, g.name.clone(), g.playtime_minutes)).collect(),
        _ if id == f.app.state.steam_id => f.app.local.games.iter().map(|g| (g.app_id, g.name.clone(), g.playtime_minutes.unwrap_or(0))).collect(),
        _ => return y,
    };
    games.retain(|g| g.2 > 0);
    if games.is_empty() {
        return y;
    }
    games.sort_by_key(|g| std::cmp::Reverse(g.2));
    let max = games[0].2.max(1) as f32;
    let s = f.s;
    heading(ui, f, &mut y, t!("Most played"));
    for (app, name, minutes) in games.iter().take(6) {
        let rect = Rect::from_min_size(pos2(left, y), vec2(width, 66.0 * s));
        let hit = bp.focus.item(ui, Id::unique(("bp-prof-most", id, *app)), rect, ui.clip_rect(), Place::Page);
        if rect.intersects(ui.clip_rect()) {
            ui.painter().rect_filled(rect, paint::radius(10.0 * s), w::mix(f.t.chrome.gamma_multiply(0.80), f.t.raised, hit.glow));
            let img = Rect::from_center_size(pos2(rect.left() + 62.0 * s, rect.center().y), vec2(102.0 * s, 46.0 * s));
            if !w::paint_cover(ui, img, &art::header(*app), paint::radius(6.0 * s)) {
                paint::skeleton(ui, &f.t, img, paint::radius(6.0 * s));
            }
            let tx = img.right() + 16.0 * s;
            let tw = (rect.right() - tx - 20.0 * s).max(80.0);
            let p = ui.painter();
            paint::line(p, pos2(tx, rect.top() + 12.0 * s), Align2::LEFT_TOP, name, w::font_semibold(18.5 * s), f.t.text, tw - 140.0 * s);
            p.text(pos2(rect.right() - 20.0 * s, rect.top() + 14.0 * s), Align2::RIGHT_TOP, ui::playtime(*minutes), w::font_medium(16.5 * s), f.t.text_dim);
            let bar = Rect::from_min_size(pos2(tx, rect.bottom() - 18.0 * s), vec2(tw, 6.0 * s));
            paint::bar(ui, &f.t, bar, *minutes as f32 / max, false);
        }
        if hit.focused {
            bp.focus.ring_around(rect, 10.0 * s);
        }
        if hit.activated {
            open_game(bp, f, *app);
        }
        y += 76.0 * s;
    }
    y + 24.0 * s
}

fn profile_friends_preview(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32) -> f32 {
    let friends = f.app.state.friend_list();
    if friends.is_empty() {
        return y;
    }
    let s = f.s;
    let online = friends.iter().filter(|p| p.state != model::PersonaState::Offline).count();
    let title = format!("{}  ·  {}", t!("Friends"), t!("{n} online", n = online));
    let all_label = t!("See all {n} friends", n = friends.len());
    let btn = (friends.len() > 8).then_some((Id::unique("bp-prof-all-friends"), Some(icons::USERS), all_label.as_str()));
    if section_header_button(bp, ui, f, left, &mut y, width, &title, btn) {
        bp.switch_tab(Tab::Friends, f.now, true);
    }
    let cols = 2;
    let gap = 14.0 * s;
    let cell_w = (width - gap) / cols as f32;
    let cell_h = 74.0 * s;
    for row in friends.iter().take(8).collect::<Vec<_>>().chunks(cols) {
        for (c, fr) in row.iter().enumerate() {
            let rect = Rect::from_min_size(pos2(left + c as f32 * (cell_w + gap), y), vec2(cell_w, cell_h));
            let hit = bp.focus.item(ui, Id::unique(("bp-prof-fr", fr.steam_id)), rect, ui.clip_rect(), Place::Page);
            if rect.intersects(ui.clip_rect()) {
                let col = crate::shell::status_color(&f.t, fr);
                ui.painter().rect_filled(rect, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.80), f.t.raised, hit.glow));
                let av = Rect::from_center_size(pos2(rect.left() + 38.0 * s, rect.center().y), vec2(48.0 * s, 48.0 * s));
                super::menu::avatar(ui, &f.t, av, fr.avatar_hash.as_deref(), col, s);
                let tx = av.right() + 14.0 * s;
                let p = ui.painter();
                paint::line(p, pos2(tx, rect.top() + 14.0 * s), Align2::LEFT_TOP, &fr.display_name(), w::font_semibold(19.0 * s), f.t.text, rect.right() - tx - 16.0 * s);
                paint::line(p, pos2(tx, rect.bottom() - 14.0 * s), Align2::LEFT_BOTTOM, &crate::shell::status_text(&f.app.state, fr), w::font(15.5 * s), col, rect.right() - tx - 16.0 * s);
            }
            if hit.focused {
                bp.focus.ring_around(rect, 12.0 * s);
            }
            if hit.activated {
                bp.open(Page::Profile(fr.steam_id), f.now);
            }
        }
        y += cell_h + gap;
    }
    y + 24.0 * s
}

fn profile_comments(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, id: SteamId, mine: bool) -> f32 {
    let s = f.s;
    let total = match f.app.state.comments.get(&id) {
        Some(Ok(c)) => c.total,
        _ => 0,
    };
    let title = if total > 0 { format!("{}  ({total})", t!("Comments")) } else { t!("Comments").to_string() };
    if section_header_button(
        bp,
        ui,
        f,
        left,
        &mut y,
        width,
        &title,
        Some((Id::unique(("bp-prof-c-refresh", id)), Some(icons::REFRESH), t!("Refresh"))),
    ) {
        bp.profile.comment_start.remove(&id);
        f.app.client.send(Command::LoadComments { steam_id: id, start: 0 });
    }

    let data = match f.app.state.comments.get(&id).cloned() {
        None => {
            for _ in 0..2 {
                paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width, 72.0 * s)), paint::radius(12.0 * s));
                y += 84.0 * s;
            }
            return y;
        }
        Some(Err(e)) => {
            let why = if e.to_lowercase().contains("private") {
                t!("This profile's comments are private.").to_string()
            } else {
                t!("Comments aren't available on this profile: {reason}", reason = i18n::tr_dyn(&e))
            };
            let r = Rect::from_min_size(pos2(left, y), vec2(width, 66.0 * s));
            ui.painter().rect_filled(r, paint::radius(12.0 * s), f.t.chrome.gamma_multiply(0.8));
            ui.painter().text(pos2(left + 22.0 * s, r.center().y), Align2::LEFT_CENTER, icons::LOCK_SIMPLE, w::font(22.0 * s), f.t.text_faint);
            paint::line(ui.painter(), pos2(left + 58.0 * s, r.center().y), Align2::LEFT_CENTER, &why, w::font(18.0 * s), f.t.text_dim, width - 80.0 * s);
            return y + 84.0 * s;
        }
        Some(Ok(c)) => c,
    };

    // Comment composer toggle (overlay OSK opens on A/click or X when focused)
    let hint = if mine { t!("Write a comment on your profile…") } else { t!("Add a comment…") };
    let post_w = paint::pill_width(ui, true, t!("Post comment"), s);
    let composer_w = (width - post_w - 16.0 * s).max(200.0 * s);
    let composer_r = Rect::from_min_size(pos2(left, y), vec2(composer_w, 64.0 * s));
    if bp.profile.comment_open {
        bp.osk.field_rect = Some(composer_r);
    }
    let comp_hit = bp.focus.item(ui, Id::unique(("bp-prof-c-input", id)), composer_r, ui.clip_rect(), Place::Page);
    if composer_r.intersects(ui.clip_rect()) {
        let p = ui.painter();
        p.rect_filled(composer_r, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.86), f.t.raised, comp_hit.glow));
        p.rect_stroke(composer_r, paint::radius(12.0 * s), Stroke::new(1.5 * s, if bp.profile.comment_open { f.t.accent } else { Color32::from_white_alpha(28) }), egui::StrokeKind::Inside);
        let inner = Rect::from_min_max(
            pos2(composer_r.left() + 18.0 * s, composer_r.top() + 8.0 * s),
            pos2(composer_r.right() - 88.0 * s, composer_r.bottom() - 8.0 * s),
        );
        osk::paint_single_line_field(
            p,
            inner,
            &bp.profile.comment_draft,
            hint,
            w::font(18.5 * s),
            &f.t,
            f.now,
            bp.profile.comment_open || comp_hit.focused,
            if bp.profile.comment_open { bp.osk.caret } else { bp.profile.comment_draft.chars().count() },
            bp.profile.comment_open && bp.osk.selected_all,
        );
        p.text(pos2(composer_r.right() - 16.0 * s, composer_r.center().y), Align2::RIGHT_CENTER, format!("{}/1000", bp.profile.comment_draft.chars().count()), w::font(14.5 * s), f.t.text_faint);
    }
    if comp_hit.focused {
        bp.focus.ring_around(composer_r, 12.0 * s);
    }
    if comp_hit.activated || (comp_hit.focused && f.x && !bp.profile.comment_open) {
        bp.profile.comment_open = !bp.profile.comment_open;
        if bp.profile.comment_open {
            bp.osk.reset_focus();
        }
    }

    let trimmed = bp.profile.comment_draft.trim().to_string();
    let can_post = !trimmed.is_empty() && trimmed.chars().count() <= 1000;
    let (post_r, post_hit) = button(
        bp,
        ui,
        f,
        Id::unique(("bp-prof-c-post", id)),
        pos2(composer_r.right() + 16.0 * s, y),
        post_w,
        Some(icons::SEND),
        t!("Post comment"),
        if can_post { Kind::Primary } else { Kind::Secondary },
        None,
    );
    if post_hit.activated && can_post {
        f.app.client.send(Command::PostComment { steam_id: id, text: trimmed });
    }
    y = composer_r.bottom().max(post_r.bottom()) + 18.0 * s;

    if data.items.is_empty() {
        paint::line(ui.painter(), pos2(left, y + 6.0 * s), Align2::LEFT_TOP, t!("No comments yet."), w::font(18.0 * s), f.t.text_faint, width);
        return y + 48.0 * s;
    }

    let my_id = f.app.state.steam_id;
    let del_w = paint::pill_width(ui, true, t!("Delete"), s);
    for c in &data.items {
        let can_del = mine || c.author == my_id;
        let body = plain_markup(&c.html);
        let est_lines = ((body.len() as f32 / 78.0).ceil() as usize).clamp(1, 5);
        let card_h = ((64.0 + est_lines as f32 * 25.0) * s).max(94.0 * s);
        let card = Rect::from_min_size(pos2(left, y), vec2(width, card_h));
        let author_w = if can_del { (width - del_w - 28.0 * s).max(200.0 * s) } else { width };
        let author_rect = Rect::from_min_size(card.min, vec2(author_w, card_h));
        let hit = bp.focus.item(ui, Id::unique(("bp-prof-comment", c.id)), author_rect, ui.clip_rect(), Place::Page);
        if card.intersects(ui.clip_rect()) {
            ui.painter().rect_filled(card, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.80), f.t.raised, hit.glow));
            let av = Rect::from_min_size(card.min + vec2(16.0 * s, 16.0 * s), vec2(44.0 * s, 44.0 * s));
            match &c.avatar {
                Some(u) if w::paint_cover(ui, av, u, paint::radius(8.0 * s)) => {}
                _ => paint::skeleton(ui, &f.t, av, paint::radius(8.0 * s)),
            }
            let tx = av.right() + 14.0 * s;
            let p = ui.painter();
            let nw = paint::text_width(p, &c.name, w::font_semibold(18.5 * s)).min(author_w - 180.0 * s);
            paint::line(p, pos2(tx, card.top() + 16.0 * s), Align2::LEFT_TOP, &c.name, w::font_semibold(18.5 * s), f.t.accent, nw);
            if c.time > 0 {
                paint::line(p, pos2(tx + nw + 14.0 * s, card.top() + 19.0 * s), Align2::LEFT_TOP, &ui::ago(c.time), w::font(14.5 * s), f.t.text_faint, 160.0 * s);
            }
            paint::wrapped(p, pos2(tx, card.top() + 44.0 * s), &body, w::font(17.5 * s), f.t.text, author_w - (tx - card.left()) - 14.0 * s, 5);
        }
        if hit.focused {
            bp.focus.ring_around(card, 12.0 * s);
        }
        if hit.activated {
            bp.open(Page::Profile(c.author), f.now);
        }
        if can_del {
            let (_, del_hit) = button(
                bp,
                ui,
                f,
                Id::unique(("bp-prof-c-del", c.id)),
                pos2(card.right() - del_w - 14.0 * s, card.center().y - 32.0 * s),
                del_w,
                Some(icons::TRASH),
                t!("Delete"),
                Kind::Danger,
                None,
            );
            if del_hit.activated {
                f.app.client.send(Command::DeleteComment { steam_id: id, comment_id: c.id });
            }
        }
        y += card_h + 12.0 * s;
    }

    let start = bp.profile.comment_start.get(&id).copied().unwrap_or(0);
    if data.total as usize > data.items.len() {
        y += 8.0 * s;
        let mut bx = left;
        if start > 0 {
            let (r, hit) = button(bp, ui, f, Id::unique(("bp-prof-c-newer", id)), pos2(bx, y), 0.0, Some(icons::CARET_LEFT), t!("Newer"), Kind::Secondary, None);
            if hit.activated {
                let next = start.saturating_sub(20);
                bp.profile.comment_start.insert(id, next);
                f.app.client.send(Command::LoadComments { steam_id: id, start: next });
            }
            bx = r.right() + 16.0 * s;
        }
        let page = start / 20 + 1;
        let pages = data.total.div_ceil(20).max(1);
        let label = t!("Page {page} of {pages}", page = page, pages = pages);
        let lw = paint::text_width(ui.painter(), &label, w::font_medium(18.0 * s));
        ui.painter().text(pos2(bx, y + 30.0 * s), Align2::LEFT_CENTER, &label, w::font_medium(18.0 * s), f.t.text_dim);
        bx += lw + 18.0 * s;
        if start + 20 < data.total {
            let (_, hit) = button(bp, ui, f, Id::unique(("bp-prof-c-older", id)), pos2(bx, y), 0.0, Some(icons::CARET_RIGHT), t!("Older"), Kind::Secondary, None);
            if hit.activated {
                let next = start + 20;
                bp.profile.comment_start.insert(id, next);
                f.app.client.send(Command::LoadComments { steam_id: id, start: next });
            }
        }
        y += 76.0 * s;
    }
    y
}

// ============================================================================
// Profile Games Subpage (`Page::ProfileGames(SteamId)`)
// ============================================================================

pub(super) fn games_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, raw_id: SteamId) -> f32 {
    let id = if raw_id == 0 { f.app.state.steam_id } else { raw_id };
    let (mine, _, _) = ensure_profile_loaded(bp, f, id);
    let name = profile_display_name(&f.app.state, id);
    let s = f.s;
    let left = f.view.left() + f.pad;
    let width = (f.view.width() - 2.0 * f.pad).min(1380.0 * s);
    let mut y = top + 24.0 * s;

    if f.y && !bp.profile.games_search_open {
        bp.profile.games_sort = bp.profile.games_sort.next();
    }

    let title = if mine { t!("Your games").to_string() } else { t!("{name}'s games", name = name) };
    heading(ui, f, &mut y, &title);

    let games = match f.app.state.player_games.get(&id).cloned() {
        None => {
            for _ in 0..7 {
                paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width, 84.0 * s)), paint::radius(12.0 * s));
                y += 96.0 * s;
            }
            return y - top + 32.0 * s;
        }
        Some(Err(e)) => {
            let body = if e == "private" { t!("Its owner has chosen not to show their games.").to_string() } else { e };
            empty(bp, ui, f, y, icons::LOCK_SIMPLE, t!("These games are hidden"), &body, None);
            return y - top + 340.0 * s;
        }
        Some(Ok(g)) => g,
    };

    let hours: u64 = games.iter().map(|g| g.playtime_minutes).sum::<u64>() / 60;
    let shared = if mine { 0 } else { games.iter().filter(|g| f.app.state.apps.as_ref().is_some_and(|a| a.contains_key(&g.app_id))).count() };
    let mut sub = t!("{n} games · {hours} hours on record", n = games.len(), hours = hours);
    if !mine && shared > 0 {
        sub = format!("{sub} · {}", t!("{n} in common with you", n = shared));
    }
    paint::line(ui.painter(), pos2(left, y - 10.0 * s), Align2::LEFT_TOP, &sub, w::font(18.0 * s), f.t.text_dim, width);
    y += 30.0 * s;

    // Search + Sort bar
    let search_label = if bp.profile.games_search.is_empty() {
        t!("Search games").to_string()
    } else {
        format!("\"{}\"", bp.profile.games_search)
    };
    let (sr, shit) = button(
        bp,
        ui,
        f,
        Id::unique(("bp-prof-games-search", id)),
        pos2(left, y),
        340.0 * s,
        Some(icons::SEARCH),
        &search_label,
        if bp.profile.games_search_open || !bp.profile.games_search.is_empty() { Kind::Primary } else { Kind::Secondary },
        None,
    );
    if shit.activated || (shit.focused && f.x && !bp.profile.games_search_open) {
        bp.profile.games_search_open = !bp.profile.games_search_open;
        if bp.profile.games_search_open {
            bp.osk.reset_focus();
        }
    }
    let sort_label = format!("{}  {}", t!("Sort"), bp.profile.games_sort.label());
    let (sor, sohit) = button(bp, ui, f, Id::unique(("bp-prof-games-sort", id)), pos2(sr.right() + 14.0 * s, y), 0.0, Some(icons::SLIDERS), &sort_label, Kind::Secondary, None);
    if sohit.activated {
        bp.profile.games_sort = bp.profile.games_sort.next();
    }
    let mut bar_bottom = sr.bottom().max(sor.bottom());
    if !bp.profile.games_search.is_empty() {
        let clr_w = paint::pill_width(ui, true, t!("Clear"), s);
        let (cx, cy) = if sor.right() + 14.0 * s + clr_w > left + width {
            (left, bar_bottom + 12.0 * s)
        } else {
            (sor.right() + 14.0 * s, y)
        };
        let (cr, clr) = button(bp, ui, f, Id::unique(("bp-prof-games-clear", id)), pos2(cx, cy), 0.0, Some(icons::X), t!("Clear"), Kind::Secondary, None);
        bar_bottom = bar_bottom.max(cr.bottom());
        if clr.activated {
            bp.profile.games_search.clear();
        }
    }
    y = bar_bottom + 18.0 * s;

    if bp.profile.games_search_open {
        let field = Rect::from_min_size(pos2(left, y), vec2(width.min(720.0 * s), 58.0 * s));
        bp.osk.field_rect = Some(field);
        let p = ui.painter();
        p.rect_filled(field, paint::radius(29.0 * s), Color32::from_black_alpha(120));
        p.rect_stroke(field, paint::radius(29.0 * s), Stroke::new(2.0 * s, f.t.accent.gamma_multiply(0.8)), egui::StrokeKind::Inside);
        p.text(pos2(field.left() + 24.0 * s, field.center().y), Align2::LEFT_CENTER, icons::SEARCH, w::font(22.0 * s), f.t.text_dim);
        let inner = Rect::from_min_max(pos2(field.left() + 60.0 * s, field.top() + 8.0 * s), pos2(field.right() - 24.0 * s, field.bottom() - 8.0 * s));
        osk::paint_single_line_field(p, inner, &bp.profile.games_search, t!("Search games"), w::font_semibold(21.0 * s), &f.t, f.now, true, bp.osk.caret, bp.osk.selected_all);
        y += 74.0 * s;
    }

    let q = bp.profile.games_search.trim().to_lowercase();
    let mut list: Vec<_> = games.iter().filter(|g| q.is_empty() || g.name.to_lowercase().contains(&q)).collect();
    match bp.profile.games_sort {
        ProfileGameSort::Recent => list.sort_by_key(|g| std::cmp::Reverse(g.last_played.unwrap_or(0))),
        ProfileGameSort::Playtime => list.sort_by_key(|g| std::cmp::Reverse(g.playtime_minutes)),
        ProfileGameSort::Name => list.sort_by(|a, b| model::cmp_ignore_case(&a.name, &b.name)),
    }
    if list.is_empty() {
        paint::line(ui.painter(), pos2(left, y + 10.0 * s), Align2::LEFT_TOP, t!("No games match your search."), w::font(19.0 * s), f.t.text_faint, width);
        return y - top + 80.0 * s;
    }
    for g in list {
        y = profile_game_row(bp, ui, f, left, y, width, g, "list");
    }
    y - top + 36.0 * s
}

// ============================================================================
// Profile Badges Subpage (`Page::ProfileBadges(SteamId)`)
// ============================================================================

pub(super) fn badges_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, raw_id: SteamId) -> f32 {
    let id = if raw_id == 0 { f.app.state.steam_id } else { raw_id };
    let (mine, _, _) = ensure_profile_loaded(bp, f, id);
    bp.profile.request_social(&f.app.state, &f.app.client, id);
    let name = profile_display_name(&f.app.state, id);
    let s = f.s;
    let left = f.view.left() + f.pad;
    let width = (f.view.width() - 2.0 * f.pad).min(1380.0 * s);
    let mut y = top + 24.0 * s;

    let title = if mine { t!("Your badges").to_string() } else { t!("{name}'s badges", name = name) };
    heading(ui, f, &mut y, &title);

    let b = match f.app.state.badges.get(&id).cloned() {
        None => {
            for _ in 0..6 {
                paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width, 84.0 * s)), paint::radius(12.0 * s));
                y += 96.0 * s;
            }
            return y - top + 32.0 * s;
        }
        Some(Err(e)) => {
            let body = if e == "private" { t!("Its owner has chosen to keep it private.").to_string() } else { e };
            empty(bp, ui, f, y, icons::LOCK_SIMPLE, t!("These badges are hidden"), &body, None);
            return y - top + 340.0 * s;
        }
        Some(Ok(b)) => b,
    };

    // Level card
    let card = Rect::from_min_size(pos2(left, y), vec2(width, 98.0 * s));
    if card.intersects(ui.clip_rect()) {
        let p = ui.painter();
        p.rect_filled(card, paint::radius(14.0 * s), f.t.chrome.gamma_multiply(0.84));
        paint_level_badge(p, &f.t, pos2(card.left() + 44.0 * s, card.center().y), b.level, s);
        let tx = card.left() + 86.0 * s;
        paint::line(p, pos2(tx, card.top() + 18.0 * s), Align2::LEFT_TOP, &t!("Level {n}", n = b.level), w::font_bold(23.0 * s), f.t.text, card.right() - tx - 24.0 * s);
        let sub = [b.xp.as_str(), b.next.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("  ·  ");
        if !sub.is_empty() {
            paint::line(p, pos2(tx, card.top() + 48.0 * s), Align2::LEFT_TOP, &sub, w::font(16.5 * s), f.t.text_dim, card.right() - tx - 24.0 * s);
        }
        if !b.next.is_empty() {
            let track = Rect::from_min_size(pos2(tx, card.bottom() - 18.0 * s), vec2((card.right() - tx - 28.0 * s).min(480.0 * s), 6.0 * s));
            paint::bar(ui, &f.t, track, b.progress, false);
        }
    }
    y += 116.0 * s;
    paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, &t!("{n} badges", n = b.items.len()), w::font_semibold(18.0 * s), f.t.text_dim, width);
    y += 34.0 * s;

    if b.items.is_empty() {
        paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, t!("No badges yet."), w::font(18.0 * s), f.t.text_faint, width);
        return y - top + 60.0 * s;
    }

    for (i, badge) in b.items.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(left, y), vec2(width, 92.0 * s));
        let hit = bp.focus.item(ui, Id::unique(("bp-prof-badge", id, i)), rect, ui.clip_rect(), Place::Page);
        if rect.intersects(ui.clip_rect()) {
            ui.painter().rect_filled(rect, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.80), f.t.raised, hit.glow));
            let icon_r = Rect::from_center_size(pos2(rect.left() + 48.0 * s, rect.center().y), vec2(68.0 * s, 68.0 * s));
            if let Some(u) = &badge.icon {
                if !w::paint_contain(ui, icon_r, u, Align2::CENTER_CENTER) {
                    paint::skeleton(ui, &f.t, icon_r, paint::radius(8.0 * s));
                }
            } else {
                ui.painter().text(icon_r.center(), Align2::CENTER_CENTER, icons::MEDAL, w::font(32.0 * s), f.t.accent);
            }
            let tx = icon_r.right() + 20.0 * s;
            let cards = card_set_of(&badge.url).is_some();
            let right_w = if cards { 230.0 * s } else { 44.0 * s };
            let p = ui.painter();
            paint::line(p, pos2(tx, rect.top() + 14.0 * s), Align2::LEFT_TOP, &badge.title, w::font_semibold(20.0 * s), f.t.text, rect.right() - tx - right_w);
            paint::line(p, pos2(tx, rect.center().y), Align2::LEFT_CENTER, &badge.detail, w::font(16.5 * s), f.t.accent, rect.right() - tx - right_w);
            paint::line(p, pos2(tx, rect.bottom() - 12.0 * s), Align2::LEFT_BOTTOM, &badge.unlocked, w::font(14.5 * s), f.t.text_faint, rect.right() - tx - right_w);
            if cards {
                p.text(pos2(rect.right() - 18.0 * s, rect.center().y), Align2::RIGHT_CENTER, format!("{}  {}", t!("Card collection"), icons::CARET_RIGHT), w::font_medium(16.5 * s), f.t.text_dim);
            } else if !badge.url.is_empty() {
                p.text(pos2(rect.right() - 18.0 * s, rect.center().y), Align2::RIGHT_CENTER, icons::EXTERNAL, w::font(16.0 * s), f.t.text_faint);
            }
        }
        if hit.focused {
            bp.focus.ring_around(rect, 12.0 * s);
        }
        if hit.activated {
            open_badge(bp, id, badge, f.now);
        }
        y += 104.0 * s;
    }
    y - top + 36.0 * s
}

// ============================================================================
// Profile Card Collection Subpage (`Page::ProfileCards(SteamId, AppId, bool)`)
// ============================================================================

pub(super) fn cards_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, raw_id: SteamId, app: AppId, foil: bool) -> f32 {
    let id = if raw_id == 0 { f.app.state.steam_id } else { raw_id };
    if bp.profile.lists_requested.insert((id, 4, app * 2 + u32::from(foil))) {
        f.app.client.send(Command::LoadCardSet { steam_id: id, app_id: app, foil });
    }
    let s = f.s;
    let left = f.view.left() + f.pad;
    let width = (f.view.width() - 2.0 * f.pad).min(1380.0 * s);
    let mut y = top + 24.0 * s;

    let set = match f.app.state.card_sets.get(&(id, app, foil)).cloned() {
        None => {
            heading(ui, f, &mut y, t!("Card collection"));
            for _ in 0..4 {
                paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width, 96.0 * s)), paint::radius(12.0 * s));
                y += 108.0 * s;
            }
            return y - top + 32.0 * s;
        }
        Some(Err(e)) => {
            let body = if e == "private" { t!("Its owner has chosen to keep it private.").to_string() } else { e };
            empty(bp, ui, f, y, icons::LOCK_SIMPLE, t!("This card collection is hidden"), &body, None);
            return y - top + 340.0 * s;
        }
        Some(Ok(s)) => s,
    };

    // Game header row
    let header_r = Rect::from_min_size(pos2(left, y), vec2(width, 108.0 * s));
    let h_hit = bp.focus.item(ui, Id::unique(("bp-cards-game", app)), header_r, ui.clip_rect(), Place::Page);
    if header_r.intersects(ui.clip_rect()) {
        ui.painter().rect_filled(header_r, paint::radius(14.0 * s), w::mix(f.t.chrome.gamma_multiply(0.84), f.t.raised, h_hit.glow));
        let img = Rect::from_center_size(pos2(header_r.left() + 114.0 * s, header_r.center().y), vec2(196.0 * s, 82.0 * s));
        if !w::paint_cover(ui, img, &art::header(app), paint::radius(8.0 * s)) {
            paint::skeleton(ui, &f.t, img, paint::radius(8.0 * s));
        }
        let tx = img.right() + 22.0 * s;
        let p = ui.painter();
        paint::line(p, pos2(tx, header_r.top() + 22.0 * s), Align2::LEFT_TOP, &set.title, w::font_bold(26.0 * s), f.t.text, header_r.right() - tx - 24.0 * s);
        let sub = [set.playtime.as_str(), set.drops.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("  ·  ");
        if !sub.is_empty() {
            paint::line(p, pos2(tx, header_r.bottom() - 24.0 * s), Align2::LEFT_BOTTOM, &sub, w::font(17.0 * s), f.t.text_dim, header_r.right() - tx - 24.0 * s);
        }
    }
    if h_hit.focused {
        bp.focus.ring_around(header_r, 14.0 * s);
    }
    if h_hit.activated {
        open_game(bp, f, app);
    }
    y += 126.0 * s;

    // Normal / Foil switcher
    let (nr, nhit) = button(bp, ui, f, Id::unique(("bp-cards-normal", app)), pos2(left, y), 150.0 * s, None, t!("Normal"), if !foil { Kind::Primary } else { Kind::Secondary }, None);
    let (fr, fhit) = button(bp, ui, f, Id::unique(("bp-cards-foil", app)), pos2(nr.right() + 14.0 * s, y), 150.0 * s, None, t!("Foil"), if foil { Kind::Primary } else { Kind::Secondary }, None);
    if nhit.activated && foil {
        bp.replace_top(Page::ProfileCards(id, app, false), f.now);
    } else if fhit.activated && !foil {
        bp.replace_top(Page::ProfileCards(id, app, true), f.now);
    }
    y = nr.bottom().max(fr.bottom()) + 20.0 * s;

    // Crafted badge
    if let Some(b) = &set.badge {
        let br = Rect::from_min_size(pos2(left, y), vec2(width, 88.0 * s));
        if br.intersects(ui.clip_rect()) {
            ui.painter().rect_filled(br, paint::radius(12.0 * s), f.t.chrome.gamma_multiply(0.8));
            let ir = Rect::from_center_size(pos2(br.left() + 48.0 * s, br.center().y), vec2(66.0 * s, 66.0 * s));
            if let Some(u) = &b.icon {
                if !w::paint_contain(ui, ir, u, Align2::CENTER_CENTER) {
                    paint::skeleton(ui, &f.t, ir, paint::radius(8.0 * s));
                }
            }
            let tx = ir.right() + 18.0 * s;
            let p = ui.painter();
            paint::line(p, pos2(tx, br.top() + 14.0 * s), Align2::LEFT_TOP, &b.name, w::font_bold(19.5 * s), f.t.text, br.right() - tx - 20.0 * s);
            paint::line(p, pos2(tx, br.center().y), Align2::LEFT_CENTER, &b.level, w::font(16.0 * s), f.t.accent, br.right() - tx - 20.0 * s);
            paint::line(p, pos2(tx, br.bottom() - 12.0 * s), Align2::LEFT_BOTTOM, &b.unlocked, w::font(14.5 * s), f.t.text_faint, br.right() - tx - 20.0 * s);
        }
        y += 104.0 * s;
    } else {
        paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, t!("No badge crafted yet."), w::font(18.0 * s), f.t.text_faint, width);
        y += 34.0 * s;
    }

    let have = set.cards.iter().filter(|c| c.count > 0).count();
    let total = set.cards.len();
    paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, &t!("Collected {n} of {total} cards", n = have, total = total), w::font_bold(20.0 * s), f.t.text, width);
    y += 30.0 * s;
    paint::bar(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width.min(520.0 * s), 8.0 * s)), have as f32 / total.max(1) as f32, false);
    y += 18.0 * s;
    if have == total && total > 0 {
        paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, t!("Every card is collected: the next badge level can be crafted on Steam."), w::font(16.5 * s), f.t.online, width);
        y += 28.0 * s;
    }
    y += 12.0 * s;

    let tile_w = 172.0 * s;
    let tile_h = 264.0 * s;
    let gap = 16.0 * s;
    let cols = (((width + gap) / (tile_w + gap)).floor() as usize).max(2);
    for (row_i, row) in set.cards.chunks(cols).enumerate() {
        for (c_i, c) in row.iter().enumerate() {
            let r = Rect::from_min_size(pos2(left + c_i as f32 * (tile_w + gap), y), vec2(tile_w, tile_h));
            let hit = bp.focus.item(ui, Id::unique(("bp-card-tile", app, foil, row_i * cols + c_i)), r, ui.clip_rect(), Place::Page);
            if r.intersects(ui.clip_rect()) {
                ui.painter().rect_filled(r, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.82), f.t.raised, hit.glow));
                let img = Rect::from_min_size(r.min + vec2(8.0 * s, 8.0 * s), vec2(tile_w - 16.0 * s, 196.0 * s));
                if !w::paint_cover(ui, img, &c.image, paint::radius(8.0 * s)) {
                    paint::skeleton(ui, &f.t, img, paint::radius(8.0 * s));
                }
                let p = ui.painter();
                if c.count == 0 {
                    p.rect_filled(img, paint::radius(8.0 * s), f.t.bg.gamma_multiply(0.65));
                }
                if c.count > 1 {
                    paint::badge(p, img.right_top() + vec2(-8.0 * s, 8.0 * s), Align2::RIGHT_TOP, &format!("×{}", c.count), s * 0.85, f.t.accent, f.t.on_accent);
                }
                let col = if c.count > 0 { f.t.text } else { f.t.text_faint };
                paint::line(p, pos2(r.left() + 10.0 * s, img.bottom() + 14.0 * s), Align2::LEFT_TOP, &c.name, w::font_semibold(16.0 * s), col, tile_w - 20.0 * s);
                paint::line(p, pos2(r.left() + 10.0 * s, img.bottom() + 36.0 * s), Align2::LEFT_TOP, &c.series, w::font(14.0 * s), f.t.text_faint, tile_w - 20.0 * s);
            }
            if hit.focused {
                bp.focus.ring_around(r, 12.0 * s);
            }
        }
        y += tile_h + gap;
    }
    y - top + 36.0 * s
}

// ============================================================================
// Profile Reviews Subpage (`Page::ProfileReviews(SteamId)`)
// ============================================================================

pub(super) fn reviews_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, raw_id: SteamId) -> f32 {
    let id = if raw_id == 0 { f.app.state.steam_id } else { raw_id };
    let (mine, _, _) = ensure_profile_loaded(bp, f, id);
    let page = bp.profile.review_page.max(1);
    if bp.profile.lists_requested.insert((id, 2, page)) {
        f.app.client.send(Command::LoadReviews { steam_id: id, page });
    }
    let name = profile_display_name(&f.app.state, id);
    let s = f.s;
    let left = f.view.left() + f.pad;
    let width = (f.view.width() - 2.0 * f.pad).min(1380.0 * s);
    let mut y = top + 24.0 * s;

    let title = if mine { t!("Your reviews").to_string() } else { t!("{name}'s reviews", name = name) };
    heading(ui, f, &mut y, &title);

    let reviews = match f.app.state.reviews.get(&(id, page)).cloned() {
        None => {
            for _ in 0..4 {
                paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width, 148.0 * s)), paint::radius(12.0 * s));
                y += 164.0 * s;
            }
            return y - top + 32.0 * s;
        }
        Some(Err(e)) => {
            let body = if e == "private" { t!("Its owner has chosen to keep it private.").to_string() } else { e };
            empty(bp, ui, f, y, icons::LOCK_SIMPLE, t!("These reviews are hidden"), &body, None);
            return y - top + 340.0 * s;
        }
        Some(Ok(r)) => r,
    };

    paint::line(ui.painter(), pos2(left, y - 10.0 * s), Align2::LEFT_TOP, &t!("{n} reviews", n = reviews.total), w::font(18.0 * s), f.t.text_dim, width);
    y += 30.0 * s;

    if reviews.items.is_empty() {
        empty(bp, ui, f, y, icons::THUMBS_UP, t!("No reviews yet"), "", None);
        return y - top + 320.0 * s;
    }

    for (i, r) in reviews.items.iter().enumerate() {
        let body_lines = if r.text.is_empty() { 0 } else { ((r.text.len() as f32 / 86.0).ceil() as usize).clamp(1, 6) };
        let card_h = (118.0 + body_lines as f32 * 25.0 + if r.helpful.is_empty() { 0.0 } else { 26.0 }) * s;
        let card = Rect::from_min_size(pos2(left, y), vec2(width, card_h));
        let hit = bp.focus.item(ui, Id::unique(("bp-prof-rev", id, page, i)), card, ui.clip_rect(), Place::Page);
        if card.intersects(ui.clip_rect()) {
            ui.painter().rect_filled(card, paint::radius(14.0 * s), w::mix(f.t.chrome.gamma_multiply(0.82), f.t.raised, hit.glow));
            let img = Rect::from_min_size(card.min + vec2(18.0 * s, 18.0 * s), vec2(180.0 * s, 76.0 * s));
            let art_url = r.capsule.clone().unwrap_or_else(|| art::header(r.app_id));
            if !w::paint_cover(ui, img, &art_url, paint::radius(8.0 * s)) {
                paint::skeleton(ui, &f.t, img, paint::radius(8.0 * s));
            }
            let tx = img.right() + 20.0 * s;
            let tw = (card.right() - tx - 22.0 * s).max(100.0);
            let (icon, verdict, col) = if r.recommended {
                (icons::THUMBS_UP, t!("Recommended"), f.t.online)
            } else {
                (icons::THUMBS_DOWN, t!("Not Recommended"), f.t.danger)
            };
            let p = ui.painter();
            let head = match f.app.state.app_name(r.app_id) {
                Some(gname) => format!("{icon}  {verdict}  ·  {gname}"),
                None => format!("{icon}  {verdict}"),
            };
            paint::line(p, pos2(tx, card.top() + 20.0 * s), Align2::LEFT_TOP, &head, w::font_bold(20.5 * s), col, tw);
            let meta = [r.hours.as_str(), r.posted.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("  ·  ");
            if !meta.is_empty() {
                paint::line(p, pos2(tx, card.top() + 52.0 * s), Align2::LEFT_TOP, &meta, w::font(16.0 * s), f.t.text_dim, tw);
            }
            if !r.text.is_empty() {
                paint::wrapped(p, pos2(card.left() + 20.0 * s, img.bottom() + 14.0 * s), &r.text, w::font(17.5 * s), f.t.text, width - 40.0 * s, 6);
            }
            if !r.helpful.is_empty() {
                paint::line(p, pos2(card.left() + 20.0 * s, card.bottom() - 14.0 * s), Align2::LEFT_BOTTOM, &r.helpful, w::font(14.5 * s), f.t.text_faint, width - 40.0 * s);
            }
        }
        if hit.focused {
            bp.focus.ring_around(card, 14.0 * s);
        }
        if hit.activated {
            open_game(bp, f, r.app_id);
        }
        y += card_h + 16.0 * s;
    }

    let pages = reviews.total.div_ceil(REVIEWS_PER_PAGE).max(1);
    if pages > 1 {
        y += 8.0 * s;
        let mut bx = left;
        if page > 1 {
            let (r, hit) = button(bp, ui, f, Id::unique(("bp-prof-rev-prev", id)), pos2(bx, y), 0.0, Some(icons::CARET_LEFT), t!("Previous"), Kind::Secondary, None);
            if hit.activated {
                bp.profile.review_page = page - 1;
            }
            bx = r.right() + 16.0 * s;
        }
        let label = t!("Page {n} of {total}", n = page, total = pages);
        let lw = paint::text_width(ui.painter(), &label, w::font_medium(18.5 * s));
        ui.painter().text(pos2(bx, y + 30.0 * s), Align2::LEFT_CENTER, &label, w::font_medium(18.5 * s), f.t.text_dim);
        bx += lw + 18.0 * s;
        if page < pages {
            let (_, hit) = button(bp, ui, f, Id::unique(("bp-prof-rev-next", id)), pos2(bx, y), 0.0, Some(icons::CARET_RIGHT), t!("Next"), Kind::Secondary, None);
            if hit.activated {
                bp.profile.review_page = page + 1;
            }
        }
        y += 78.0 * s;
    }
    y - top + 36.0 * s
}

// ============================================================================
// Profile Groups Subpage (`Page::ProfileGroups(SteamId)`)
// ============================================================================

pub(super) fn groups_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, top: f32, raw_id: SteamId) -> f32 {
    let id = if raw_id == 0 { f.app.state.steam_id } else { raw_id };
    let (mine, _, _) = ensure_profile_loaded(bp, f, id);
    if bp.profile.lists_requested.insert((id, 3, 0)) {
        f.app.client.send(Command::LoadGroups { steam_id: id });
    }
    let name = profile_display_name(&f.app.state, id);
    let s = f.s;
    let left = f.view.left() + f.pad;
    let width = (f.view.width() - 2.0 * f.pad).min(1380.0 * s);
    let mut y = top + 24.0 * s;

    let title = if mine { t!("Your groups").to_string() } else { t!("{name}'s groups", name = name) };
    heading(ui, f, &mut y, &title);

    let groups = match f.app.state.groups.get(&id).cloned() {
        None => {
            for _ in 0..6 {
                paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width, 82.0 * s)), paint::radius(12.0 * s));
                y += 94.0 * s;
            }
            return y - top + 32.0 * s;
        }
        Some(Err(e)) => {
            let body = if e == "private" { t!("Its owner has chosen to keep it private.").to_string() } else { e };
            empty(bp, ui, f, y, icons::LOCK_SIMPLE, t!("These groups are hidden"), &body, None);
            return y - top + 340.0 * s;
        }
        Some(Ok(g)) => g,
    };

    paint::line(ui.painter(), pos2(left, y - 10.0 * s), Align2::LEFT_TOP, &t!("{n} groups", n = groups.len()), w::font(18.0 * s), f.t.text_dim, width);
    y += 30.0 * s;

    if groups.is_empty() {
        empty(bp, ui, f, y, icons::USERS_THREE, t!("Not in any groups"), "", None);
        return y - top + 320.0 * s;
    }

    for (i, g) in groups.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(left, y), vec2(width, 84.0 * s));
        let hit = bp.focus.item(ui, Id::unique(("bp-prof-group", id, i)), rect, ui.clip_rect(), Place::Page);
        if rect.intersects(ui.clip_rect()) {
            ui.painter().rect_filled(rect, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.80), f.t.raised, hit.glow));
            let img = Rect::from_center_size(pos2(rect.left() + 44.0 * s, rect.center().y), vec2(60.0 * s, 60.0 * s));
            if let Some(u) = &g.avatar {
                if !w::paint_cover(ui, img, u, paint::radius(8.0 * s)) {
                    paint::skeleton(ui, &f.t, img, paint::radius(8.0 * s));
                }
            } else {
                ui.painter().rect_filled(img, paint::radius(8.0 * s), f.t.raised);
            }
            let tx = img.right() + 18.0 * s;
            let sub = [g.members.as_str(), g.online.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("  ·  ");
            let p = ui.painter();
            paint::line(p, pos2(tx, rect.top() + 17.0 * s), Align2::LEFT_TOP, &g.name, w::font_semibold(20.0 * s), f.t.text, rect.right() - tx - 48.0 * s);
            paint::line(p, pos2(tx, rect.bottom() - 17.0 * s), Align2::LEFT_BOTTOM, &sub, w::font(16.0 * s), f.t.text_dim, rect.right() - tx - 48.0 * s);
            p.text(pos2(rect.right() - 18.0 * s, rect.center().y), Align2::RIGHT_CENTER, icons::EXTERNAL, w::font(16.0 * s), f.t.text_faint);
        }
        if hit.focused {
            bp.focus.ring_around(rect, 12.0 * s);
        }
        if hit.activated && !g.url.is_empty() {
            crate::links::open_http(&g.url);
        }
        y += 96.0 * s;
    }
    y - top + 36.0 * s
}

// ============================================================================
// Native Big Picture Edit Profile (`Page::ProfileEdit`)
// ============================================================================

fn original_edit(state: &client::State) -> Option<ProfileEdit> {
    state.profile_edit.as_ref().and_then(|r| r.as_ref().ok()).cloned()
}

pub(super) fn edit_page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame) -> f32 {
    let s = f.s;
    let me = f.app.state.steam_id;
    if !bp.profile.edit.asked {
        bp.profile.edit.asked = true;
        f.app.client.send(Command::LoadProfileEdit { language: lang() });
    }
    if bp.profile.edit.draft.is_none() && !bp.profile.edit.reloading {
        if let Some(Ok(e)) = &f.app.state.profile_edit {
            bp.profile.edit.draft = Some(e.clone());
        }
    }

    let left = f.view.left() + f.pad;
    let total_w = f.view.width() - 2.0 * f.pad;
    let rail_w = (260.0 * s).min(total_w * 0.25);
    let gap = 28.0 * s;
    let body_left = left + rail_w + gap;
    let body_w = (total_w - rail_w - gap).max(320.0 * s);

    let top = f.view.top() + 8.0 * s - 88.0 * s * bp.bar_hide;
    let bottom = f.view.bottom() - 10.0 * s;
    let rail_view = Rect::from_min_max(pos2(left - 10.0 * s, top), pos2(left + rail_w + 10.0 * s, bottom));
    let body_view = Rect::from_min_max(pos2(body_left - 10.0 * s, top), pos2(body_left + body_w + 10.0 * s, bottom));

    // ---- Left section rail ----
    let rail_clip = rail_view.intersect(ui.clip_rect());
    let mut rail_ui = ui.new_child(egui::UiBuilder::new().max_rect(rail_view));
    rail_ui.set_clip_rect(rail_clip);
    let mut ry = rail_view.top() + 4.0 * s - bp.profile.edit.rail_scroll.pos;
    let mut rail_focused = None;

    // Header
    paint::line(rail_ui.painter(), pos2(left + 6.0 * s, ry + 2.0 * s), Align2::LEFT_TOP, t!("Edit profile"), w::font_bold(24.0 * s), f.t.text, rail_w - 12.0 * s);
    ry += 40.0 * s;

    let nav_h = 42.0 * s;
    let nav_step = 48.0 * s;
    for sec in EditSection::ALL {
        let id = Id::unique(("bp-pedit-sec", sec as usize));
        let r = Rect::from_min_size(pos2(left, ry), vec2(rail_w, nav_h));
        let hit = bp.focus.item(&rail_ui, id, r, rail_clip, Place::Bar);
        let selected = bp.profile.edit.section == sec;
        if r.intersects(rail_clip) {
            let fill = if selected {
                f.t.accent.gamma_multiply(0.26 + 0.18 * hit.glow)
            } else {
                w::mix(Color32::TRANSPARENT, Color32::from_white_alpha(20), hit.glow)
            };
            let p = rail_ui.painter();
            p.rect_filled(r, paint::radius(9.0 * s), fill);
            if selected {
                let bar = Rect::from_min_size(pos2(r.left() + 3.0 * s, r.top() + 9.0 * s), vec2(3.5 * s, r.height() - 18.0 * s));
                p.rect_filled(bar, paint::radius(2.0 * s), f.t.accent);
            }
            let col = if selected { f.t.accent } else { f.t.text_dim };
            p.text(pos2(r.left() + 22.0 * s, r.center().y), Align2::CENTER_CENTER, sec.icon(), w::font(18.0 * s), col);
            paint::line(p, pos2(r.left() + 42.0 * s, r.center().y), Align2::LEFT_CENTER, sec.label(), w::font_semibold(16.5 * s), if selected { f.t.text } else { f.t.text_dim }, rail_w - 52.0 * s);
        }
        if hit.focused {
            bp.focus.ring_around_clipped(r, 9.0 * s, rail_clip);
            rail_focused = Some(r);
            if !bp.focus.by_pointer && bp.profile.edit.section != sec {
                bp.profile.edit.section = sec;
                bp.profile.edit.active_field = None;
                bp.profile.edit.filter.clear();
                bp.profile.edit.body_scroll = Scroller::default();
            }
        }
        if hit.activated {
            if bp.profile.edit.section != sec {
                bp.profile.edit.section = sec;
                bp.profile.edit.active_field = None;
                bp.profile.edit.filter.clear();
                bp.profile.edit.body_scroll = Scroller::default();
            }
        }
        ry += nav_step;
    }

    // Points Shop external shortcut
    ry += 8.0 * s;
    let ps_r = Rect::from_min_size(pos2(left, ry), vec2(rail_w, nav_h));
    let ps_hit = bp.focus.item(&rail_ui, Id::unique("bp-pedit-points"), ps_r, rail_clip, Place::Bar);
    if ps_r.intersects(rail_clip) {
        let p = rail_ui.painter();
        p.rect_filled(ps_r, paint::radius(9.0 * s), w::mix(Color32::TRANSPARENT, Color32::from_white_alpha(20), ps_hit.glow));
        p.text(pos2(ps_r.left() + 22.0 * s, ps_r.center().y), Align2::CENTER_CENTER, icons::STOREFRONT, w::font(18.0 * s), f.t.accent);
        paint::line(p, pos2(ps_r.left() + 42.0 * s, ps_r.center().y), Align2::LEFT_CENTER, t!("Points Shop"), w::font_semibold(16.5 * s), f.t.text_dim, rail_w - 72.0 * s);
        p.text(pos2(ps_r.right() - 14.0 * s, ps_r.center().y), Align2::RIGHT_CENTER, icons::EXTERNAL, w::font(14.0 * s), f.t.text_faint);
    }
    if ps_hit.focused {
        bp.focus.ring_around_clipped(ps_r, 9.0 * s, rail_clip);
        rail_focused = Some(ps_r);
    }
    if ps_hit.activated {
        crate::links::open_http("https://store.steampowered.com/points/shop");
    }
    ry += nav_step;

    let rail_h = ry - (rail_view.top() - bp.profile.edit.rail_scroll.pos) + 12.0 * s;
    if ui.rect_contains_pointer(rail_view) {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            bp.profile.edit.rail_scroll.target -= wheel;
        }
    }
    if bp.focus.moved && let Some(r) = rail_focused {
        let rtop = r.top() - (rail_view.top() - bp.profile.edit.rail_scroll.pos);
        bp.profile.edit.rail_scroll.reveal(rtop, rtop + r.height(), rail_view.height(), 20.0 * s);
    }
    bp.profile.edit.rail_scroll.clamp(rail_h, rail_view.height());
    bp.profile.edit.rail_scroll.step(ui);

    // ---- Right content pane ----
    let body_clip = body_view.intersect(ui.clip_rect());
    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body_view));
    body_ui.set_clip_rect(body_clip);
    let body_top = body_view.top() - bp.profile.edit.body_scroll.pos;
    let mut y = body_top + 6.0 * s;

    if let (None, Some(Err(e))) = (&bp.profile.edit.draft, &f.app.state.profile_edit) {
        paint::line(body_ui.painter(), pos2(body_left, y), Align2::LEFT_TOP, t!("Couldn't load your profile settings"), w::font_bold(24.0 * s), f.t.text, body_w);
        y += 38.0 * s;
        paint::wrapped(body_ui.painter(), pos2(body_left, y), &i18n::tr_dyn(e), w::font(18.0 * s), f.t.text_dim, body_w, 3);
        y += 66.0 * s;
        let (_, retry) = button(bp, &mut body_ui, f, Id::unique("bp-pedit-retry"), pos2(body_left, y), 180.0 * s, Some(icons::REFRESH), t!("Retry"), Kind::Primary, None);
        if retry.activated {
            bp.profile.edit.asked = false;
        }
        return f.view.height();
    }

    if bp.profile.edit.draft.is_none() {
        for _ in 0..5 {
            paint::skeleton(&body_ui, &f.t, Rect::from_min_size(pos2(body_left, y), vec2(body_w, 60.0 * s)), paint::radius(10.0 * s));
            y += 72.0 * s;
        }
        return f.view.height();
    }

    let sec = bp.profile.edit.section;
    y = match sec {
        EditSection::General => edit_general(bp, &mut body_ui, f, body_left, y, body_w, body_clip),
        EditSection::Avatar => edit_avatar(bp, &mut body_ui, f, body_left, y, body_w, body_clip, me),
        EditSection::Background => edit_background(bp, &mut body_ui, f, body_left, y, body_w, body_clip),
        EditSection::MiniProfile => edit_mini(bp, &mut body_ui, f, body_left, y, body_w, body_clip),
        EditSection::Theme => edit_theme(bp, &mut body_ui, f, body_left, y, body_w, body_clip),
        EditSection::Badge => edit_badge(bp, &mut body_ui, f, body_left, y, body_w, body_clip, me),
        EditSection::Privacy => edit_privacy(bp, &mut body_ui, f, body_left, y, body_w, body_clip),
    };

    let body_h = y - body_top + 32.0 * s;
    if ui.rect_contains_pointer(body_view) {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            bp.profile.edit.body_scroll.target -= wheel;
        }
    }
    if f.stick_scroll != 0.0 {
        bp.profile.edit.body_scroll.target += f.stick_scroll;
    }
    if bp.focus.moved
        && let Some((r, Place::Page)) = bp.focus.focused()
    {
        let rtop = r.top() - body_top;
        bp.profile.edit.body_scroll.reveal(rtop, rtop + r.height(), body_view.height(), 36.0 * s);
    }
    if !ui.is_enabled() && let Some(field) = bp.osk.take_field_reveal() {
        let top = field.top() - body_top;
        bp.profile.edit.body_scroll.reveal(top, top + field.height(), body_view.height(), 16.0 * s);
    }
    bp.profile.edit.body_scroll.clamp(body_h, body_view.height());
    bp.profile.edit.body_scroll.step(ui);
    if let Some((r, Place::Page)) = bp.focus.focused() {
        bp.focus.ring_around_clipped(r, 12.0 * s, body_clip);
    }
    f.view.height()
}

fn edit_section_header(ui: &Ui, f: &Frame, left: f32, y: &mut f32, width: f32, title: &str, body: &str) {
    let s = f.s;
    paint::line(ui.painter(), pos2(left, *y), Align2::LEFT_TOP, title, w::font_bold(26.0 * s), f.t.text, width);
    *y += 38.0 * s;
    if !body.is_empty() {
        let r = paint::wrapped(ui.painter(), pos2(left, *y), body, w::font(17.0 * s), f.t.text_dim, width, 3);
        *y = r.bottom() + 18.0 * s;
    } else {
        *y += 8.0 * s;
    }
}

fn edit_text_row(
    bp: &mut BigPicture,
    ui: &mut Ui,
    f: &mut Frame,
    left: f32,
    y: &mut f32,
    width: f32,
    clip: Rect,
    field: EditField,
    label: &str,
    hint: &str,
    value: &mut String,
) {
    let s = f.s;
    let active = bp.profile.edit.active_field == Some(field);
    let r = Rect::from_min_size(pos2(left, *y), vec2(width, 56.0 * s));
    if active {
        bp.osk.field_rect = Some(r);
    }
    let hit = bp.focus.item(ui, Id::unique(("bp-pedit-field", field as usize)), r, clip, Place::Page);
    if r.intersects(clip) {
        let p = ui.painter();
        p.rect_filled(r, paint::radius(10.0 * s), w::mix(f.t.chrome.gamma_multiply(0.84), f.t.raised, hit.glow));
        if active {
            p.rect_stroke(r, paint::radius(10.0 * s), Stroke::new(2.0 * s, f.t.accent), egui::StrokeKind::Inside);
        }
        paint::line(p, pos2(r.left() + 16.0 * s, r.top() + 8.0 * s), Align2::LEFT_TOP, label, w::font_semibold(13.5 * s), f.t.text_dim, width - 32.0 * s);
        let inner = Rect::from_min_max(pos2(r.left() + 16.0 * s, r.top() + 26.0 * s), pos2(r.right() - 16.0 * s, r.bottom() - 4.0 * s));
        osk::paint_single_line_field(
            p,
            inner,
            value,
            hint,
            w::font_medium(17.5 * s),
            &f.t,
            f.now,
            active,
            if active { bp.osk.caret } else { value.chars().count() },
            active && bp.osk.selected_all,
        );
    }
    if hit.focused {
        bp.focus.ring_around_clipped(r, 10.0 * s, clip);
    }
    if hit.activated || (hit.focused && f.x && !active) {
        bp.profile.edit.active_field = if active { None } else { Some(field) };
        if !active {
            bp.osk.reset_focus();
        }
    }
    *y += 64.0 * s;
}

fn edit_choice_row(
    bp: &mut BigPicture,
    ui: &Ui,
    f: &Frame,
    left: f32,
    y: &mut f32,
    width: f32,
    clip: Rect,
    id: Id,
    label: &str,
    sub: &str,
    value: &str,
) -> i32 {
    let s = f.s;
    let h = if sub.is_empty() { 50.0 } else { 68.0 } * s;
    let r = Rect::from_min_size(pos2(left, *y), vec2(width, h));
    let hit = bp.focus.item(ui, id, r, clip, Place::Page);
    if r.intersects(clip) {
        let p = ui.painter();
        p.rect_filled(r, paint::radius(10.0 * s), w::mix(f.t.chrome.gamma_multiply(0.84), f.t.raised, hit.glow));
        let val = format!("{}  {value}  {}", icons::CARET_LEFT, icons::CARET_RIGHT);
        let vw = paint::text_width(p, &val, w::font_semibold(16.5 * s)).min(width * 0.44);
        let lw = (width - vw - 44.0 * s).max(120.0 * s);
        if sub.is_empty() {
            paint::line(p, pos2(r.left() + 16.0 * s, r.center().y), Align2::LEFT_CENTER, label, w::font_semibold(17.5 * s), f.t.text, lw);
        } else {
            paint::line(p, pos2(r.left() + 16.0 * s, r.top() + 12.0 * s), Align2::LEFT_TOP, label, w::font_semibold(17.5 * s), f.t.text, lw);
            paint::line(p, pos2(r.left() + 16.0 * s, r.bottom() - 11.0 * s), Align2::LEFT_BOTTOM, sub, w::font(14.0 * s), f.t.text_dim, lw);
        }
        paint::line(p, pos2(r.right() - 16.0 * s, r.center().y), Align2::RIGHT_CENTER, &val, w::font_semibold(16.5 * s), f.t.accent, vw);
    }
    if hit.focused {
        bp.focus.ring_around_clipped(r, 10.0 * s, clip);
    }
    *y += h + 8.0 * s;
    if hit.focused && bp.focus.last_dir == Some(Dir::Left) {
        -1
    } else if hit.focused && bp.focus.last_dir == Some(Dir::Right) {
        1
    } else if hit.activated {
        2
    } else {
        0
    }
}

fn edit_save_bar(bp: &mut BigPicture, ui: &mut Ui, f: &Frame, left: f32, y: &mut f32, changed: bool, saving: bool, valid: bool, salt: &str) -> (bool, bool) {
    let s = f.s;
    *y += 8.0 * s;
    let mut save = false;
    let mut cancel = false;
    let save_label = if saving { t!("Saving…") } else { t!("Save") };
    let (sr, shit) = button(
        bp,
        ui,
        f,
        Id::unique(("bp-pedit-save", salt)),
        pos2(left, *y),
        170.0 * s,
        Some(icons::CHECK),
        save_label,
        if changed && valid && !saving { Kind::Primary } else { Kind::Secondary },
        None,
    );
    if shit.activated && changed && valid && !saving {
        save = true;
    }
    let (cr, chit) = button(bp, ui, f, Id::unique(("bp-pedit-cancel", salt)), pos2(sr.right() + 14.0 * s, *y), 150.0 * s, Some(icons::X), t!("Cancel"), Kind::Secondary, None);
    if chit.activated && changed && !saving {
        cancel = true;
    }
    *y = sr.bottom().max(cr.bottom()) + 18.0 * s;
    (save, cancel)
}

fn cycle_location(list: &[(String, String)], current: &mut String, delta: i32) -> bool {
    let mut codes: Vec<&str> = vec![""];
    codes.extend(list.iter().map(|(c, _)| c.as_str()));
    let idx = codes.iter().position(|c| *c == current.as_str()).unwrap_or(0);
    let next = super::settings::step_index(codes.len(), idx, delta, true);
    if next != idx {
        *current = codes[next].to_string();
        true
    } else {
        false
    }
}

fn edit_general(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, clip: Rect) -> f32 {
    let saving = bp.profile.edit.saving();
    edit_section_header(
        ui,
        f,
        left,
        &mut y,
        width,
        t!("About"),
        t!("Set your profile name and details. Providing additional information like your real name can help friends find you on the Steam Community."),
    );

    let mut d = match bp.profile.edit.draft.clone() {
        Some(d) => d,
        None => return y,
    };

    edit_text_row(bp, ui, f, left, &mut y, width, clip, EditField::PersonaName, &format!("{} *", t!("Profile name")), "", &mut d.persona_name);
    edit_text_row(bp, ui, f, left, &mut y, width, clip, EditField::RealName, t!("Real name"), t!("Optional"), &mut d.real_name);
    edit_text_row(bp, ui, f, left, &mut y, width, clip, EditField::CustomUrl, t!("Custom URL"), "steamcommunity.com/id/…", &mut d.custom_url);
    edit_text_row(bp, ui, f, left, &mut y, width, clip, EditField::Summary, t!("Summary"), t!("A few words about you, shown under your name."), &mut d.summary);

    // Location pickers
    let mut want = vec![(None, None)];
    if !d.country.is_empty() {
        want.push((Some(d.country.clone()), None));
    }
    if !d.country.is_empty() && !d.state.is_empty() {
        want.push((Some(d.country.clone()), Some(d.state.clone())));
    }
    for key in want {
        if bp.profile.edit.asked_locations.insert(key.clone()) {
            f.app.client.send(Command::LoadLocations { country: key.0, state: key.1 });
        }
    }
    let countries = f.app.state.locations.get(&(None, None)).cloned();
    let states = (!d.country.is_empty()).then(|| f.app.state.locations.get(&(Some(d.country.clone()), None)).cloned()).flatten();
    let cities = (!d.country.is_empty() && !d.state.is_empty())
        .then(|| f.app.state.locations.get(&(Some(d.country.clone()), Some(d.state.clone()))).cloned())
        .flatten();

    let name_of = |list: &Option<Vec<(String, String)>>, code: &str| -> String {
        if code.is_empty() {
            return t!("None").to_string();
        }
        list.as_ref()
            .and_then(|v| v.iter().find(|(c, _)| c == code).map(|(_, n)| n.clone()))
            .unwrap_or_else(|| code.to_string())
    };
    let delta = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-pedit-country"), t!("Country"), "", &name_of(&countries, &d.country));
    if delta != 0 && let Some(list) = &countries && cycle_location(list, &mut d.country, delta) {
        d.state.clear();
        d.city.clear();
    }
    if let Some(list) = states.filter(|v| !v.is_empty()) {
        let delta = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-pedit-state"), t!("State or region"), "", &name_of(&Some(list.clone()), &d.state));
        if delta != 0 && cycle_location(&list, &mut d.state, delta) {
            d.city.clear();
        }
    }
    if let Some(list) = cities.filter(|v| !v.is_empty()) {
        let delta = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-pedit-city"), t!("City"), "", &name_of(&Some(list.clone()), &d.city));
        if delta != 0 {
            cycle_location(&list, &mut d.city, delta);
        }
    }

    let delta = edit_choice_row(
        bp,
        ui,
        f,
        left,
        &mut y,
        width,
        clip,
        Id::unique("bp-pedit-awards"),
        t!("Hide the awards you've been given on your profile"),
        "",
        if d.hide_awards { t!("On") } else { t!("Off") },
    );
    if delta != 0 {
        d.hide_awards = !d.hide_awards;
    }

    let orig = original_edit(&f.app.state);
    let changed = orig.as_ref().is_some_and(|orig| {
        (&orig.persona_name, &orig.real_name, &orig.custom_url, &orig.country, &orig.state, &orig.city, &orig.summary, orig.hide_awards)
            != (&d.persona_name, &d.real_name, &d.custom_url, &d.country, &d.state, &d.city, &d.summary, d.hide_awards)
    });
    let (save, cancel) = edit_save_bar(bp, ui, f, left, &mut y, changed, saving, !d.persona_name.trim().is_empty(), "general");
    if save {
        bp.profile.send_saves(&f.app.client, vec![Command::SaveProfileInfo(Box::new(d.clone()))], vec![]);
    }
    bp.profile.edit.draft = if cancel { orig } else { Some(d) };
    y
}

fn edit_avatar(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, clip: Rect, me_id: SteamId) -> f32 {
    let s = f.s;
    let saving = bp.profile.edit.saving();
    let me = f.app.state.persona(me_id);
    let still = art::avatar_full(me.avatar_hash.as_deref());
    let mut d = match bp.profile.edit.draft.clone() {
        Some(d) => d,
        None => return y,
    };

    edit_section_header(ui, f, left, &mut y, width, t!("Avatar"), t!("Choose a picture for your profile. It's shown at 184×184."));

    let animated = d.animated_avatars.iter().find(|i| i.id == d.equipped[3]).map(|i| i.image.clone());
    let frame = d.frames.iter().find(|i| i.id == d.equipped[2]).map(|i| i.image.clone());
    let shown = animated.unwrap_or_else(|| still.clone());

    // Preview + Upload
    let preview_r = Rect::from_min_size(pos2(left, y), vec2(128.0 * s, 128.0 * s));
    if preview_r.intersects(clip) {
        paint_avatar_with_frame(ui, &f.t, preview_r, &shown, frame.as_deref(), s);
    }
    let (up_r, up_hit) = button(bp, ui, f, Id::unique("bp-pedit-upload"), pos2(preview_r.right() + 24.0 * s, y + 6.0 * s), 0.0, Some(icons::IMAGE), t!("Upload your avatar"), Kind::Primary, None);
    if up_hit.activated && !saving {
        if let Some(path) = rfd::FileDialog::new().add_filter(t!("Images").to_string(), &["jpg", "jpeg", "png"]).pick_file() {
            bp.profile.edit.pending += 1;
            f.app.client.send(Command::UploadAvatar { path });
        }
    }
    let note_r = paint::wrapped(
        ui.painter(),
        pos2(preview_r.right() + 24.0 * s, up_r.bottom() + 10.0 * s),
        t!("JPEG or PNG. Steam crops it to a square. An upload is saved right away."),
        w::font(15.0 * s),
        f.t.text_faint,
        width - 160.0 * s,
        2,
    );
    y = preview_r.bottom().max(note_r.bottom()) + 24.0 * s;

    // Animated avatars
    let mut see_anim = bp.profile.edit.see_all[0];
    y = edit_item_section(bp, ui, f, left, y, width, clip, "anim-av", t!("Animated avatar"), &d.animated_avatars, &mut d.equipped[3], vec2(104.0 * s, 104.0 * s), &mut see_anim, |ui, r, it| {
        match it {
            Some(item) => {
                if !w::paint_contain(ui, r.shrink(6.0 * s), &item.image, Align2::CENTER_CENTER) {
                    paint::skeleton(ui, &f.t, r.shrink(6.0 * s), paint::radius(8.0 * s));
                }
            }
            None => {
                if !w::paint_cover(ui, r.shrink(6.0 * s), &still, paint::radius(8.0 * s)) {
                    ui.painter().rect_filled(r.shrink(6.0 * s), paint::radius(8.0 * s), f.t.raised);
                }
            }
        }
    });
    bp.profile.edit.see_all[0] = see_anim;

    // Avatar frames
    let mut see_frames = bp.profile.edit.see_all[1];
    y = edit_item_section(bp, ui, f, left, y, width, clip, "av-frame", t!("Avatar frame"), &d.frames, &mut d.equipped[2], vec2(104.0 * s, 104.0 * s), &mut see_frames, |ui, r, it| {
        paint_avatar_with_frame(ui, &f.t, r.shrink(6.0 * s), &shown, it.map(|i| i.image.as_str()), s);
    });
    bp.profile.edit.see_all[1] = see_frames;

    let orig = original_edit(&f.app.state);
    let o = orig.as_ref().map(|o| o.equipped).unwrap_or_default();
    let changed = o[2] != d.equipped[2] || o[3] != d.equipped[3];
    let (save, cancel) = edit_save_bar(bp, ui, f, left, &mut y, changed, saving, true, "avatar");
    if cancel {
        d.equipped[2] = o[2];
        d.equipped[3] = o[3];
    }
    if save {
        let now: Vec<Command> = [3usize, 2]
            .into_iter()
            .filter(|k| o[*k] != d.equipped[*k])
            .map(|k| Command::EquipProfileItem { kind: k as u8, id: d.equipped[k] })
            .collect();
        bp.profile.send_saves(&f.app.client, now, vec![]);
    }
    bp.profile.edit.draft = Some(d);
    y
}

fn edit_background(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, clip: Rect) -> f32 {
    let s = f.s;
    let saving = bp.profile.edit.saving();
    let mut d = match bp.profile.edit.draft.clone() {
        Some(d) => d,
        None => return y,
    };

    edit_section_header(ui, f, left, &mut y, width, t!("Profile background"), t!("Backgrounds you own, shown behind your profile."));

    let chosen = d.backgrounds.iter().find(|i| i.id == d.equipped[0]);
    let prev_r = Rect::from_min_size(pos2(left, y), vec2(width, (width * 0.30).clamp(160.0 * s, 250.0 * s)));
    if prev_r.intersects(clip) {
        paint_profile_preview(ui, &f.t, prev_r, chosen, d.background_full, themes::get(&d.theme), &f.app.state, &d, s);
    }
    y += prev_r.height() + 14.0 * s;

    if chosen.is_some() {
        let delta = edit_choice_row(
            bp,
            ui,
            f,
            left,
            &mut y,
            width,
            clip,
            Id::unique("bp-pedit-bg-full"),
            t!("Profile background"),
            "",
            if d.background_full { t!("Full screen") } else { t!("Original size") },
        );
        if delta != 0 {
            d.background_full = !d.background_full;
        }
    }

    let mut filter = std::mem::take(&mut bp.profile.edit.filter);
    edit_text_row(bp, ui, f, left, &mut y, width, clip, EditField::Filter, t!("Filter"), "", &mut filter);
    let q = filter.trim().to_lowercase();
    let filtered: Vec<ProfileItemChoice> = d
        .backgrounds
        .iter()
        .filter(|i| q.is_empty() || i.name.to_lowercase().contains(&q) || i.app_name.to_lowercase().contains(&q))
        .cloned()
        .collect();
    bp.profile.edit.filter = filter;

    let mut all = true;
    y = edit_item_section(bp, ui, f, left, y, width, clip, "bg", "", &filtered, &mut d.equipped[0], vec2(210.0 * s, 118.0 * s), &mut all, |ui, r, it| {
        match it {
            Some(item) => {
                let src = if item.large.is_empty() { &item.image } else { &item.large };
                if !w::paint_cover(ui, r, src, paint::radius(8.0 * s)) {
                    paint::skeleton(ui, &f.t, r, paint::radius(8.0 * s));
                }
            }
            None => {
                ui.painter().rect_filled(r, paint::radius(8.0 * s), f.t.raised);
            }
        }
    });

    let orig = original_edit(&f.app.state);
    let (o, o_full) = orig.as_ref().map(|o| (o.equipped[0], o.background_full)).unwrap_or_default();
    let changed = o != d.equipped[0] || (d.equipped[0] != 0 && o_full != d.background_full);
    let (save, cancel) = edit_save_bar(bp, ui, f, left, &mut y, changed, saving, true, "bg");
    if cancel {
        d.equipped[0] = o;
        d.background_full = o_full;
    }
    if save {
        let (id, full) = (d.equipped[0], d.background_full);
        let flags = (id != 0 && (o != id || o_full != full)).then_some(Command::SetBackgroundFullScreen { id, full });
        if o != id {
            bp.profile.send_saves(&f.app.client, vec![Command::EquipProfileItem { kind: 0, id }], flags.into_iter().collect());
        } else {
            bp.profile.send_saves(&f.app.client, flags.into_iter().collect(), vec![]);
        }
    }
    bp.profile.edit.draft = Some(d);
    y
}

fn edit_mini(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, clip: Rect) -> f32 {
    let s = f.s;
    let saving = bp.profile.edit.saving();
    let mut d = match bp.profile.edit.draft.clone() {
        Some(d) => d,
        None => return y,
    };

    edit_section_header(ui, f, left, &mut y, width, t!("Mini profile"), t!("The background people see when they hover over your name."));

    let chosen = d.mini_backgrounds.iter().find(|i| i.id == d.equipped[1]);
    let prev_r = Rect::from_min_size(pos2(left, y), vec2(width.min(420.0 * s), 190.0 * s));
    if prev_r.intersects(clip) {
        paint_profile_preview(ui, &f.t, prev_r, chosen, true, None, &f.app.state, &d, s);
    }
    y += prev_r.height() + 16.0 * s;

    let mut filter = std::mem::take(&mut bp.profile.edit.filter);
    edit_text_row(bp, ui, f, left, &mut y, width, clip, EditField::Filter, t!("Filter"), "", &mut filter);
    let q = filter.trim().to_lowercase();
    let filtered: Vec<ProfileItemChoice> = d
        .mini_backgrounds
        .iter()
        .filter(|i| q.is_empty() || i.name.to_lowercase().contains(&q) || i.app_name.to_lowercase().contains(&q))
        .cloned()
        .collect();
    bp.profile.edit.filter = filter;

    let mut all = true;
    y = edit_item_section(bp, ui, f, left, y, width, clip, "mini", "", &filtered, &mut d.equipped[1], vec2(210.0 * s, 118.0 * s), &mut all, |ui, r, it| {
        match it {
            Some(item) => {
                let src = if item.large.is_empty() { &item.image } else { &item.large };
                if !w::paint_cover(ui, r, src, paint::radius(8.0 * s)) {
                    paint::skeleton(ui, &f.t, r, paint::radius(8.0 * s));
                }
            }
            None => {
                ui.painter().rect_filled(r, paint::radius(8.0 * s), f.t.raised);
            }
        }
    });

    let o = original_edit(&f.app.state).map(|o| o.equipped[1]).unwrap_or_default();
    let (save, cancel) = edit_save_bar(bp, ui, f, left, &mut y, o != d.equipped[1], saving, true, "mini");
    if cancel {
        d.equipped[1] = o;
    }
    if save {
        bp.profile.send_saves(&f.app.client, vec![Command::EquipProfileItem { kind: 1, id: d.equipped[1] }], vec![]);
    }
    bp.profile.edit.draft = Some(d);
    y
}

fn edit_theme(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, clip: Rect) -> f32 {
    let s = f.s;
    let saving = bp.profile.edit.saving();
    let mut d = match bp.profile.edit.draft.clone() {
        Some(d) => d,
        None => return y,
    };

    edit_section_header(ui, f, left, &mut y, width, t!("Theme"), t!("The colors of your profile page on Steam."));

    let bg = d.backgrounds.iter().find(|i| i.id == d.equipped[0]);
    let prev_r = Rect::from_min_size(pos2(left, y), vec2(width, (width * 0.30).clamp(160.0 * s, 250.0 * s)));
    if prev_r.intersects(clip) {
        paint_profile_preview(ui, &f.t, prev_r, bg, d.background_full, themes::get(&d.theme), &f.app.state, &d, s);
    }
    y += prev_r.height() + 18.0 * s;

    if d.themes.is_empty() {
        paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, t!("No themes available."), w::font(18.0 * s), f.t.text_faint, width);
        y += 38.0 * s;
    } else {
        let mut list: Vec<(String, String)> = vec![(String::new(), String::new())];
        for (id, title) in &d.themes {
            if !id.is_empty() && !id.eq_ignore_ascii_case("Default") {
                list.push((id.clone(), title.clone()));
            }
        }
        let cols = if width >= 720.0 * s { 4 } else { 3 };
        let gap = 14.0 * s;
        let cell_w = (width - gap * (cols - 1) as f32) / cols as f32;
        let cell_h = 114.0 * s;
        for row in list.chunks(cols) {
            for (c, (tid, title)) in row.iter().enumerate() {
                let r = Rect::from_min_size(pos2(left + c as f32 * (cell_w + gap), y), vec2(cell_w, cell_h));
                let on = tid == &d.theme || (tid.is_empty() && d.theme.eq_ignore_ascii_case("Default"));
                let hit = bp.focus.item(ui, Id::unique(("bp-pedit-theme", tid)), r, clip, Place::Page);
                if r.intersects(clip) {
                    let p = ui.painter();
                    p.rect_filled(r, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.82), f.t.raised, hit.glow));
                    if on {
                        p.rect_stroke(r, paint::radius(12.0 * s), Stroke::new(2.5 * s, f.t.accent), egui::StrokeKind::Inside);
                    }
                    let center = pos2(r.center().x, r.top() + 42.0 * s);
                    if let Some(st) = themes::get(tid) {
                        p.add(st.swatch_shape(center, 26.0 * s));
                    } else {
                        p.circle_filled(center, 26.0 * s, f.t.raised);
                    }
                    p.circle_stroke(center, 26.0 * s, Stroke::new(if on { 2.5 * s } else { 1.0 }, if on { f.t.accent } else { Color32::from_white_alpha(40) }));
                    paint::line(p, pos2(r.center().x, r.bottom() - 16.0 * s), Align2::CENTER_BOTTOM, &themes::name(tid, title), w::font_semibold(16.0 * s), if on { f.t.text } else { f.t.text_dim }, cell_w - 16.0 * s);
                }
                if hit.focused {
                    bp.focus.ring_around_clipped(r, 12.0 * s, clip);
                }
                if hit.activated {
                    d.theme = tid.clone();
                }
            }
            y += cell_h + gap;
        }
    }

    let o = original_edit(&f.app.state).map(|o| o.theme).unwrap_or_default();
    let changed = o != d.theme && !(o.is_empty() && d.theme.eq_ignore_ascii_case("Default")) && !(d.theme.is_empty() && o.eq_ignore_ascii_case("Default"));
    let (save, cancel) = edit_save_bar(bp, ui, f, left, &mut y, changed, saving, true, "theme");
    if cancel {
        d.theme = o;
    }
    if save {
        bp.profile.send_saves(&f.app.client, vec![Command::SetProfileTheme(d.theme.clone())], vec![]);
    }
    bp.profile.edit.draft = Some(d);
    y
}

fn edit_badge(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, clip: Rect, me: SteamId) -> f32 {
    let s = f.s;
    let saving = bp.profile.edit.saving();
    let badges = f.app.state.badges.get(&me).cloned();
    if badges.is_none() && !bp.profile.edit.badges_asked {
        bp.profile.edit.badges_asked = true;
        f.app.client.send(Command::LoadBadges { steam_id: me });
    }
    let mut d = match bp.profile.edit.draft.clone() {
        Some(d) => d,
        None => return y,
    };

    edit_section_header(
        ui,
        f,
        left,
        &mut y,
        width,
        t!("Featured badge"),
        t!("The badge shown next to your level on your profile and mini profile. Community badges can be featured here; game badges from Steam's own page."),
    );

    match &badges {
        None => {
            for _ in 0..2 {
                paint::skeleton(ui, &f.t, Rect::from_min_size(pos2(left, y), vec2(width, 90.0 * s)), paint::radius(12.0 * s));
                y += 102.0 * s;
            }
        }
        Some(Err(e)) => {
            paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, &i18n::tr_dyn(e), w::font(18.0 * s), f.t.text_dim, width);
            y += 38.0 * s;
        }
        Some(Ok(b)) => {
            let list: Vec<(u32, &model::Badge)> = b.items.iter().filter_map(|x| community_badge_id(&x.url).map(|id| (id, x))).collect();
            if list.is_empty() {
                paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, t!("No badges yet."), w::font(18.0 * s), f.t.text_faint, width);
                y += 38.0 * s;
            } else {
                let tile = 140.0 * s;
                let gap = 14.0 * s;
                let cols = (((width + gap) / (tile + gap)).floor() as usize).max(2);
                let mut opts: Vec<(u32, Option<&model::Badge>)> = vec![(0, None)];
                opts.extend(list.iter().map(|(id, b)| (*id, Some(*b))));
                for row in opts.chunks(cols) {
                    for (c, (bid, badge)) in row.iter().enumerate() {
                        let r = Rect::from_min_size(pos2(left + c as f32 * (tile + gap), y), vec2(tile, tile + 32.0 * s));
                        let on = *bid == d.favorite_badge;
                        let hit = bp.focus.item(ui, Id::unique(("bp-pedit-badge", *bid)), r, clip, Place::Page);
                        if r.intersects(clip) {
                            ui.painter().rect_filled(r, paint::radius(12.0 * s), w::mix(f.t.chrome.gamma_multiply(0.82), f.t.raised, hit.glow));
                            if on {
                                ui.painter().rect_stroke(r, paint::radius(12.0 * s), Stroke::new(2.5 * s, f.t.accent), egui::StrokeKind::Inside);
                            }
                            let ir = Rect::from_min_size(r.min + vec2(18.0 * s, 12.0 * s), vec2(tile - 36.0 * s, tile - 36.0 * s));
                            if let Some(u) = badge.and_then(|b| b.icon.as_deref()) {
                                if !w::paint_contain(ui, ir, u, Align2::CENTER_CENTER) {
                                    paint::skeleton(ui, &f.t, ir, paint::radius(8.0 * s));
                                }
                            } else {
                                ui.painter().rect_filled(ir, paint::radius(8.0 * s), f.t.raised);
                                if badge.is_some() {
                                    ui.painter().text(ir.center(), Align2::CENTER_CENTER, icons::MEDAL, w::font(30.0 * s), f.t.accent);
                                }
                            }
                            let label = badge.map(|b| b.title.as_str()).unwrap_or(t!("None"));
                            paint::line(ui.painter(), pos2(r.center().x, r.bottom() - 12.0 * s), Align2::CENTER_BOTTOM, label, w::font_semibold(15.0 * s), if on { f.t.text } else { f.t.text_dim }, tile - 16.0 * s);
                        }
                        if hit.focused {
                            bp.focus.ring_around_clipped(r, 12.0 * s, clip);
                        }
                        if hit.activated {
                            d.favorite_badge = *bid;
                        }
                    }
                    y += tile + 32.0 * s + gap;
                }
            }
        }
    }

    let o = original_edit(&f.app.state).map(|o| o.favorite_badge).unwrap_or_default();
    let (save, cancel) = edit_save_bar(bp, ui, f, left, &mut y, o != d.favorite_badge, saving, true, "badge");
    if cancel {
        d.favorite_badge = o;
    }
    if save {
        bp.profile.send_saves(&f.app.client, vec![Command::SetFavoriteBadge { badge: d.favorite_badge }], vec![]);
    }
    bp.profile.edit.draft = Some(d);
    y
}

fn edit_privacy(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame, left: f32, mut y: f32, width: f32, clip: Rect) -> f32 {
    let mut d = match bp.profile.edit.draft.clone() {
        Some(d) => d,
        None => return y,
    };
    edit_section_header(
        ui,
        f,
        left,
        &mut y,
        width,
        t!("Privacy settings"),
        t!("Choose who can see your profile and its parts. Changes are saved right away."),
    );
    let before = d.privacy;
    let p: &mut ProfilePrivacy = &mut d.privacy;

    let vis_label = |v: u8| match v {
        3 => t!("Public"),
        2 => t!("Friends only"),
        _ => t!("Private"),
    };
    let step_vis = |val: &mut u8, max: u8, delta: i32| {
        let opts: Vec<u8> = [3u8, 2, 1].into_iter().filter(|v| *v <= max).collect();
        let idx = opts.iter().position(|v| *v == *val).unwrap_or(0);
        let next = super::settings::step_index(opts.len(), idx, delta, delta > 1);
        *val = opts[next];
    };

    let d1 = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-priv-profile"), t!("My profile"), t!("Your level, badges, friends, groups and summary."), vis_label(p.profile));
    if d1 != 0 {
        step_vis(&mut p.profile, 3, d1);
    }
    let top = p.profile;
    p.games = p.games.min(top);
    p.friends = p.friends.min(top);
    p.inventory = p.inventory.min(top);

    let d2 = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-priv-games"), t!("Game details"), t!("The games you own, your playtime and achievements."), vis_label(p.games));
    if d2 != 0 {
        step_vis(&mut p.games, top, d2);
    }
    let mut hide_time = p.playtime == 1;
    if p.games > 1 {
        let dt = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-priv-playtime"), t!("Always keep my total playtime private"), "", if hide_time { t!("On") } else { t!("Off") });
        if dt != 0 {
            hide_time = !hide_time;
            p.playtime = if hide_time { 1 } else { p.games };
        }
    }
    if !hide_time {
        p.playtime = p.games;
    }

    let d3 = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-priv-friends"), t!("Friends list"), t!("Who can see your friends."), vis_label(p.friends));
    if d3 != 0 {
        step_vis(&mut p.friends, top, d3);
    }

    let d4 = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-priv-inv"), t!("Inventory"), t!("Your items, trading cards and gifts."), vis_label(p.inventory));
    if d4 != 0 {
        step_vis(&mut p.inventory, top, d4);
    }
    let mut hide_gifts = p.gifts == 1;
    if p.inventory > 1 {
        let dg = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-priv-gifts"), t!("Always keep Steam Gifts private"), "", if hide_gifts { t!("On") } else { t!("Off") });
        if dg != 0 {
            hide_gifts = !hide_gifts;
            p.gifts = if hide_gifts { 1 } else { p.inventory };
        }
    }
    if !hide_gifts {
        p.gifts = p.inventory;
    }

    let comment_opts = [(1u8, t!("Public")), (0u8, t!("Friends only")), (2u8, t!("Private"))];
    let c_idx = comment_opts.iter().position(|(v, _)| *v == p.comments).unwrap_or(0);
    let dc = edit_choice_row(bp, ui, f, left, &mut y, width, clip, Id::unique("bp-priv-comments"), t!("Can post comments on my profile"), "", comment_opts[c_idx].1);
    if dc != 0 {
        let next = super::settings::step_index(comment_opts.len(), c_idx, dc, dc > 1);
        p.comments = comment_opts[next].0;
    }

    if d.privacy != before {
        f.app.client.send(Command::SaveProfilePrivacy(d.privacy));
    }
    bp.profile.edit.draft = Some(d);
    y
}

#[allow(clippy::too_many_arguments)]
fn edit_item_section(
    bp: &mut BigPicture,
    ui: &mut Ui,
    f: &Frame,
    left: f32,
    mut y: f32,
    width: f32,
    clip: Rect,
    salt: &str,
    title: &str,
    items: &[ProfileItemChoice],
    selected: &mut u64,
    tile: egui::Vec2,
    see_all: &mut bool,
    draw_pic: impl Fn(&Ui, Rect, Option<&ProfileItemChoice>),
) -> f32 {
    let s = f.s;
    let gap = 14.0 * s;
    let cols = (((width + gap) / (tile.x + gap)).floor() as usize).max(2);
    if !title.is_empty() {
        if items.len() + 1 > cols {
            let label = if *see_all { t!("Show less") } else { t!("See all") };
            let bw = paint::pill_width(ui, false, label, s);
            let (r, hit) = button(bp, ui, f, Id::unique(("bp-pedit-more", salt)), pos2(left + width - bw, y), bw, None, label, Kind::Secondary, None);
            paint::line(ui.painter(), pos2(left, r.center().y), Align2::LEFT_CENTER, title, w::font_bold(21.0 * s), f.t.text, (width - bw - 20.0 * s).max(80.0 * s));
            if hit.activated {
                *see_all = !*see_all;
            }
            y = r.bottom() + 14.0 * s;
        } else {
            paint::line(ui.painter(), pos2(left, y), Align2::LEFT_TOP, title, w::font_bold(21.0 * s), f.t.text, width);
            y += 38.0 * s;
        }
    }
    let mut entries: Vec<Option<&ProfileItemChoice>> = vec![None];
    entries.extend(items.iter().map(Some));
    let limit = if *see_all { entries.len() } else { cols };
    let card_h = tile.y + 34.0 * s;
    for (row_i, row) in entries[..limit.min(entries.len())].chunks(cols).enumerate() {
        for (c_i, entry) in row.iter().enumerate() {
            let r = Rect::from_min_size(pos2(left + c_i as f32 * (tile.x + gap), y), vec2(tile.x, card_h));
            let id_val = entry.map_or(0, |i| i.id);
            let on = id_val == *selected;
            let hit = bp.focus.item(ui, Id::unique(("bp-pedit-item", salt, row_i * cols + c_i)), r, clip, Place::Page);
            if r.intersects(clip) {
                ui.painter().rect_filled(r, paint::radius(10.0 * s), w::mix(f.t.chrome.gamma_multiply(0.82), f.t.raised, hit.glow));
                if on {
                    ui.painter().rect_stroke(r, paint::radius(10.0 * s), Stroke::new(2.5 * s, f.t.accent), egui::StrokeKind::Inside);
                }
                let pic = Rect::from_min_size(r.min + vec2(4.0 * s, 4.0 * s), vec2(tile.x - 8.0 * s, tile.y - 8.0 * s));
                draw_pic(ui, pic, *entry);
                let label = entry.map(|i| i.name.as_str()).unwrap_or(t!("None"));
                paint::line(ui.painter(), pos2(r.center().x, r.bottom() - 10.0 * s), Align2::CENTER_BOTTOM, label, w::font_medium(14.5 * s), if on { f.t.text } else { f.t.text_dim }, tile.x - 14.0 * s);
            }
            if hit.focused {
                bp.focus.ring_around_clipped(r, 10.0 * s, clip);
            }
            if hit.activated {
                *selected = id_val;
            }
        }
        y += card_h + gap;
    }
    y + 10.0 * s
}

fn paint_avatar_with_frame(ui: &Ui, t: &Theme, r: Rect, avatar: &str, frame: Option<&str>, s: f32) {
    let inner = if frame.is_some() { r.shrink(r.width() * 0.085) } else { r };
    if !w::paint_cover(ui, inner, avatar, paint::radius(8.0 * s)) {
        paint::skeleton(ui, t, inner, paint::radius(8.0 * s));
    }
    if let Some(f) = frame {
        w::paint_cover(ui, r, f, CornerRadius::ZERO);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_profile_preview(
    ui: &Ui,
    t: &Theme,
    r: Rect,
    bg: Option<&ProfileItemChoice>,
    full: bool,
    theme: Option<themes::SteamTheme>,
    state: &client::State,
    d: &ProfileEdit,
    s: f32,
) {
    let rad = paint::radius(12.0 * s);
    ui.painter().rect_filled(r, rad, Color32::from_rgb(20, 24, 32));
    if let Some(item) = bg {
        let src = if item.large.is_empty() { &item.image } else { &item.large };
        let img = if full { r } else { r.shrink2(vec2(r.width() * 0.14, 0.0)) };
        w::paint_cover(ui, img, src, rad);
    }
    let panel = r.shrink2(vec2(r.width() * 0.16, 12.0 * s));
    if let Some(st) = theme {
        ui.painter().with_clip_rect(panel).add(st.preview_overlay(panel));
    } else {
        ui.painter().rect_filled(panel, paint::radius(8.0 * s), t.chrome.gamma_multiply(0.75));
    }
    let me = state.persona(state.steam_id);
    let av_url = d
        .animated_avatars
        .iter()
        .find(|i| i.id == d.equipped[3])
        .map(|i| i.image.clone())
        .unwrap_or_else(|| art::avatar_full(me.avatar_hash.as_deref()));
    let frame = d.frames.iter().find(|i| i.id == d.equipped[2]).map(|i| i.image.as_str());
    let av = Rect::from_min_size(panel.min + vec2(16.0 * s, 16.0 * s), vec2(68.0 * s, 68.0 * s));
    paint_avatar_with_frame(ui, t, av, &av_url, frame, s);
    let name = if d.persona_name.is_empty() { &state.persona_name } else { &d.persona_name };
    paint::line(ui.painter(), pos2(av.right() + 14.0 * s, av.top() + 12.0 * s), Align2::LEFT_TOP, name, w::font_bold(20.0 * s), Color32::WHITE, (panel.right() - av.right() - 26.0 * s).max(40.0));
    ui.painter().rect_stroke(r, rad, Stroke::new(1.0, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_profile_routes() {
        let me = 76561198000000001;
        assert_eq!(parse_route(me, None), (Page::Profile(me), None));
        assert_eq!(parse_route(me, Some("edit")), (Page::ProfileEdit, Some(EditSection::General)));
        assert_eq!(parse_route(me, Some("edit/privacy")), (Page::ProfileEdit, Some(EditSection::Privacy)));
        assert_eq!(parse_route(me, Some("edit/theme")), (Page::ProfileEdit, Some(EditSection::Theme)));
        assert_eq!(parse_route(me, Some("badges")), (Page::ProfileBadges(me), None));
        assert_eq!(parse_route(me, Some("76561198000000002/games")), (Page::ProfileGames(76561198000000002), None));
        assert_eq!(parse_route(me, Some("76561198000000002/cards/570/foil")), (Page::ProfileCards(76561198000000002, 570, true), None));
        assert_eq!(parse_route(me, Some("76561198000000002/reviews")), (Page::ProfileReviews(76561198000000002), None));
        assert_eq!(parse_route(me, Some("76561198000000002/groups")), (Page::ProfileGroups(76561198000000002), None));
    }

    #[test]
    fn strips_basic_html_markup_cleanly() {
        assert_eq!(plain_markup("Hello <b>world</b><br>Line 2 &amp; more"), "Hello world\nLine 2 & more");
    }
}

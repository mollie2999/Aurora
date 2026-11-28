use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{Align, Align2, CornerRadius, Margin, Stroke, vec2};
use i18n::t;
use model::{Achievement, AppId};
use ui::{Theme, icons, widgets as w};

const POLL_INTERVAL: Duration = Duration::from_secs(15);
const TOAST_DURATION: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct Notifications {
    session: Option<(AppId, u64)>,
    next_poll: Option<Instant>,
    seen: HashSet<u32>,
    queue: VecDeque<Achievement>,
    expires: Option<Instant>,
}

impl Notifications {
    pub(super) fn start(&mut self, app_id: AppId) {
        *self = Self::default();
        let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        self.session = Some((app_id, since));
    }

    pub(super) fn poll(&mut self, client: &client::Client, ctx: &egui::Context) {
        let Some((app_id, since)) = self.session else { return };
        let now = Instant::now();
        if self.next_poll.is_none_or(|next| now >= next) {
            self.next_poll = Some(now + POLL_INTERVAL);
            client.send(client::Command::CheckAchievementUnlocks {
                app_id,
                since,
                seen: self.seen.iter().copied().collect(),
                language: i18n::language().steam_name().into(),
            });
        }
        if let Some(next) = self.next_poll {
            ctx.request_repaint_after_for(next.saturating_duration_since(now), egui::ViewportId::ROOT);
        }
    }

    pub(super) fn enqueue(&mut self, app_id: AppId, since: u64, achievements: &[(u32, Achievement)]) {
        if self.session == Some((app_id, since)) {
            self.queue.extend(achievements.iter().filter_map(|(id, a)| self.seen.insert(*id).then(|| a.clone())));
        }
    }

    pub(super) fn enqueue_stored(&mut self, app_id: AppId, achievements: &[Achievement]) {
        if self.session.is_some_and(|(id, _)| id == app_id) {
            self.queue.extend(achievements.iter().cloned());
        }
    }

    pub(super) fn update(&mut self, visible: bool, ctx: &egui::Context) {
        let now = Instant::now();
        if self.expires.is_some_and(|until| now >= until) {
            self.queue.pop_front();
            self.expires = None;
        }
        if visible && !self.queue.is_empty() && self.expires.is_none() {
            self.expires = Some(now + TOAST_DURATION);
        }
        if let Some(until) = self.expires {
            ctx.request_repaint_after_for(until.saturating_duration_since(now), egui::ViewportId::ROOT);
        }
    }

    pub(super) fn visible(&self) -> bool {
        self.expires.is_some() && !self.queue.is_empty()
    }

    pub(super) fn show(&self, ctx: &egui::Context, theme: &Theme) {
        let Some(achievement) = self.queue.front().filter(|_| self.visible()) else { return };
        egui::Area::new(egui::Id::unique("overlay-achievement-notification"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -22.0))
            .interactable(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme.raised)
                    .stroke(Stroke::new(1.0, theme.accent))
                    .corner_radius(theme.corner_small())
                    .inner_margin(Margin::symmetric(12, 9))
                    .show(ui, |ui| {
                        ui.set_max_width(310.0);
                        ui.horizontal(|ui| {
                            let (icon, _) = ui.allocate_exact_size(vec2(42.0, 42.0), egui::Sense::hover());
                            if !w::paint_cover(ui, icon, &achievement.icon, CornerRadius::same(4)) {
                                ui.painter().text(icon.center(), Align2::CENTER_CENTER, icons::TROPHY, w::font(24.0), theme.accent);
                            }
                            ui.with_layout(egui::Layout::top_down(Align::Min), |ui| {
                                ui.label(w::text(t!("Achievement unlocked"), 12.0, theme.accent));
                                ui.label(w::text_semibold(&achievement.name, 15.0, theme.text));
                            });
                        });
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_queues_each_unlock_once_for_the_current_session() {
        let mut notices = Notifications::default();
        notices.start(440);
        let (_, since) = notices.session.unwrap();
        let unlock = [(42, Achievement::default())];
        notices.enqueue(730, since, &unlock);
        notices.enqueue(440, since - 1, &unlock);
        assert!(notices.queue.is_empty());
        notices.enqueue(440, since, &unlock);
        notices.enqueue(440, since, &unlock);
        assert_eq!(notices.queue.len(), 1);
        assert!(!notices.visible());
        let ctx = egui::Context::default();
        notices.update(true, &ctx);
        assert!(notices.visible());
        notices.expires = Some(Instant::now());
        notices.update(true, &ctx);
        assert!(!notices.visible());
    }
}

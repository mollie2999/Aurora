//! Focus for a couch: one thing is always focused, arrows/D-pad move it to
//! the nearest thing in that direction, and a glowing ring follows its bounds.

use egui::{Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};
use ui::Theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Where an item lives: in the scrolling page or in the fixed bars.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    Page,
    Bar,
}

#[derive(Clone, Copy)]
struct Item {
    id: Id,
    rect: Rect,
    place: Place,
}

#[derive(Default)]
pub struct Focus {
    pub current: Option<Id>,
    /// The focus got where it is by the mouse pointing at it (not keys, a
    /// controller or a click). Tabs only switch on a click then.
    pub by_pointer: bool,
    items: Vec<Item>,
    last: Vec<Item>,
    press: bool,
    pointer_moved: bool,
    /// The focus moved by D-pad/keys this frame (scroll it into view).
    pub moved: bool,
    pub last_dir: Option<Dir>,
    ring_target: Option<(Id, Rect, f32, Option<Rect>)>,
    /// No ring this frame (a full-page viewer: a box around the whole page
    /// only gets in the way).
    no_ring: bool,
    /// A move with nowhere to go briefly brightens the ring.
    bump: Option<f64>,
    /// Focus last frame, and when it last changed (for the tile sheen).
    seen: Option<Id>,
    pub changed_at: f64,
    /// When this focus scope was first used.
    opened: f64,
    last_pointer: Option<egui::Pos2>,
}

pub struct Hit {
    pub focused: bool,
    pub activated: bool,
    /// 0..1, eases in and out with focus.
    pub glow: f32,
    pub response: Option<egui::Response>,
}

impl Focus {
    /// Start a frame: apply a move using last frame's item positions.
    pub fn begin(&mut self, ui: &Ui, dir: Option<Dir>, press: bool) {
        self.begin_frame(ui, dir, press, |focus, dir, now| focus.step(dir, now));
    }

    /// Drawer rows are registered in reading order. Scrolling moves their
    /// rectangles, so geometry alone cannot identify the previous or next row.
    pub fn begin_drawer(&mut self, ui: &Ui, dir: Option<Dir>, press: bool, skip: impl Fn(Id) -> bool, wrap: bool) {
        self.begin_frame(ui, dir, press, |focus, dir, now| {
            let ordered = focus.last.iter().filter(|item| !skip(item.id)).map(|item| item.id);
            let current = focus.current;
            let next = match dir {
                Dir::Down => drawer_neighbor(ordered, current, wrap),
                Dir::Up => drawer_neighbor(ordered.rev(), current, wrap),
                Dir::Left | Dir::Right => return focus.step(dir, now),
            };
            focus.by_pointer = false;
            if let Some(id) = next.filter(|id| Some(*id) != current) {
                focus.current = Some(id);
                focus.moved = true;
            } else {
                focus.bump = Some(now);
            }
        });
    }

    fn begin_frame(&mut self, ui: &Ui, dir: Option<Dir>, press: bool, step: impl FnOnce(&mut Self, Dir, f64)) {
        if self.opened == 0.0 {
            self.opened = ui.input(|i| i.time).max(0.001);
        }
        self.last = std::mem::take(&mut self.items);
        self.press = press;
        self.moved = false;
        self.last_dir = dir;
        self.ring_target = None;
        self.no_ring = false;
        let now = ui.input(|i| i.time);
        // The mouse takes focus only when it really moves (not when a page
        // scrolls under it, nor right as the screen opens).
        // The first position seen is just where the cursor already was (the
        // window appeared under it): only a move after that counts.
        let pos = ui.input(|i| i.pointer.latest_pos());
        self.pointer_moved = matches!((self.last_pointer, pos), (Some(a), Some(b)) if (a - b).length() > 1.0) && now - self.opened > 0.6;
        if pos.is_some() {
            self.last_pointer = pos;
        }
        if let Some(d) = dir {
            step(self, d, now);
        }
        if self.current != self.seen {
            self.seen = self.current;
            self.changed_at = now;
        }
    }

    /// Freeze focus state while an overlay drawer owns input: clears transient
    /// input and the current frame's item buffer without discarding `current` or `last`.
    pub fn pause_frame(&mut self) {
        self.items.clear();
        self.press = false;
        self.moved = false;
        self.last_dir = None;
        self.pointer_moved = false;
        self.ring_target = None;
        self.no_ring = false;
    }

    fn step(&mut self, dir: Dir, now: f64) {
        self.by_pointer = false;
        let Some(from) = self.current.and_then(|c| self.last.iter().find(|i| i.id == c)).copied() else {
            self.current = self.last.first().map(|i| i.id);
            self.moved = true;
            return;
        };
        match nearest(&self.last, from.rect, dir) {
            Some(id) => {
                self.current = Some(id);
                self.moved = true;
            }
            None => self.bump = Some(now),
        }
    }

    /// Register a focusable item. `clip` is the visible area (mouse only
    /// reaches what's visible; the D-pad reaches everything).
    pub fn item(&mut self, ui: &Ui, id: Id, rect: Rect, clip: Rect, place: Place) -> Hit {
        self.items.push(Item { id, rect, place });
        let mut activated = false;
        let visible = rect.intersect(clip);
        let response = visible.is_positive().then(|| ui.interact(visible, id, Sense::click()));
        if let Some(r) = &response {
            if r.hovered() && self.pointer_moved && self.current != Some(id) {
                self.current = Some(id);
                self.by_pointer = true;
            }
            if r.clicked() {
                self.current = Some(id);
                self.by_pointer = false;
                activated = true;
            }
        }
        let focused = self.current == Some(id);
        if focused && self.press && visible.is_positive() {
            activated = true;
            self.press = false;
        }
        let glow = ui.ctx().animate_bool_with_time(id.with("bp-focus"), focused, 0.18);
        if focused {
            self.ring_target = Some((id, rect, 0.0, None));
        }
        Hit { focused, activated, glow, response }
    }

    /// Ring around a different shape than the item (a grown tile, a pill).
    /// Hide the ring this frame.
    pub fn hide_ring(&mut self) {
        self.no_ring = true;
    }

    pub fn ring_around(&mut self, rect: Rect, radius: f32) {
        self.ring_target = self.current.map(|id| (id, rect, radius, None));
    }

    /// Ring around a shape clipped to a specific scroll/drawer container.
    pub fn ring_around_clipped(&mut self, rect: Rect, radius: f32, clip: Rect) {
        self.ring_target = self.current.map(|id| (id, rect, radius, Some(clip)));
    }

    pub fn exists(&self, id: Id) -> bool {
        self.items.iter().any(|i| i.id == id)
    }

    /// End a frame: if the focused thing is gone, focus the first of
    /// `first` that exists, else the first page item, else `last`, else
    /// anything. Returns true if focus was newly assigned (so callers can reveal it).
    pub fn end(&mut self, first: &[Id], last: Id) -> bool {
        if self.current.is_some_and(|c| self.exists(c)) {
            return false;
        }
        let next = first
            .iter()
            .copied()
            .find(|f| self.exists(*f))
            .or_else(|| self.items.iter().find(|i| i.place == Place::Page).map(|i| i.id))
            .or_else(|| Some(last).filter(|l| self.exists(*l)))
            .or_else(|| self.items.first().map(|i| i.id));
        let changed = next != self.current;
        self.current = next;
        self.ring_target = None;
        changed && self.current.is_some()
    }

    pub fn current_place(&self) -> Option<Place> {
        let id = self.current?;
        self.items.iter().find(|item| item.id == id).map(|item| item.place)
    }

    /// The focused item's rect and place this frame.
    pub fn focused(&self) -> Option<(Rect, Place)> {
        let c = self.current?;
        self.items.iter().find(|i| i.id == c).map(|i| (i.rect, i.place))
    }

    /// Paint the ring around the current visual bounds, clipped to its region.
    /// Static while idle to save CPU/battery; animates briefly on edge bump or focus change.
    pub fn paint_ring(&mut self, ui: &Ui, t: &Theme, s: f32, radius: f32, page_clip: Rect) {
        if self.no_ring {
            return;
        }
        let Some((bounds, place)) = self.focused() else { return };
        let (_, rect, r_override, clip_override) = self
            .ring_target
            .filter(|(id, _, _, _)| Some(*id) == self.current)
            .unwrap_or((Id::NULL, bounds, 0.0, None));
        let now = ui.input(|i| i.time);
        let mut pulse = 0.25f32;
        if now - self.changed_at < 0.22 {
            let e = ((now - self.changed_at) / 0.22) as f32;
            pulse = pulse.max(0.25 + 0.55 * (e * std::f32::consts::PI).sin());
            ui.ctx().request_repaint();
        }
        if let Some(at) = self.bump {
            let elapsed = ((now - at) / 0.25) as f32;
            if elapsed >= 1.0 {
                self.bump = None;
            } else {
                pulse = pulse.max((elapsed * std::f32::consts::PI).sin());
                ui.ctx().request_repaint();
            }
        }
        let radius = if r_override > 0.0 { r_override } else { radius };
        let p = match (clip_override, place) {
            (Some(c), _) => ui.painter().with_clip_rect(c),
            (None, Place::Page) => ui.painter().with_clip_rect(page_clip),
            (None, Place::Bar) => ui.painter().clone(),
        };
        // Soft outer glow, wider and fainter outward.
        for i in 1..=5 {
            let grow = i as f32 * 3.0 * s + pulse * 2.0 * s;
            let a = (0.16 - i as f32 * 0.025).max(0.0) * (0.75 + 0.25 * pulse);
            p.rect_stroke(rect.expand(grow), CornerRadius::same((radius + grow).min(255.0) as u8), Stroke::new(3.0 * s, t.accent.gamma_multiply(a)), StrokeKind::Outside);
        }
        let ring_color = ui::widgets::mix(t.accent, Color32::WHITE, 0.25 + 0.2 * pulse);
        p.rect_stroke(rect.expand(2.0 * s), CornerRadius::same((radius + 2.0 * s).min(255.0) as u8), Stroke::new(3.5 * s, ring_color), StrokeKind::Outside);
    }
}

fn drawer_neighbor(mut items: impl Iterator<Item = Id>, current: Option<Id>, wrap: bool) -> Option<Id> {
    let first = items.next()?;
    if current == Some(first) || items.by_ref().any(|id| Some(id) == current) {
        items.next().or_else(|| wrap.then_some(first))
    } else {
        Some(first)
    }
}

/// The best item in `dir` from `from`: close along the way, and lined up
/// across it (items in the same row or column win over diagonal ones).
fn nearest(items: &[Item], from: Rect, dir: Dir) -> Option<Id> {
    let c = from.center();
    let mut best: Option<(f32, Id)> = None;
    for it in items {
        let r = it.rect;
        if r == from {
            continue;
        }
        let rc = r.center();
        // Gap along the move, and how far off-line across it.
        let (ahead, gap, off, off_center) = match dir {
            Dir::Down => (rc.y > c.y + 1.0 && r.bottom() > from.bottom() - 1.0, r.top() - from.bottom(), span_gap(from.x_range(), r.x_range()), (rc.x - c.x).abs()),
            Dir::Up => (rc.y < c.y - 1.0 && r.top() < from.top() + 1.0, from.top() - r.bottom(), span_gap(from.x_range(), r.x_range()), (rc.x - c.x).abs()),
            Dir::Right => (rc.x > c.x + 1.0 && r.right() > from.right() - 1.0, r.left() - from.right(), span_gap(from.y_range(), r.y_range()), (rc.y - c.y).abs()),
            Dir::Left => (rc.x < c.x - 1.0 && r.left() < from.left() + 1.0, from.left() - r.right(), span_gap(from.y_range(), r.y_range()), (rc.y - c.y).abs()),
        };
        // Sideways moves stay in the row: the end of a shelf stops there
        // instead of jumping to a shelf above or below.
        if !ahead || (matches!(dir, Dir::Left | Dir::Right) && off > 0.0) {
            continue;
        }
        let score = gap.max(0.0) + off * 4.0 + off_center * 0.05;
        if best.is_none_or(|(b, _)| score < b) {
            best = Some((score, it.id));
        }
    }
    best.map(|b| b.1)
}

/// Distance between two ranges, 0 when they overlap.
fn span_gap(a: egui::Rangef, b: egui::Rangef) -> f32 {
    if a.max < b.min {
        b.min - a.max
    } else if b.max < a.min {
        a.min - b.max
    } else {
        0.0
    }
}

/// Smooth scrolling toward a target that follows the focus.
#[derive(Default, Clone, Copy, Debug)]
pub struct Scroller {
    pub pos: f32,
    pub target: f32,
}

impl Scroller {
    /// Bring `start..end` (content coordinates) into a view of `view` length,
    /// keeping `margin` of room around it.
    pub fn reveal(&mut self, start: f32, end: f32, view: f32, margin: f32) {
        let margin = margin.min(((view - (end - start)) / 2.0).max(0.0));
        if start - margin < self.target {
            self.target = start - margin;
        } else if end + margin > self.target + view {
            self.target = end + margin - view;
        }
    }

    pub fn clamp(&mut self, content: f32, view: f32) {
        self.target = self.target.clamp(0.0, (content - view).max(0.0));
    }

    /// Move toward the target; true while still moving.
    pub fn step(&mut self, ui: &Ui) -> bool {
        let dt = ui.input(|i| i.stable_dt.min(0.1));
        let k = 1.0 - (-dt * 12.0).exp();
        self.pos += (self.target - self.pos) * k;
        if (self.target - self.pos).abs() < 0.5 {
            self.pos = self.target;
            false
        } else {
            ui.ctx().request_repaint();
            true
        }
    }
}

/// Ease-out cubic.
pub fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x).powi(3)
}

/// 0→1 over `secs` since `start`, eased (and keeps painting until done).
pub fn since(ui: &Ui, start: f64, secs: f32) -> f32 {
    let e = ((ui.input(|i| i.time) - start) as f32 / secs).clamp(0.0, 1.0);
    if e < 1.0 {
        ui.ctx().request_repaint();
    }
    ease(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ring_tracks_the_drawn_bounds_and_respects_the_footer() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 240.0));
        let content = Rect::from_min_max(screen.min, egui::pos2(300.0, 180.0));
        let id = Id::unique("scrolling-card");
        let mut focus = Focus { current: Some(id), ..Default::default() };
        for y in [134.0, 112.0] {
            let bounds = Rect::from_min_size(egui::pos2(50.0, y), egui::vec2(110.0, 85.0));
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                focus.begin(ui, None, false);
                focus.item(ui, id, bounds, content, Place::Page);
                focus.ring_around(bounds, 12.0);
                focus.paint_ring(ui, &Theme::default(), 1.0, 12.0, content);
            });
            output.textures_delta.clear();
            let rectangles: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => Some((shape.clip_rect, rect.rect)),
                _ => None,
            }).collect();
            assert_eq!(rectangles.len(), 6);
            for (clip, rect) in &rectangles {
                assert_eq!(*clip, content);
                assert_eq!(rect.center(), bounds.center());
            }
            assert_eq!(rectangles.last().unwrap().1, bounds.expand(2.0));
        }
    }

    #[test]
    fn ring_around_clipped_respects_custom_container_clip() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));
        let drawer = Rect::from_min_max(egui::pos2(200.0, 20.0), egui::pos2(390.0, 220.0));
        let id = Id::unique("drawer-row");
        let mut focus = Focus { current: Some(id), ..Default::default() };
        let bounds = Rect::from_min_size(egui::pos2(210.0, 40.0), egui::vec2(160.0, 50.0));
        let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            focus.begin(ui, None, false);
            focus.item(ui, id, bounds, drawer, Place::Bar);
            focus.ring_around_clipped(bounds, 10.0, drawer);
            focus.paint_ring(ui, &Theme::default(), 1.0, 10.0, screen);
        });
        output.textures_delta.clear();
        let clips: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => Some(shape.clip_rect),
            _ => None,
        }).collect();
        assert_eq!(clips.len(), 6);
        assert!(clips.iter().all(|c| *c == drawer));
    }

    #[test]
    fn offscreen_page_item_does_not_consume_press() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 200.0));
        let clip = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 100.0));
        let id = Id::unique("offscreen-item");
        let mut focus = Focus { current: Some(id), ..Default::default() };
        let offscreen = Rect::from_min_size(egui::pos2(10.0, 150.0), egui::vec2(100.0, 40.0));
        let mut activated = true;
        let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            focus.begin(ui, None, true);
            activated = focus.item(ui, id, offscreen, clip, Place::Page).activated;
        });
        output.textures_delta.clear();
        assert!(!activated);
    }

    #[test]
    fn pause_frame_clears_transient_input_and_preserves_focus() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 200.0));
        let id = Id::unique("keep-focus");
        let mut focus = Focus { current: Some(id), ..Default::default() };
        let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            focus.begin(ui, None, false);
            focus.item(ui, id, screen, screen, Place::Page);
        });
        output.textures_delta.clear();
        let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            focus.begin(ui, Some(Dir::Right), true);
            focus.item(ui, id, screen, screen, Place::Page);
            assert_eq!(focus.last_dir, Some(Dir::Right));
            focus.pause_frame();
        });
        output.textures_delta.clear();
        assert_eq!(focus.current, Some(id));
        assert_eq!(focus.last_dir, None);
        assert!(!focus.moved);
        assert!(focus.items.is_empty());
    }
}

#[cfg(test)]
#[test]
fn drawer_up_reaches_scrolled_rows_before_fixed_controls() {
    let ctx = egui::Context::default();
    let tab = Id::unique("tab");
    let other_tab = Id::unique("other-tab");
    let banner = Id::unique("banner");
    let rows = [Id::unique("first"), Id::unique("second")];
    let mut focus = Focus { current: Some(rows[1]), ..Default::default() };
    let mut frame = |dir, expected| {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            focus.begin_drawer(ui, dir, false, |id| id == other_tab, false);
            assert_eq!(focus.current, Some(expected));
            for (id, x, y, place) in [
                (tab, 0.0, 15.0, Place::Bar),
                (other_tab, 50.0, 15.0, Place::Bar),
                (banner, 0.0, 50.0, Place::Bar),
                (rows[0], 0.0, 36.0, Place::Page),
                (rows[1], 0.0, 99.0, Place::Page),
            ] {
                let rect = Rect::from_min_size(egui::pos2(x, y), egui::vec2(40.0, 20.0));
                focus.item(ui, id, rect, ui.clip_rect(), place);
            }
        });
        output.textures_delta.clear();
    };
    frame(None, rows[1]);
    frame(Some(Dir::Up), rows[0]);
    frame(Some(Dir::Up), banner);
    frame(Some(Dir::Up), tab);
    frame(Some(Dir::Down), banner);
    frame(Some(Dir::Down), rows[0]);
}

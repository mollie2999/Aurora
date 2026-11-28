//! Browser-style autoscroll: click the middle mouse button and move away
//! from where you clicked to scroll that way, faster the further you go.
//! Click again (any button), press Escape or use the wheel to stop. Holding
//! the middle button and dragging scrolls until it's let go.

use egui::{Color32, Event, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, RawInput, Stroke, TouchPhase, Vec2, vec2};

/// Distance from the anchor (points) that doesn't scroll yet.
const DEAD_ZONE: f32 = 10.0;
/// Scroll speed (points a second) per point beyond the dead zone.
const SPEED: f32 = 7.0;
const MAX_SPEED: f32 = 8000.0;

#[derive(Default)]
pub struct AutoScroll {
    anchor: Option<Pos2>,
    pointer: Option<Pos2>,
    /// The middle button is still held since autoscroll started.
    held: bool,
    /// The pointer left the dead zone while held (a drag, ended on release).
    dragged: bool,
    last_time: Option<f64>,
}

impl AutoScroll {
    pub fn stop(&mut self) {
        self.anchor = None;
        self.held = false;
        self.dragged = false;
        self.last_time = None;
    }

    /// Read (and take) the middle-button clicks, then add this frame's scroll.
    pub fn feed(&mut self, raw: &mut RawInput) {
        let mut events = std::mem::take(&mut raw.events);
        events.retain(|e| match e {
            Event::PointerMoved(p) => {
                self.pointer = Some(*p);
                true
            }
            Event::PointerGone => {
                self.stop();
                true
            }
            Event::PointerButton { pos, button: PointerButton::Middle, pressed, modifiers } => {
                if *pressed {
                    if self.anchor.is_some() || modifiers.any() {
                        self.stop();
                    } else {
                        self.anchor = Some(*pos);
                        self.pointer = Some(*pos);
                        self.held = true;
                        self.dragged = false;
                    }
                } else {
                    if self.dragged {
                        self.stop();
                    }
                    self.held = false;
                }
                // Middle clicks are autoscroll's, not the widgets'.
                false
            }
            // Like a browser: another click only ends autoscroll.
            Event::PointerButton { pressed: true, .. } if self.anchor.is_some() => {
                self.stop();
                false
            }
            Event::PointerButton { pressed: false, .. } => true,
            Event::Key { key: egui::Key::Escape, pressed: true, .. } if self.anchor.is_some() => {
                self.stop();
                false
            }
            Event::MouseWheel { .. } if self.anchor.is_some() => {
                self.stop();
                true
            }
            _ => true,
        });
        raw.events = events;

        let (Some(anchor), Some(pointer)) = (self.anchor, self.pointer) else { return };
        let now = raw.time.unwrap_or(0.0);
        let dt = self.last_time.map(|t| (now - t) as f32).unwrap_or(0.0).clamp(0.0, 0.1);
        self.last_time = Some(now);
        let offset = pointer - anchor;
        if self.held && offset.length() > DEAD_ZONE {
            self.dragged = true;
        }
        let axis = |d: f32| d.signum() * ((d.abs() - DEAD_ZONE).max(0.0) * SPEED).min(MAX_SPEED);
        let velocity = vec2(axis(offset.x), axis(offset.y));
        if velocity != Vec2::ZERO && dt > 0.0 {
            // Moving down scrolls the content down (a negative wheel delta).
            raw.events.push(Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: -velocity * dt,
                phase: TouchPhase::Move,
                source: MouseWheelSource::Trackpad,
                modifiers: Modifiers::NONE,
            });
        }
    }

    /// The anchor mark and the scroll cursor while autoscroll is on.
    pub fn paint(&self, ctx: &egui::Context, accent: Color32) {
        let Some(anchor) = self.anchor else { return };
        ctx.request_repaint();
        ctx.set_cursor_icon(egui::CursorIcon::AllScroll);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("aurora-autoscroll")));
        painter.circle(anchor, 14.0, Color32::from_black_alpha(170), Stroke::new(1.5, accent));
        painter.circle_filled(anchor, 2.5, Color32::WHITE);
        let arrow = |dir: Vec2| {
            let tip = anchor + dir * 10.0;
            let side = vec2(-dir.y, dir.x) * 3.5;
            let base = anchor + dir * 6.0;
            painter.add(egui::Shape::convex_polygon(vec![tip, base + side, base - side], Color32::WHITE, Stroke::NONE));
        };
        for dir in [vec2(0.0, -1.0), vec2(0.0, 1.0), vec2(-1.0, 0.0), vec2(1.0, 0.0)] {
            arrow(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_clears_active_autoscroll_state() {
        let mut s = AutoScroll::default();
        let mut raw = RawInput {
            time: Some(1.0),
            events: vec![
                Event::PointerButton {
                    pos: egui::pos2(100.0, 100.0),
                    button: PointerButton::Middle,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
                Event::PointerMoved(egui::pos2(100.0, 150.0)),
            ],
            ..Default::default()
        };
        s.feed(&mut raw);
        assert!(s.anchor.is_some());
        assert!(s.held);
        assert!(s.dragged);

        s.stop();
        assert!(s.anchor.is_none());
        assert!(!s.held);
        assert!(!s.dragged);
        assert!(s.last_time.is_none());
    }
}

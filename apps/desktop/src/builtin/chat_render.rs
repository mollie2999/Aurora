//! Shared chat message rendering: selectable text with clickable URLs,
//! copy / link context actions, inline Steam/YouTube media previews,
//! click-to-load external images, and an in-app image lightbox.

use crate::links;
use crate::sized_images;
use egui::{Align, Align2, Color32, CornerRadius, Key, Layout, Margin, Rect, Sense, Stroke, Ui, vec2};
use i18n::t;
use model::{ChatMedia, ChatMediaKind};
use std::collections::HashSet;
use ui::{Theme, icons, widgets as w};

const IMAGE_PREVIEW_FPS: f32 = 24.0;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChatLightbox {
    url: String,
    label: Option<String>,
}

fn lightbox_id() -> egui::Id {
    egui::Id::unique("aurora_chat_image_lightbox")
}

fn revealed_id() -> egui::Id {
    egui::Id::unique("aurora_chat_revealed_images")
}

fn short_url_label(url: &str) -> String {
    let trimmed = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let mut chars = trimmed.chars();
    let head: String = chars.by_ref().take(36).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Text placed on the clipboard when copying a chat message.
pub fn copy_text(text: &str, media: &[ChatMedia]) -> String {
    if !text.trim().is_empty() {
        text.to_string()
    } else {
        media
            .iter()
            .map(|m| m.url.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Compact preview string for toast notifications or friend sidebar summaries.
pub fn toast_preview(text: &str, media: &[ChatMedia]) -> String {
    if !text.trim().is_empty() {
        return text.to_string();
    }
    match media.first() {
        Some(m) => match m.kind {
            ChatMediaKind::Image => t!("[Image]").to_string(),
            ChatMediaKind::Video => t!("[Video]").to_string(),
            ChatMediaKind::Link => m.label.clone().unwrap_or_else(|| m.url.clone()),
        },
        None => String::new(),
    }
}

/// Collect up to 4 unique validated `http(s)://` URLs referenced by a message.
pub fn collect_urls(text: &str, media: &[ChatMedia]) -> Vec<String> {
    let mut urls = Vec::new();
    for (_, u) in model::chat::find_http_urls(text) {
        if links::validate_http_url(&u).is_some() && !urls.contains(&u) {
            urls.push(u);
            if urls.len() >= 4 {
                return urls;
            }
        }
    }
    for m in media {
        if links::validate_http_url(&m.url).is_some() && !urls.contains(&m.url) {
            urls.push(m.url.clone());
            if urls.len() >= 4 {
                return urls;
            }
        }
    }
    urls
}

fn open_lightbox(ctx: &egui::Context, url: String, label: Option<String>) {
    ctx.data_mut(|d| d.insert_temp(lightbox_id(), Some(ChatLightbox { url, label })));
}

fn media_context_menu(resp: egui::Response, url: &str) {
    let url_owned = url.to_string();
    resp.context_menu(move |ui| {
        if ui.button(format!("{}  {}", icons::LINK, t!("Copy link"))).clicked() {
            ui.ctx().copy_text(url_owned.clone());
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::EXTERNAL, t!("Open in browser"))).clicked() {
            links::open_http(&url_owned);
            ui.close();
        }
    });
}

/// Render selectable chat message text with highlighted, clickable `http(s)://` links.
pub fn render_message_text(
    ui: &mut Ui,
    t: &Theme,
    text: &str,
    _media: &[ChatMedia],
    text_color: Color32,
) -> egui::Response {
    let url_hits: Vec<(std::ops::Range<usize>, String)> = model::chat::find_http_urls(text)
        .into_iter()
        .filter(|(_, u)| links::validate_http_url(u).is_some())
        .collect();

    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width().max(1.0);
    let mut link_spans: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut cursor = 0;

    let plain_fmt = egui::text::TextFormat {
        font_id: w::font(14.0),
        color: text_color,
        ..Default::default()
    };
    let link_fmt = egui::text::TextFormat {
        font_id: w::font(14.0),
        color: t.accent,
        underline: Stroke::new(1.0, t.accent),
        ..Default::default()
    };

    for (range, url) in url_hits {
        if range.start > cursor {
            job.append(&text[cursor..range.start], 0.0, plain_fmt.clone());
        }
        let start = job.text.len();
        job.append(&text[range.clone()], 0.0, link_fmt.clone());
        let end = job.text.len();
        link_spans.push((start..end, url));
        cursor = range.end;
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, plain_fmt);
    }

    let resp = ui
        .scope(|ui| {
            ui.style_mut().interaction.selectable_labels = true;
            ui.add(egui::Label::new(job.clone()).wrap().selectable(true))
        })
        .inner;

    if !link_spans.is_empty() {
        if let Some(pos) = resp.hover_pos() {
            let mut hit_job = job;
            hit_job.wrap.max_width = ui.available_width().max(resp.rect.width()).max(1.0);
            let full_text = hit_job.text.clone();
            let galley = ui.painter().layout_job(hit_job);
            let c = galley.cursor_from_pos(pos - resp.rect.min);
            let hovered_url = full_text.char_indices().nth(c.index.0).and_then(|(byte, _)| {
                link_spans
                    .iter()
                    .find(|(range, _)| range.contains(&byte))
                    .map(|(_, u)| u.clone())
            });
            if let Some(url) = hovered_url {
                let no_drag = ui.input(|i| {
                    match (i.pointer.press_origin(), i.pointer.interact_pos()) {
                        (Some(a), Some(b)) => a.distance(b) <= 4.0,
                        _ => true,
                    }
                });
                let link_resp = resp
                    .clone()
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(&url);
                if no_drag && link_resp.clicked()
                    && let Some(url) = links::validate_http_url(&url)
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url.as_str()));
                }
                return link_resp;
            }
        }
    }
    resp
}

/// Add `Copy message`, `Copy link`, and `Open in browser` entries to a context menu.
pub fn context_menu_items(ui: &mut Ui, _t: &Theme, text: &str, media: &[ChatMedia]) {
    let copy_val = copy_text(text, media);
    if !copy_val.is_empty()
        && ui
            .button(format!("{}  {}", icons::COPY, t!("Copy message")))
            .clicked()
    {
        ui.ctx().copy_text(copy_val);
        ui.close();
    }
    let urls = collect_urls(text, media);
    let many = urls.len() > 1;
    for url in urls {
        let suffix = if many {
            format!(" ({})", short_url_label(&url))
        } else {
            String::new()
        };
        if ui
            .button(format!("{}  {}{}", icons::LINK, t!("Copy link"), suffix))
            .clicked()
        {
            ui.ctx().copy_text(url.clone());
            ui.close();
        }
        if ui
            .button(format!("{}  {}{}", icons::EXTERNAL, t!("Open in browser"), suffix))
            .clicked()
        {
            links::open_http(&url);
            ui.close();
        }
    }
}

/// Render inline media previews (trusted images, click-to-load external images,
/// YouTube video cards, and masked `[url=...]` link chips) below message text.
pub fn render_media(ui: &mut Ui, t: &Theme, text: &str, media: &[ChatMedia]) {
    if media.is_empty() {
        return;
    }
    let revealed = ui
        .ctx()
        .data(|d| d.get_temp::<HashSet<String>>(revealed_id()))
        .unwrap_or_default();
    let ppp = ui.ctx().pixels_per_point();

    for m in media {
        if links::validate_http_url(&m.url).is_none() {
            continue;
        }
        match m.kind {
            ChatMediaKind::Image => {
                ui.add_space(4.0);
                if m.trusted || revealed.contains(&m.url) {
                    let width = ui.available_width().clamp(1.0, 320.0);
                    let sized = sized_images::inline_url(&m.url, width, 240.0, ppp);
                    let r = w::picture_fit_width(ui, t, &sized, width, 240.0, IMAGE_PREVIEW_FPS);
                    let clicked = r
                        .clone()
                        .on_hover_cursor(egui::CursorIcon::ZoomIn)
                        .on_hover_text(&m.url)
                        .clicked();
                    if clicked {
                        open_lightbox(ui.ctx(), m.url.clone(), m.label.clone());
                    }
                    media_context_menu(r, &m.url);
                } else {
                    let host = model::chat::url_host(&m.url).unwrap_or("external");
                    egui::Frame::new()
                        .fill(t.surface)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(t.corner_small())
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_max_width(320.0);
                            ui.horizontal(|ui| {
                                ui.label(w::text(icons::IMAGE, 16.0, t.text_dim));
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.add(egui::Label::new(w::text_medium(host, 13.0, t.text)).truncate());
                                    ui.horizontal(|ui| {
                                        if w::button(ui, t, None, t!("Load image"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                            ui.ctx().data_mut(|d| {
                                                let mut set = d.get_temp::<HashSet<String>>(revealed_id()).unwrap_or_default();
                                                set.insert(m.url.clone());
                                                d.insert_temp(revealed_id(), set);
                                            });
                                        }
                                        if w::button(ui, t, Some(icons::EXTERNAL), t!("Open in browser"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                            links::open_http(&m.url);
                                        }
                                    });
                                });
                            });
                        });
                }
            }
            ChatMediaKind::Video => {
                if let Some(id) = model::chat::youtube_id_from_url(&m.url) {
                    if links::valid_youtube_id(&id).is_some() {
                        ui.add_space(4.0);
                        let thumb = format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg");
                        let wdt = ui.available_width().min(320.0);
                        let hgt = (wdt * 9.0 / 16.0).round();
                        let (rect, resp) = ui.allocate_exact_size(vec2(wdt, hgt), Sense::click());
                        w::image_or_skeleton(ui, t, rect, &thumb, t.corner_small());
                        let hover = resp.hovered();
                        ui.painter().circle_filled(
                            rect.center(),
                            22.0,
                            if hover { t.accent } else { Color32::from_black_alpha(180) },
                        );
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            icons::PLAY,
                            w::font(18.0),
                            Color32::WHITE,
                        );
                        if resp
                            .clone()
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(&m.url)
                            .clicked()
                        {
                            links::open_youtube(&id);
                        }
                        media_context_menu(resp, &m.url);
                    }
                }
            }
            ChatMediaKind::Link => {
                if let Some(label) = m.label.as_deref().filter(|l| !l.trim().is_empty()) {
                    if !text.contains(&m.url) {
                        ui.add_space(3.0);
                        let chip = ui
                            .add(
                                egui::Button::new(w::text(format!("{}  {label}", icons::LINK), 12.5, t.accent))
                                    .fill(t.surface)
                                    .stroke(Stroke::new(1.0, t.border))
                                    .corner_radius(t.corner_small()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(&m.url);
                        if chip.clicked() {
                            if let Some(url) = links::validate_http_url(&m.url) {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(url.as_str()));
                            }
                        }
                        media_context_menu(chip, &m.url);
                    }
                }
            }
        }
    }
}

/// Render the active chat image lightbox, if one was opened in this viewport context.
pub fn show_lightbox(ctx: &egui::Context, t: &Theme) {
    let Some(lb) = ctx
        .data(|d| d.get_temp::<Option<ChatLightbox>>(lightbox_id()))
        .flatten()
    else {
        return;
    };
    if links::validate_http_url(&lb.url).is_none() || ctx.input(|i| i.key_pressed(Key::Escape)) {
        ctx.data_mut(|d| d.insert_temp(lightbox_id(), Option::<ChatLightbox>::None));
        return;
    }

    let screen = ctx.content_rect();
    let mut close = false;
    egui::Area::new(egui::Id::unique("aurora_chat_image_lightbox_area"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let bg = ui.allocate_rect(screen, Sense::click());
            ui.painter()
                .rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(210));
            if bg.clicked() {
                close = true;
            }

            let card_w = (screen.width() - 64.0).clamp(280.0, 1100.0);
            let card_h = (screen.height() - 64.0).clamp(220.0, 860.0);
            let card_rect = Rect::from_center_size(screen.center(), vec2(card_w, card_h));
            let card_resp = ui
                .scope_builder(egui::UiBuilder::new().max_rect(card_rect), |ui| {
                    egui::Frame::new()
                        .fill(t.chrome)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(t.corner())
                        .inner_margin(Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(card_w - 28.0);
                            ui.horizontal(|ui| {
                                if let Some(label) = &lb.label {
                                    ui.add(egui::Label::new(w::text_semibold(label, 14.0, t.text)).truncate());
                                } else {
                                    ui.add(
                                        egui::Label::new(w::text_medium(short_url_label(&lb.url), 13.5, t.text_dim))
                                            .truncate(),
                                    );
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if w::button(ui, t, Some(icons::X), t!("Close"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                        close = true;
                                    }
                                    if w::button(ui, t, Some(icons::COPY), t!("Copy link"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                        ui.ctx().copy_text(lb.url.clone());
                                    }
                                    if w::button(ui, t, Some(icons::EXTERNAL), t!("Open in browser"), w::Kind::Primary, w::Size::Sm).clicked() {
                                        links::open_http(&lb.url);
                                    }
                                });
                            });
                            ui.add_space(10.0);
                            let avail = vec2(ui.available_width(), (card_h - 84.0).max(120.0));
                            let (img_rect, _) = ui.allocate_exact_size(avail, Sense::hover());
                            if !w::paint_contain_animated(ui, img_rect, &lb.url, Align2::CENTER_CENTER, IMAGE_PREVIEW_FPS) {
                                ui.put(
                                    Rect::from_center_size(img_rect.center(), vec2(32.0, 32.0)),
                                    egui::Spinner::new().size(28.0).color(t.accent),
                                );
                            }
                        });
                })
                .response;

            if card_resp.hovered() || card_resp.clicked() {
                close = false;
            }
        });

    if close {
        ctx.data_mut(|d| d.insert_temp(lightbox_id(), Option::<ChatLightbox>::None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_and_preview_fall_back_to_media() {
        let img = ChatMedia {
            kind: ChatMediaKind::Image,
            url: "https://images.steamusercontent.com/ugc/1/shot.png".into(),
            label: None,
            trusted: true,
        };
        assert_eq!(copy_text("hello", std::slice::from_ref(&img)), "hello");
        assert_eq!(
            copy_text("", std::slice::from_ref(&img)),
            "https://images.steamusercontent.com/ugc/1/shot.png"
        );
        assert_eq!(toast_preview("", &[img]), "[Image]");
    }

    #[test]
    fn collect_urls_deduplicates_and_filters_schemes() {
        let text = "check https://store.steampowered.com/app/620 and javascript:alert(1)";
        let media = vec![
            ChatMedia {
                kind: ChatMediaKind::Link,
                url: "https://store.steampowered.com/app/620".into(),
                label: Some("Portal 2".into()),
                trusted: false,
            },
            ChatMedia {
                kind: ChatMediaKind::Image,
                url: "file:///etc/passwd".into(),
                label: None,
                trusted: false,
            },
            ChatMedia {
                kind: ChatMediaKind::Video,
                url: "https://youtu.be/dQw4w9WgXcQ".into(),
                label: None,
                trusted: true,
            },
        ];
        assert_eq!(
            collect_urls(text, &media),
            vec![
                "https://store.steampowered.com/app/620".to_string(),
                "https://youtu.be/dQw4w9WgXcQ".to_string(),
            ]
        );
    }

    #[test]
    fn chat_render_draws_selectable_links_and_lightbox() {
        let ctx = egui::Context::default();
        let t = Theme::default();
        let media = vec![ChatMedia {
            kind: ChatMediaKind::Link,
            url: "https://store.steampowered.com/app/620".into(),
            label: Some("Store page".into()),
            trusted: true,
        }];
        open_lightbox(&ctx, "https://images.steamusercontent.com/ugc/1/shot.png".into(), None);
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            let r = render_message_text(ui, &t, "see https://example.com/a for info", &media, t.text);
            r.context_menu(|ui| context_menu_items(ui, &t, "see https://example.com/a for info", &media));
            render_media(ui, &t, "see https://example.com/a for info", &media);
            show_lightbox(ui.ctx(), &t);
        });
        out.textures_delta.clear();
    }
}

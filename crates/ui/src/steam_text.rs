//! Steam text as Steam shows it: BBCode or Steam's rendered HTML (profile
//! summaries, comments, custom showcases) and chat text, with Steam
//! emoticons, chat stickers and colored emoji drawn inline as images.
//!
//! [`parse`] turns the source into [`Piece`]s; [`show`] lays them out in a
//! wrapping row and returns the link the user clicked, if any.

use crate::{Theme, widgets as w};
use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontFamily, FontId, Sense, Stroke, Ui, vec2};

/// Where the text comes from, which decides what counts as markup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Profile summaries, comments, showcases: BBCode and Steam's HTML.
    Markup,
    /// Chat messages (already parsed): emoticons, stickers, emoji and links
    /// only, so a typed `[b]` stays as text.
    Chat,
}

/// How a run of text looks.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub spoiler: bool,
    pub code: bool,
    /// 1-3 for `[h1]`-`[h3]`, 0 for body text.
    pub heading: u8,
    pub quote: bool,
    /// The small `[host]` after an external link.
    pub faint: bool,
    pub color: Option<Color32>,
    pub link: Option<String>,
}

/// One piece of laid-out Steam text.
#[derive(Clone, PartialEq, Debug)]
pub enum Piece {
    Text(String, Style),
    /// A Steam emoticon by name (`:steamhappy:` -> `steamhappy`).
    Emoticon(String, Style),
    /// A chat sticker by type.
    Sticker(String),
    /// A colored emoji: its Twemoji file name, and the text to show if the
    /// image can't load.
    Emoji(String, String, Style),
    Break,
    Rule,
}

const EMOTICON: &str = "https://community.fastly.steamstatic.com/economy/emoticon/";
const EMOTICON_LARGE: &str = "https://community.fastly.steamstatic.com/economy/emoticonlarge/";
const STICKER: &str = "https://community.fastly.steamstatic.com/economy/sticker/";
const TWEMOJI: &str = "https://cdn.jsdelivr.net/gh/jdecked/twemoji@15.1.0/assets/72x72/";

pub fn emoticon_url(name: &str, large: bool) -> String {
    format!("{}{name}", if large { EMOTICON_LARGE } else { EMOTICON })
}

pub fn sticker_url(kind: &str) -> String {
    format!("{STICKER}{kind}")
}

pub fn emoji_url(file: &str) -> String {
    format!("{TWEMOJI}{file}.png")
}

// ---------- parsing ----------

#[derive(Clone, Copy, PartialEq)]
enum Span {
    Plain,
    Strike,
    Spoiler,
    Host,
    Color,
}

#[derive(Clone, Copy, PartialEq)]
enum Div {
    Plain,
    Heading,
    Code,
}

#[derive(Default)]
struct State {
    bold: u32,
    italic: u32,
    underline: u32,
    strike: u32,
    spoiler: u32,
    code: u32,
    quote: u32,
    faint: u32,
    headings: Vec<u8>,
    colors: Vec<Color32>,
    links: Vec<String>,
    spans: Vec<Span>,
    divs: Vec<Div>,
    /// The external link whose `[host]` hint goes after `[/url]`.
    open_urls: Vec<String>,
}

impl State {
    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0,
            italic: self.italic > 0,
            underline: self.underline > 0,
            strike: self.strike > 0,
            spoiler: self.spoiler > 0,
            code: self.code > 0,
            heading: self.headings.last().copied().unwrap_or(0),
            quote: self.quote > 0,
            faint: self.faint > 0,
            color: self.colors.last().copied(),
            link: self.links.last().cloned(),
        }
    }
}

struct Out {
    pieces: Vec<Piece>,
    html: bool,
    mode: Mode,
}

impl Out {
    fn brk(&mut self) {
        self.pieces.push(Piece::Break);
    }

    /// Text in the current style, with emoticons, emoji and bare links split out.
    fn text(&mut self, s: &str, st: &State) {
        if s.is_empty() {
            return;
        }
        let decoded = if self.html || self.mode == Mode::Markup { decode_entities(s) } else { s.to_string() };
        let style = st.style();
        let mut buf = String::new();
        let mut rest = decoded.as_str();
        let mut prev: Option<char> = None;
        while let Some(c) = rest.chars().next() {
            if c == '\r' {
                rest = &rest[1..];
                continue;
            }
            if c == '\n' {
                if self.html {
                    // HTML: newlines are spaces; <br> breaks lines.
                    if !buf.ends_with(' ') && !buf.is_empty() {
                        buf.push(' ');
                    }
                } else {
                    self.flush(&mut buf, &style);
                    self.brk();
                }
                prev = Some(c);
                rest = &rest[1..];
                continue;
            }
            if (c == ':' || c == EMOTICON_COLON) && !style.code {
                if let Some((len, name)) = emoticon_at(rest, prev) {
                    self.flush(&mut buf, &style);
                    self.pieces.push(Piece::Emoticon(name.to_string(), style.clone()));
                    rest = &rest[len..];
                    prev = Some(':');
                    continue;
                }
            }
            if (c == 'h') && style.link.is_none() && prev.is_none_or(|p| p.is_whitespace() || p == '(') {
                if let Some(len) = bare_url_len(rest) {
                    self.flush(&mut buf, &style);
                    let url = rest[..len].to_string();
                    let mut ls = style.clone();
                    ls.link = Some(url.clone());
                    self.pieces.push(Piece::Text(url, ls));
                    rest = &rest[len..];
                    prev = Some('/');
                    continue;
                }
            }
            if !style.code {
                if let Some((len, file)) = emoji_at(rest) {
                    self.flush(&mut buf, &style);
                    self.pieces.push(Piece::Emoji(file, rest[..len].to_string(), style.clone()));
                    rest = &rest[len..];
                    prev = Some(' ');
                    continue;
                }
            }
            buf.push(c);
            prev = Some(c);
            rest = &rest[c.len_utf8()..];
        }
        self.flush(&mut buf, &style);
    }

    fn flush(&mut self, buf: &mut String, style: &Style) {
        if buf.is_empty() {
            return;
        }
        // HTML: no spaces at the start of a line.
        if self.html && matches!(self.pieces.last(), None | Some(Piece::Break)) {
            let trimmed = buf.trim_start().to_string();
            *buf = trimmed;
            if buf.is_empty() {
                return;
            }
        }
        match self.pieces.last_mut() {
            Some(Piece::Text(prev, s)) if s == style => prev.push_str(buf),
            _ => self.pieces.push(Piece::Text(std::mem::take(buf), style.clone())),
        }
        buf.clear();
    }
}

/// Parse Steam text into pieces.
pub fn parse(src: &str, mode: Mode) -> Vec<Piece> {
    let html = mode == Mode::Markup && looks_like_html(src);
    let mut out = Out { pieces: Vec::new(), html, mode };
    let mut st = State::default();
    let mut text_start = 0usize;
    let mut i = 0usize;
    while i < src.len() {
        let b = src.as_bytes()[i];
        // Steam escapes a literal "[" as "\[" in BBCode.
        if b == b'\\' && mode == Mode::Markup && !html && src[i + 1..].starts_with('[') {
            out.text(&src[text_start..i], &st);
            out.text("[", &st);
            i += 2;
            text_start = i;
            continue;
        }
        if b != b'[' && b != b'<' {
            i += 1;
            continue;
        }
        let close = if b == b'[' { ']' } else { '>' };
        let Some(end) = src[i..].find(close).map(|e| i + e) else {
            i += 1;
            continue;
        };
        let tag = &src[i + 1..end];
        // Each handler emits the text before its tag itself.
        let handled = if b == b'[' {
            bbcode(&mut out, &mut st, tag, src, i, end, text_start, mode)
        } else if mode == Mode::Markup {
            html_tag(&mut out, &mut st, tag, &src[text_start..i])
        } else {
            None
        };
        match handled {
            Some(Consumed::Tag) => {
                i = end + 1;
                text_start = i;
            }
            Some(Consumed::Through(next)) => {
                i = next;
                text_start = i;
            }
            None => i += 1,
        }
    }
    out.text(&src[text_start..], &st);
    // Drop trailing breaks.
    while matches!(out.pieces.last(), Some(Piece::Break)) {
        out.pieces.pop();
    }
    // …and leading blank lines (Steam doesn't show them either), so a
    // summary starting with newlines doesn't open with a big gap.
    // Leading spaces on the first real line stay (profiles indent with them).
    let blank_run = out.pieces.iter().take_while(|p| matches!(p, Piece::Break) || matches!(p, Piece::Text(s, _) if is_blank(s))).count();
    let through_last_break = out.pieces[..blank_run].iter().rposition(|p| matches!(p, Piece::Break)).map_or(0, |i| i + 1);
    out.pieces.drain(..through_last_break);
    out.pieces
}

/// Text that shows nothing: spaces and invisible formatting marks (profiles
/// pad with runs of U+200E). The braille blank U+2800 is visible spacing on
/// Steam, so it isn't blank.
pub fn is_blank(s: &str) -> bool {
    s.chars().all(|c| c.is_whitespace() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}'))
}

/// How many lines the text has at most before wrapping (its breaks + 1).
pub fn line_count(pieces: &[Piece]) -> usize {
    pieces.iter().filter(|p| matches!(p, Piece::Break)).count() + 1
}

enum Consumed {
    /// The tag itself (up to its `]` / `>`).
    Tag,
    /// Everything up to this byte index (tags with literal content).
    Through(usize),
}

fn looks_like_html(s: &str) -> bool {
    ["<br", "<a ", "<img", "<span", "<div", "<b>", "<i>", "<u>", "<blockquote", "<ul", "<p>"].iter().any(|t| s.contains(t))
}

#[allow(clippy::too_many_arguments)]
fn bbcode(out: &mut Out, st: &mut State, tag: &str, src: &str, at: usize, end: usize, text_start: usize, mode: Mode) -> Option<Consumed> {
    let closing = tag.starts_with('/');
    let body = tag.trim_start_matches('/');
    let name_end = body.find(|c: char| c == '=' || c.is_whitespace()).unwrap_or(body.len());
    let name = body[..name_end].to_ascii_lowercase();
    let value = body[name_end..].trim_start().strip_prefix('=').map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
    if mode == Mode::Chat {
        // Chat keeps only stickers (and Steam's raw emoticon tags).
        return match (name.as_str(), closing) {
            ("sticker", false) => {
                let kind = attr(body, "type").filter(|kind| valid_media_name(kind))?;
                out.text(&src[text_start..at], st);
                out.pieces.push(Piece::Sticker(kind));
                let after = end + 1;
                let skip = src[after..].strip_prefix("[/sticker]").map(|_| after + "[/sticker]".len()).unwrap_or(after);
                Some(Consumed::Through(skip))
            }
            ("emoticon", false) => {
                let close = src[end + 1..].find("[/emoticon]")? + end + 1;
                let name = &src[end + 1..close];
                if !valid_media_name(name) {
                    return None;
                }
                out.text(&src[text_start..at], st);
                out.pieces.push(Piece::Emoticon(name.to_string(), st.style()));
                Some(Consumed::Through(close + "[/emoticon]".len()))
            }
            _ => None,
        };
    }
    let known = matches!(
        name.as_str(),
        "b" | "i" | "u" | "s" | "strike" | "spoiler" | "url" | "h1" | "h2" | "h3" | "quote" | "code" | "noparse" | "list" | "olist" | "*" | "hr" | "color" | "img" | "emoticon" | "sticker" | "table" | "tr" | "td" | "th" | "previewyoutube"
    );
    if !known {
        return None;
    }
    out.text(&src[text_start..at], st);
    match (name.as_str(), closing) {
        ("b", false) => st.bold += 1,
        ("b", true) => st.bold = st.bold.saturating_sub(1),
        ("i", false) => st.italic += 1,
        ("i", true) => st.italic = st.italic.saturating_sub(1),
        ("u", false) => st.underline += 1,
        ("u", true) => st.underline = st.underline.saturating_sub(1),
        ("s" | "strike", false) => st.strike += 1,
        ("s" | "strike", true) => st.strike = st.strike.saturating_sub(1),
        ("spoiler", false) => st.spoiler += 1,
        ("spoiler", true) => st.spoiler = st.spoiler.saturating_sub(1),
        ("color", false) => st.colors.push(value.as_deref().and_then(parse_color).unwrap_or(st.colors.last().copied().unwrap_or(Color32::PLACEHOLDER))),
        ("color", true) => {
            st.colors.pop();
        }
        ("h1" | "h2" | "h3", false) => {
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
            st.headings.push(name.as_bytes()[1] - b'0');
        }
        ("h1" | "h2" | "h3", true) => {
            st.headings.pop();
            out.brk();
        }
        ("quote", false) => {
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
            st.quote += 1;
            if let Some(author) = value.filter(|a| !a.is_empty()) {
                let mut s = State { quote: 1, bold: 1, ..Default::default() };
                s.colors = st.colors.clone();
                out.text(&format!("{} ", author.split(';').next().unwrap_or_default()), &s);
                out.brk();
            }
        }
        ("quote", true) => {
            st.quote = st.quote.saturating_sub(1);
            out.brk();
        }
        ("code", false) => {
            st.code += 1;
        }
        ("code", true) => st.code = st.code.saturating_sub(1),
        ("noparse", false) => {
            // Literal until [/noparse].
            let close = src[end + 1..].find("[/noparse]").map(|c| c + end + 1).unwrap_or(src.len());
            let literal = &src[end + 1..close];
            let style = st.style();
            out.flush(&mut literal.to_string(), &style);
            return Some(Consumed::Through((close + "[/noparse]".len()).min(src.len())));
        }
        ("list" | "olist", _) | ("tr", true) => {
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
        }
        ("*", false) => {
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
            out.text("  •  ", st);
        }
        ("td" | "th", true) => out.text("   ", st),
        ("hr", false) => out.pieces.push(Piece::Rule),
        ("url", false) => {
            let url = match value {
                Some(v) => v,
                None => {
                    // [url]https://…[/url]: the text is the address.
                    let close = src[end + 1..].find("[/url]").map(|c| c + end + 1)?;
                    let url = src[end + 1..close].trim().to_string();
                    st.links.push(clean_link(&url));
                    out.text(&url, st);
                    st.links.pop();
                    host_hint(out, st, &url);
                    return Some(Consumed::Through(close + "[/url]".len()));
                }
            };
            let url = clean_link(&url);
            st.open_urls.push(url.clone());
            st.links.push(url);
        }
        ("url", true) => {
            st.links.pop();
            if let Some(url) = st.open_urls.pop() {
                host_hint(out, st, &url);
            }
        }
        ("emoticon", false) => {
            let close = src[end + 1..].find("[/emoticon]")? + end + 1;
            let name = &src[end + 1..close];
            if valid_media_name(name) {
                out.pieces.push(Piece::Emoticon(name.to_string(), st.style()));
            }
            return Some(Consumed::Through(close + "[/emoticon]".len()));
        }
        ("sticker", false) => {
            if let Some(kind) = attr(body, "type").filter(|kind| valid_media_name(kind)) {
                out.pieces.push(Piece::Sticker(kind));
            }
        }
        ("img" | "previewyoutube", false) => {
            // Images and videos aren't shown inline; skip their contents.
            let close_tag = format!("[/{name}]");
            if let Some(close) = src[end + 1..].find(&close_tag) {
                return Some(Consumed::Through(close + end + 1 + close_tag.len()));
            }
        }
        _ => {}
    }
    Some(Consumed::Tag)
}

fn host_hint(out: &mut Out, st: &State, url: &str) {
    if let Some(host) = link_host(url).filter(|h| !is_steam_host(h)) {
        let mut s = State { faint: 1, ..Default::default() };
        s.colors = st.colors.clone();
        out.text(&format!(" [{host}]"), &s);
    }
}

fn html_tag(out: &mut Out, st: &mut State, tag: &str, before: &str) -> Option<Consumed> {
    let closing = tag.starts_with('/');
    let body = tag.trim_start_matches('/').trim_end_matches('/').trim();
    let name_end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
    let name = body[..name_end].to_ascii_lowercase();
    let known = matches!(name.as_str(), "b" | "strong" | "i" | "em" | "u" | "a" | "br" | "p" | "div" | "span" | "img" | "blockquote" | "ul" | "ol" | "li" | "h1" | "h2" | "h3" | "hr" | "font" | "bdi" | "s");
    if !known {
        return None;
    }
    out.text(before, st);
    let class = attr(body, "class").unwrap_or_default();
    match (name.as_str(), closing) {
        ("b" | "strong", false) => st.bold += 1,
        ("b" | "strong", true) => st.bold = st.bold.saturating_sub(1),
        ("i" | "em", false) => st.italic += 1,
        ("i" | "em", true) => st.italic = st.italic.saturating_sub(1),
        ("u", false) => st.underline += 1,
        ("u", true) => st.underline = st.underline.saturating_sub(1),
        ("s", false) => st.strike += 1,
        ("s", true) => st.strike = st.strike.saturating_sub(1),
        ("br", _) => out.brk(),
        ("p", _) => {
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
        }
        ("hr", _) => out.pieces.push(Piece::Rule),
        ("a", false) => st.links.push(attr(body, "href").map(|h| clean_link(&h)).unwrap_or_default()),
        ("a", true) => {
            st.links.pop();
        }
        ("span", false) => {
            let kind = if class.contains("bb_strike") {
                st.strike += 1;
                Span::Strike
            } else if class.contains("bb_spoiler") {
                st.spoiler += 1;
                Span::Spoiler
            } else if class.contains("bb_link_host") {
                st.faint += 1;
                Span::Host
            } else if let Some(c) = attr(body, "style").and_then(|s| s.split_once("color:").map(|x| x.1.to_string())).and_then(|c| parse_color(c.trim().trim_end_matches(';'))) {
                st.colors.push(c);
                Span::Color
            } else {
                Span::Plain
            };
            st.spans.push(kind);
        }
        ("span", true) => match st.spans.pop() {
            Some(Span::Strike) => st.strike = st.strike.saturating_sub(1),
            Some(Span::Spoiler) => st.spoiler = st.spoiler.saturating_sub(1),
            Some(Span::Host) => st.faint = st.faint.saturating_sub(1),
            Some(Span::Color) => {
                st.colors.pop();
            }
            _ => {}
        },
        ("font", false) => st.colors.push(attr(body, "color").as_deref().and_then(parse_color).unwrap_or(st.colors.last().copied().unwrap_or(Color32::PLACEHOLDER))),
        ("font", true) => {
            st.colors.pop();
        }
        ("div" | "h1" | "h2" | "h3", false) => {
            let heading = match name.as_str() {
                "h1" => 1,
                "h2" => 2,
                "h3" => 3,
                _ if class.contains("bb_h1") => 1,
                _ if class.contains("bb_h2") => 2,
                _ if class.contains("bb_h3") => 3,
                _ => 0,
            };
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
            let kind = if heading > 0 {
                st.headings.push(heading);
                Div::Heading
            } else if class.contains("bb_code") {
                st.code += 1;
                Div::Code
            } else {
                Div::Plain
            };
            st.divs.push(kind);
        }
        ("div" | "h1" | "h2" | "h3", true) => {
            match st.divs.pop() {
                Some(Div::Heading) => {
                    st.headings.pop();
                }
                Some(Div::Code) => st.code = st.code.saturating_sub(1),
                _ => {}
            }
            out.brk();
        }
        ("blockquote", false) => {
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
            st.quote += 1;
        }
        ("blockquote", true) => {
            st.quote = st.quote.saturating_sub(1);
            out.brk();
        }
        ("ul" | "ol", _) => {
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
        }
        ("li", false) => {
            if !matches!(out.pieces.last(), None | Some(Piece::Break)) {
                out.brk();
            }
            out.text("  •  ", st);
        }
        ("img", _) => {
            let src = attr(body, "src").unwrap_or_default();
            let emoticon = src.split_once("/emoticon/").map(|(_, name)| name);
            if class.contains("emoticon") || emoticon.is_some() {
                let name = emoticon.unwrap_or_else(|| src.rsplit('/').next().unwrap_or_default());
                let alt = attr(body, "alt").unwrap_or_default();
                let name = if name.is_empty() { alt.trim_matches(':') } else { name };
                if !src.contains(['?', '#']) && valid_media_name(name) {
                    out.pieces.push(Piece::Emoticon(name.to_string(), st.style()));
                }
            } else if let Some((_, name)) = src.split_once("/sticker/") {
                if valid_media_name(name) {
                    out.pieces.push(Piece::Sticker(name.to_string()));
                }
            }
        }
        _ => {}
    }
    Some(Consumed::Tag)
}

/// `name="value"` (or `name='value'`, or unquoted) inside a tag.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find(name) {
        let at = from + p;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[at + name.len()..].trim_start();
        if !before_ok || !after.starts_with('=') {
            continue;
        }
        let vstart = tag.len() - after.len() + 1;
        let v = tag[vstart..].trim_start();
        let value = match v.chars().next()? {
            q @ ('"' | '\'') => v[1..].split(q).next().unwrap_or_default(),
            _ => v.split(|c: char| c.is_whitespace()).next().unwrap_or_default(),
        };
        return Some(decode_entities(value));
    }
    None
}

fn parse_color(v: &str) -> Option<Color32> {
    let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
    if let Some(hex) = v.strip_prefix('#') {
        let hex = if hex.len() == 3 { hex.chars().flat_map(|c| [c, c]).collect::<String>() } else { hex.get(..6)?.to_string() };
        let n = u32::from_str_radix(&hex, 16).ok()?;
        return Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    Some(match v.to_ascii_lowercase().as_str() {
        "red" => Color32::from_rgb(0xff, 0x40, 0x40),
        "green" => Color32::from_rgb(0x4c, 0xc2, 0x4c),
        "blue" => Color32::from_rgb(0x4c, 0x8b, 0xf5),
        "yellow" => Color32::from_rgb(0xff, 0xdd, 0x44),
        "orange" => Color32::from_rgb(0xff, 0x98, 0x00),
        "purple" => Color32::from_rgb(0xa0, 0x5a, 0xf0),
        "white" => Color32::WHITE,
        "black" => Color32::BLACK,
        "gray" | "grey" => Color32::GRAY,
        _ => return None,
    })
}

/// Steam media names are inserted as single segments of CDN paths.
fn valid_media_name(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let mut end = tail.len().min(12);
        while !tail.is_char_boundary(end) {
            end -= 1;
        }
        let Some(semi) = tail[..end].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The real address behind Steam's link filter, entities decoded.
pub fn clean_link(href: &str) -> String {
    let href = decode_entities(href.trim());
    if href.contains("steamcommunity.com/linkfilter/") {
        if let Some(q) = href.split_once('?').map(|x| x.1) {
            for pair in q.split('&') {
                if let Some(v) = pair.strip_prefix("u=").or_else(|| pair.strip_prefix("url=")) {
                    return percent_decode(v);
                }
            }
        }
    }
    href
}

fn link_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|x| x.1)?;
    let host = rest.split(['/', '?', '#']).next()?.split('@').next_back()?.split(':').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host).to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn is_steam_host(host: &str) -> bool {
    ["steamcommunity.com", "steampowered.com", "s.team", "steamgames.com", "steamdeck.com", "steamstatic.com", "valvesoftware.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Steam writes emoticons in profile text with U+02D0 ("ːsteamhappyː"),
/// which looks like a colon but isn't one.
const EMOTICON_COLON: char = 'ː';

/// `:name:` or `ːnameː` at the start of `s` (an emoticon): its byte length and name.
fn emoticon_at(s: &str, prev: Option<char>) -> Option<(usize, &str)> {
    if prev.is_some_and(|p| p.is_alphanumeric()) {
        return None;
    }
    let open = s.chars().next()?;
    let inner = &s[open.len_utf8()..];
    let end = inner.find(open)?;
    let name = &inner[..end];
    let ok = (2..=40).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && name.bytes().any(|b| b.is_ascii_alphabetic());
    ok.then_some((open.len_utf8() + end + open.len_utf8(), name))
}

/// A bare `http(s)://` address at the start of `s`, as its byte length.
fn bare_url_len(s: &str) -> Option<usize> {
    if !(s.starts_with("https://") || s.starts_with("http://")) {
        return None;
    }
    let mut len = s.find(|c: char| c.is_whitespace() || c == '<' || c == '"' || c == '[').unwrap_or(s.len());
    while len > 0 && matches!(s.as_bytes()[len - 1], b'.' | b',' | b'!' | b'?' | b')' | b';' | b':' | b'\'') {
        len -= 1;
    }
    let host_part = &s[s.find("://")? + 3..len];
    (!host_part.is_empty()).then_some(len)
}

/// Emoji that show as color pictures even without U+FE0F.
fn emoji_by_default(c: u32) -> bool {
    matches!(c,
        0x231A | 0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD | 0x25FE | 0x2614 | 0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1
        | 0x26AA | 0x26AB | 0x26BD | 0x26BE | 0x26C4 | 0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2 | 0x26F3 | 0x26F5 | 0x26FA | 0x26FD | 0x2705
        | 0x270A | 0x270B | 0x2728 | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B | 0x2B1C | 0x2B50
        | 0x2B55 | 0x1F004 | 0x1F0CF | 0x1F18E | 0x1F191..=0x1F19A | 0x1F201 | 0x1F21A | 0x1F22F | 0x1F232..=0x1F236 | 0x1F238..=0x1F23A
        | 0x1F250 | 0x1F251 | 0x1F300..=0x1F320 | 0x1F32D..=0x1F335 | 0x1F337..=0x1F37C | 0x1F37E..=0x1F393 | 0x1F3A0..=0x1F3CA
        | 0x1F3CF..=0x1F3D3 | 0x1F3E0..=0x1F3F0 | 0x1F3F4 | 0x1F3F8..=0x1F43E | 0x1F440 | 0x1F442..=0x1F4FC | 0x1F4FF..=0x1F53D
        | 0x1F54B..=0x1F54E | 0x1F550..=0x1F567 | 0x1F57A | 0x1F595 | 0x1F596 | 0x1F5A4 | 0x1F5FB..=0x1F64F | 0x1F680..=0x1F6C5 | 0x1F6CC
        | 0x1F6D0..=0x1F6D2 | 0x1F6D5..=0x1F6D7 | 0x1F6DC..=0x1F6DF | 0x1F6EB | 0x1F6EC | 0x1F6F4..=0x1F6FC | 0x1F7E0..=0x1F7EB | 0x1F7F0
        | 0x1F90C..=0x1F93A | 0x1F93C..=0x1F945 | 0x1F947..=0x1F9FF | 0x1FA70..=0x1FAFF)
}

/// Symbols that are emoji only when followed by U+FE0F.
fn emoji_with_selector(c: u32) -> bool {
    matches!(c, 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x21AA | 0x2328 | 0x23CF | 0x23ED..=0x23F2 | 0x23F8..=0x23FA
        | 0x24C2 | 0x25AA | 0x25AB | 0x25B6 | 0x25C0 | 0x25FB | 0x25FC | 0x2600..=0x27BF | 0x2934 | 0x2935 | 0x2B05..=0x2B07 | 0x3030 | 0x303D
        | 0x3297 | 0x3299 | 0x1F170 | 0x1F171 | 0x1F17E | 0x1F17F | 0x1F202 | 0x1F237 | 0x1F321..=0x1F32C | 0x1F336 | 0x1F37D | 0x1F396..=0x1F39F
        | 0x1F3CB..=0x1F3CE | 0x1F3D4..=0x1F3DF | 0x1F3F3..=0x1F3F7 | 0x1F43F | 0x1F441 | 0x1F4FD | 0x1F549 | 0x1F54A | 0x1F56F | 0x1F570
        | 0x1F573..=0x1F579 | 0x1F587 | 0x1F58A..=0x1F58D | 0x1F590 | 0x1F5A5 | 0x1F5A8 | 0x1F5B1 | 0x1F5B2 | 0x1F5BC | 0x1F5C2..=0x1F5C4
        | 0x1F5D1..=0x1F5D3 | 0x1F5DC..=0x1F5DE | 0x1F5E1 | 0x1F5E3 | 0x1F5E8 | 0x1F5EF | 0x1F5F3 | 0x1F5FA | 0x1F6CB | 0x1F6CD..=0x1F6CF
        | 0x1F6E0..=0x1F6E5 | 0x1F6E9 | 0x1F6F0 | 0x1F6F3)
}

/// An emoji (with its modifiers and ZWJ sequence) at the start of `s`:
/// its byte length and Twemoji file name.
pub fn emoji_at(s: &str) -> Option<(usize, String)> {
    let mut it = s.char_indices().peekable();
    let (_, first) = it.next()?;
    let cp = first as u32;
    // Flags: two regional indicators.
    if (0x1F1E6..=0x1F1FF).contains(&cp) {
        let (i2, second) = it.next()?;
        if !(0x1F1E6..=0x1F1FF).contains(&(second as u32)) {
            return None;
        }
        return Some((i2 + second.len_utf8(), format!("{:x}-{:x}", cp, second as u32)));
    }
    // Keycaps: 1️⃣ #️⃣.
    if first.is_ascii_digit() || first == '#' || first == '*' {
        let rest = &s[1..];
        let rest_cps: Vec<char> = rest.chars().take(2).collect();
        let (len, ok) = match rest_cps.as_slice() {
            ['\u{FE0F}', '\u{20E3}', ..] => (1 + 3 + 3, true),
            ['\u{20E3}', ..] => (1 + 3, true),
            _ => (0, false),
        };
        return ok.then(|| (len, format!("{:x}-20e3", cp)));
    }
    let selector_next = s[first.len_utf8()..].starts_with('\u{FE0F}');
    if !(emoji_by_default(cp) || (emoji_with_selector(cp) && selector_next)) {
        return None;
    }
    // Text presentation asked for: keep it as text.
    if s[first.len_utf8()..].starts_with('\u{FE0E}') {
        return None;
    }
    let mut cps = vec![cp];
    let mut end = first.len_utf8();
    let mut zwj = false;
    loop {
        let rest = &s[end..];
        let Some(c) = rest.chars().next() else { break };
        let u = c as u32;
        if u == 0xFE0F || (0x1F3FB..=0x1F3FF).contains(&u) || (0xE0020..=0xE007F).contains(&u) {
            cps.push(u);
            end += c.len_utf8();
            continue;
        }
        if u == 0x200D {
            let next = rest[c.len_utf8()..].chars().next();
            if let Some(n) = next.filter(|n| emoji_by_default(*n as u32) || emoji_with_selector(*n as u32)) {
                zwj = true;
                cps.push(u);
                cps.push(n as u32);
                end += c.len_utf8() + n.len_utf8();
                continue;
            }
        }
        break;
    }
    let file = cps.iter().filter(|c| zwj || **c != 0xFE0F).map(|c| format!("{c:x}")).collect::<Vec<_>>().join("-");
    Some((end, file))
}

// ---------- drawing ----------

fn font_for(st: &Style, size: f32) -> FontId {
    if st.code {
        return FontId::new(size * 0.95, FontFamily::Monospace);
    }
    // Steam's `div.bb_h1/2/3`: 20/18/16 px over its 13 px body text, at a
    // normal weight (only [b] makes them bold).
    let size = match st.heading {
        1 => size * 20.0 / 13.0,
        2 => size * 18.0 / 13.0,
        3 => size * 16.0 / 13.0,
        _ if st.faint => size * 0.82,
        _ => size,
    };
    if st.bold { w::font_semibold(size) } else { w::font(size) }
}

/// Steam's heading blue (`div.bb_h1 { color: #5aa9d6 }`).
const HEADING: Color32 = Color32::from_rgb(0x5a, 0xa9, 0xd6);

/// Lay out Steam text. Returns the address of a link the user clicked.
pub fn show(ui: &mut Ui, t: &Theme, src: &str, mode: Mode, size: f32, color: Color32) -> Option<String> {
    let pieces = parse(src, mode);
    show_pieces(ui, t, &pieces, size, color)
}

/// Lay out already-parsed pieces (see [`parse`]).
pub fn show_pieces(ui: &mut Ui, t: &Theme, pieces: &[Piece], size: f32, color: Color32) -> Option<String> {
    let mut clicked = None;
    let line_h = (size * 1.35).round();
    // A message of only emoticons/emoji shows them large, like Steam's chat.
    let only_pictures = pieces.iter().all(|p| match p {
        Piece::Text(s, _) => s.trim().is_empty(),
        Piece::Emoticon(..) | Piece::Emoji(..) => true,
        _ => false,
    }) && pieces.iter().filter(|p| matches!(p, Piece::Emoticon(..) | Piece::Emoji(..))).count() <= 3
        && !pieces.is_empty();
    let pic = if only_pictures { (size * 2.6).round() } else { line_h };
    let base_id = ui.next_auto_id();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        let mut row_empty = true;
        for (i, piece) in pieces.iter().enumerate() {
            match piece {
                Piece::Text(s, st) => {
                    let id = base_id.with(("steam-text", i));
                    let revealed = st.spoiler && ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                    let mut fg = st.color.filter(|c| *c != Color32::PLACEHOLDER).unwrap_or(if st.heading > 0 { HEADING } else { color });
                    if st.link.is_some() {
                        fg = t.link_color();
                    } else if st.faint {
                        fg = t.text_faint;
                    } else if st.quote {
                        fg = w::mix(fg, t.text_dim, 0.6);
                    }
                    // Centered in the row like the inline emoji and emoticons
                    // (the default sits text on the row's bottom, under them).
                    let mut f = TextFormat { font_id: font_for(st, size), color: fg, italics: st.italic || st.quote, valign: egui::Align::Center, ..Default::default() };
                    if st.underline {
                        f.underline = Stroke::new(1.0, fg);
                    }
                    if st.strike {
                        f.strikethrough = Stroke::new(1.0, fg);
                    }
                    if st.code {
                        f.background = t.raised;
                    }
                    if st.spoiler {
                        let bar = Color32::from_gray(12);
                        f.background = bar;
                        f.color = if revealed { Color32::from_gray(230) } else { bar };
                        f.underline = Stroke::NONE;
                        f.strikethrough = Stroke::NONE;
                    }
                    let mut job = LayoutJob::default();
                    job.append(s, 0.0, f);
                    let sense = if st.link.is_some() || st.spoiler { Sense::click() } else { Sense::hover() };
                    let resp = ui.add(egui::Label::new(job).sense(sense));
                    if st.spoiler {
                        ui.ctx().data_mut(|d| d.insert_temp(id, resp.hovered()));
                    }
                    if let Some(url) = &st.link {
                        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(url);
                        if resp.hovered() {
                            ui.painter().line_segment([resp.rect.left_bottom(), resp.rect.right_bottom()], Stroke::new(1.0, fg));
                        }
                        if resp.clicked() {
                            clicked = Some(url.clone());
                        }
                    }
                    row_empty = false;
                }
                Piece::Emoticon(name, st) => {
                    let url = emoticon_url(name, pic > 40.0);
                    if let Some(resp) = picture(ui, &url, pic, &format!(":{name}:"), size, color) {
                        let resp = resp.on_hover_text(format!(":{name}:"));
                        if let Some(link) = &st.link {
                            if resp.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                clicked = Some(link.clone());
                            }
                        }
                    }
                    row_empty = false;
                }
                Piece::Emoji(file, fallback, _) => {
                    picture(ui, &emoji_url(file), if only_pictures { pic } else { (size * 1.2).round() }, fallback, size, color);
                    row_empty = false;
                }
                Piece::Sticker(kind) => {
                    if !row_empty {
                        ui.end_row();
                    }
                    let shown = picture(ui, &sticker_url(kind), 128.0, &format!("[{kind}]"), size, color);
                    if let Some(r) = shown {
                        r.on_hover_text(kind);
                    }
                    ui.end_row();
                    row_empty = true;
                }
                Piece::Break => {
                    if row_empty {
                        ui.allocate_exact_size(vec2(0.0, line_h * 0.6), Sense::hover());
                    }
                    ui.end_row();
                    row_empty = true;
                }
                Piece::Rule => {
                    if !row_empty {
                        ui.end_row();
                    }
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
                    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, t.border));
                    ui.end_row();
                    row_empty = true;
                }
            }
        }
    });
    clicked
}

/// An inline picture, or its text while it can't be shown.
fn picture(ui: &mut Ui, url: &str, side: f32, fallback: &str, size: f32, color: Color32) -> Option<egui::Response> {
    match w::image_state(ui, url) {
        w::ImageState::Missing => {
            ui.label(w::text(fallback, size, color));
            None
        }
        _ => {
            let (r, resp) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            w::paint_contain(ui, r, url, Align2::CENTER_CENTER);
            Some(resp)
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn entity_search_preserves_utf8_at_byte_twelve() {
        for character in ["Я", "😀"] {
            let input = format!("&{}{character} &amp;", "x".repeat(10));
            assert_eq!(super::decode_entities(&input), format!("&{}{character} &", "x".repeat(10)));
        }
    }

    #[test]
    fn invalid_media_names_do_not_create_cdn_images() {
        for name in ["", "../name", "name?x", "name#x", "a%2fb", "Я", &"x".repeat(65)] {
            for mode in [Mode::Chat, Mode::Markup] {
                for markup in [format!("[emoticon]{name}[/emoticon]"), format!(r#"[sticker type="{name}"][/sticker]"#)] {
                    assert!(!parse(&markup, mode).iter().any(|p| matches!(p, Piece::Emoticon(..) | Piece::Sticker(..))), "{markup}");
                }
            }
        }
        for markup in [
            r#"<img src="https://example.com/emoticon/../name" class="emoticon">"#,
            r#"<img src="https://example.com/emoticon/name?x">"#,
            r#"<img src="https://example.com/sticker/name#x">"#,
            r#"<img class="emoticon" alt=":../name:">"#,
        ] {
            assert!(!parse(markup, Mode::Markup).iter().any(|p| matches!(p, Piece::Emoticon(..) | Piece::Sticker(..))), "{markup}");
        }
    }
    use super::*;

    fn texts(p: &[Piece]) -> String {
        p.iter()
            .map(|p| match p {
                Piece::Text(s, _) => s.clone(),
                Piece::Emoticon(n, _) => format!("<emo {n}>"),
                Piece::Sticker(n) => format!("<sticker {n}>"),
                Piece::Emoji(f, _, _) => format!("<emoji {f}>"),
                Piece::Break => "\n".into(),
                Piece::Rule => "<hr>".into(),
            })
            .collect()
    }

    #[test]
    fn bbcode_summary_with_emoticons_links_and_styles() {
        let p = parse("hi :Cursor_Z: [b]bold[/b]\n[url=https://lnk.ua/abc]OnlyFans[/url] at 10:30:45", Mode::Markup);
        assert_eq!(texts(&p), "hi <emo Cursor_Z> bold\nOnlyFans [lnk.ua] at 10:30:45");
        assert!(p.iter().any(|x| matches!(x, Piece::Text(s, st) if s == "bold" && st.bold)));
        assert!(p.iter().any(|x| matches!(x, Piece::Text(s, st) if s == "OnlyFans" && st.link.as_deref() == Some("https://lnk.ua/abc"))));
        // Steam's own links get no host hint.
        assert_eq!(texts(&parse("[url=https://store.steampowered.com/app/1]x[/url]", Mode::Markup)), "x");
    }

    #[test]
    fn steam_summaries_keep_their_lines() {
        // altushka's: 4 lines, shown whole on Steam.
        let altushka = "[url=https://steamcommunity.com/id/yayazzli/]❤️...хочу забрать тебя домой[/url]\n⠀:Cursor_Z:\n⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀звезды\n [b]Это убило меня[/b]   [url=https://lnk.ua/x] OnlyFans [/url]";
        assert_eq!(line_count(&parse(altushka, Mode::Markup)), 4);
        // saya's opens with a run of U+200E marks: no blank first line.
        let saya = "\u{200e} \u{200e} \u{200e} <div class=\"bb_h1\"> happy puppygirl! <div class=\"bb_h1\">\u{200e} \u{200e}  <a class=\"bb_link\" href=\"https://steamcommunity.com/id/liathy/\" > ❤️ Ты похож на кота... </a>  </div></div>";
        let p = parse(saya, Mode::Markup);
        assert!(matches!(p.first(), Some(Piece::Text(s, st)) if s.trim() == "happy puppygirl!" && st.heading == 1));
        assert_eq!(line_count(&p), 2);
    }

    #[test]
    fn leading_blank_lines_go_but_indentation_stays() {
        let p = parse("\n \n\n   hello\nworld\n\n", Mode::Markup);
        assert!(matches!(p.first(), Some(Piece::Text(s, _)) if s == "   hello"));
        assert!(!matches!(p.last(), Some(Piece::Break)));
    }

    #[test]
    fn steam_triangular_colon_emoticons() {
        // How Steam stores emoticons in profile summaries (U+02D0).
        let p = parse("[url=https://steamcommunity.com/id/x/]❤️...hi[/url]\n⠀ːCursor_Zː\n⠀⠀звезды", Mode::Markup);
        assert_eq!(texts(&p), "<emoji 2764>...hi\n⠀<emo Cursor_Z>\n⠀⠀звезды");
        // Mixed delimiters don't count.
        assert_eq!(texts(&parse(":abcː", Mode::Markup)), ":abcː");
    }

    #[test]
    fn steam_comment_html() {
        let html = r#"so <b>good</b> <img src="https://community.akamai.steamstatic.com/economy/emoticon/steamhappy" alt=":steamhappy:" class="emoticon"><br><a class="bb_link" href="https://steamcommunity.com/linkfilter/?u=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1" target="_blank" rel="">site</a> <span class="bb_link_host">[example.com]</span> <span class="bb_spoiler"><span>secret</span></span> &lt;3"#;
        let p = parse(html, Mode::Markup);
        assert_eq!(texts(&p), "so good <emo steamhappy>\nsite [example.com] secret <3");
        assert!(p.iter().any(|x| matches!(x, Piece::Text(s, st) if s == "site" && st.link.as_deref() == Some("https://example.com/a?b=1"))));
        assert!(p.iter().any(|x| matches!(x, Piece::Text(s, st) if s == "secret" && st.spoiler)));
        assert!(p.iter().any(|x| matches!(x, Piece::Text(s, st) if s == "[example.com]" && st.faint)));
    }

    #[test]
    fn chat_keeps_brackets_but_draws_stickers_and_emoticons() {
        let p = parse("[b]not bold[/b] :steamhappy: [sticker type=\"Sticker_Hearts\" limit=\"0\"][/sticker]", Mode::Chat);
        assert_eq!(texts(&p), "[b]not bold[/b] <emo steamhappy> <sticker Sticker_Hearts>");
        let p = parse("see https://example.com/x. ok", Mode::Chat);
        assert!(p.iter().any(|x| matches!(x, Piece::Text(s, st) if s == "https://example.com/x" && st.link.is_some())));
    }

    #[test]
    fn emoji_sequences_map_to_twemoji_files() {
        assert_eq!(emoji_at("😀 hi"), Some((4, "1f600".into())));
        assert_eq!(emoji_at("❤️x").map(|e| e.1), Some("2764".into()));
        assert_eq!(emoji_at("♥ plain"), None);
        assert_eq!(emoji_at("👍🏽").map(|e| e.1), Some("1f44d-1f3fd".into()));
        assert_eq!(emoji_at("👨‍👩‍👧").map(|e| e.1), Some("1f468-200d-1f469-200d-1f467".into()));
        assert_eq!(emoji_at("🇧🇷").map(|e| e.1), Some("1f1e7-1f1f7".into()));
        assert_eq!(emoji_at("1️⃣").map(|e| e.1), Some("31-20e3".into()));
        assert_eq!(emoji_at("1 apple"), None);
        assert_eq!(emoji_at("☺︎"), None);
    }

    #[test]
    fn literal_angle_and_square_brackets_stay() {
        assert_eq!(texts(&parse("yanna <3 and [x] and a:b:c", Mode::Markup)), "yanna <3 and [x] and a:b:c");
        assert_eq!(texts(&parse("[noparse][b]raw[/b][/noparse]", Mode::Markup)), "[b]raw[/b]");
    }
}

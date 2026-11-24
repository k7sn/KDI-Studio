//! Immediate-mode widgets on top of [`Canvas`].
//!
//! Pages call `ui.button(id, rect)` etc. while drawing; every interactive
//! region is recorded as a [`Hit`]. Mouse presses/releases from the window are
//! resolved against the previous frame's hits, so a click shows up as
//! `clicked == true` on the next frame (the app always repaints after input).

use crate::gfx::{Align, Canvas, TextStyle, Weight};
use crate::icons::Icon;
use poni_core::geom::{palette, Color, Point, Rect};

pub type WidgetId = u64;

/// Stable widget id from a string (FNV-1a).
pub fn wid(s: &str) -> WidgetId {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Widget id for the `n`-th item of a list.
pub fn wid_n(s: &str, n: u64) -> WidgetId {
    wid(s) ^ n.wrapping_mul(0x9E3779B97F4A7C15)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyInput {
    Char(char),
    Backspace,
    BackspaceWord,
    Delete,
    DeleteWord,
    Enter,
    Escape,
    Left,
    WordLeft,
    Right,
    WordRight,
    Up,
    Down,
    Home,
    End,
    SelectAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Arrow,
    Hand,
    Text,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub id: WidgetId,
    pub rect: Rect,
    pub cursor: CursorKind,
}

/// Input state owned by the app and shared with widgets.
#[derive(Debug, Default)]
pub struct Input {
    /// Cursor position in canvas DIPs (None when outside the island).
    pub mouse: Option<Point>,
    /// Left button is held.
    pub down: bool,
    /// Widget under the cursor when the button went down.
    pub pressed_id: Option<WidgetId>,
    /// The left button went down since the last frame (valid for one frame).
    pub just_pressed: bool,
    pub press_pos: Option<Point>,
    /// Click resolved on mouse-up (valid for exactly one frame).
    pub clicked: Option<(WidgetId, MouseButton)>,
    /// Widget that was held when the left button came up, and where it came
    /// up, even off the widget (a slider drag; valid for exactly one frame).
    pub released: Option<(WidgetId, Point)>,
    /// Mouse-wheel notches this frame (+ = away from user).
    pub wheel: f32,
    /// Keys typed while a text field has focus.
    pub keys: Vec<KeyInput>,
    /// Widget that owns keyboard focus.
    pub focused: Option<WidgetId>,
    /// Caret position (chars) inside the focused field.
    pub caret: usize,
    /// Selection anchor (char index) when selecting text with mouse or Ctrl+A.
    pub sel_anchor: Option<usize>,
}

impl Input {
    /// Called after each frame: one-shot inputs are consumed.
    pub fn end_frame(&mut self) {
        self.clicked = None;
        self.released = None;
        self.just_pressed = false;
        self.wheel = 0.0;
        self.keys.clear();
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Resp {
    pub hovered: bool,
    /// Held down on this widget.
    pub pressed: bool,
    pub clicked: bool,
    pub rclicked: bool,
    /// While pressed: distance moved since the press (DIPs).
    pub drag: Option<(f32, f32)>,
}

/// What a seekable [`Ui::progress_bar`] did this frame (fractions 0..=1).
#[derive(Clone, Copy, Debug, Default)]
pub struct Scrub {
    /// Held down: where the pointer is along the bar.
    pub held: Option<f32>,
    /// Let go: where to seek.
    pub released: Option<f32>,
    /// The press on the bar began this frame.
    pub began: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TextInputResp {
    pub focused: bool,
    pub submitted: bool,
    pub cancelled: bool,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum InputLook {
    Box,
    Row { accent: Color, icon: Option<Icon> },
}

/// The button system: two heights and three looks.
pub const PILL_SMALL: f32 = 28.0;
pub const PILL_REGULAR: f32 = 34.0;
/// Gap between neighbouring capsules.
pub const PILL_GAP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PillStyle {
    /// Secondary utility (Clear, Undo, …): neutral fill, dim label that brightens on hover.
    Utility,
    /// A page action: neutral fill, full-white label.
    Neutral,
    /// The page's one primary action: accent at 0.20 fill, accent label.
    Primary(Color),
    /// A utility capsule whose label is in an accent colour (e.g. "Undo").
    UtilityAccent(Color),
}

pub struct Ui<'a, 'g> {
    pub c: &'a mut Canvas<'g>,
    pub input: &'a mut Input,
    pub hits: &'a mut Vec<Hit>,
    /// Monotonic seconds.
    pub now: f64,
    repaint: bool,
    animating: bool,
    live: bool,
    wants_focus: Option<bool>,
}

impl<'a, 'g> Ui<'a, 'g> {
    pub fn new(c: &'a mut Canvas<'g>, input: &'a mut Input, hits: &'a mut Vec<Hit>, now: f64) -> Self {
        Self { c, input, hits, now, repaint: false, animating: false, live: false, wants_focus: None }
    }

    /// Ask for one more frame (e.g. after a state change).
    pub fn request_repaint(&mut self) {
        self.repaint = true;
    }

    /// Ask for continuous frames while something moves (e.g. a lyric scroll).
    pub fn request_anim(&mut self) {
        self.animating = true;
    }

    /// Ask for slow ambient frames (~30 fps) while something drifts steadily,
    /// e.g. a progress bar while music plays. Paused when the screen is off.
    pub fn request_live(&mut self) {
        self.live = true;
    }

    pub fn live_requested(&self) -> bool {
        self.live
    }

    pub fn repaint_requested(&self) -> bool {
        self.repaint
    }

    pub fn anim_requested(&self) -> bool {
        self.animating
    }

    /// `Some(true)` = a text field wants keyboard focus, `Some(false)` = release it.
    pub fn focus_request(&self) -> Option<bool> {
        self.wants_focus
    }

    pub fn hovered(&self, r: Rect) -> bool {
        self.input.mouse.is_some_and(|m| r.contains(m))
    }

    /// Register an interactive region and get its state.
    pub fn interact(&mut self, id: WidgetId, r: Rect, cursor: CursorKind) -> Resp {
        self.hits.push(Hit { id, rect: r, cursor });
        let hovered = self.hovered(r);
        let pressed = self.input.down && self.input.pressed_id == Some(id);
        let clicked = self.input.clicked == Some((id, MouseButton::Left));
        let rclicked = self.input.clicked == Some((id, MouseButton::Right));
        if clicked || rclicked {
            self.repaint = true;
        }
        let drag = if pressed {
            match (self.input.press_pos, self.input.mouse) {
                (Some(a), Some(b)) => Some((b.x - a.x, b.y - a.y)),
                _ => None,
            }
        } else {
            None
        };
        Resp { hovered, pressed, clicked, rclicked, drag }
    }

    /// Invisible clickable area.
    pub fn button(&mut self, id: WidgetId, r: Rect) -> Resp {
        self.interact(id, r, CursorKind::Hand)
    }

    /// Round icon button with hover/pressed background.
    pub fn icon_button(&mut self, id: WidgetId, r: Rect, icon: Icon, color: Color, icon_size: f32) -> Resp {
        let resp = self.button(id, r);
        let bg = if resp.pressed {
            palette::SURFACE_PRESSED
        } else if resp.hovered {
            palette::SURFACE_HOVER
        } else {
            Color::white(0.0)
        };
        if bg.a > 0.0 {
            self.c.fill_rrect(r, r.h.min(r.w) / 2.0, bg);
        }
        let ir = Rect::from_center(r.center(), icon_size, icon_size);
        let col = if resp.pressed { color.mul_alpha(0.8) } else { color };
        self.c.icon(icon, ir, col);
        resp
    }

    /// Filled capsule button with an optional leading icon.
    pub fn pill_button(&mut self, id: WidgetId, r: Rect, label: &str, icon: Option<Icon>, fill: Color, text: Color) -> Resp {
        let resp = self.button(id, r);
        let f = if resp.pressed {
            Color::lerp(fill, Color::white(1.0), 0.18)
        } else if resp.hovered {
            Color::lerp(fill, Color::white(1.0), 0.1)
        } else {
            fill
        };
        self.c.fill_rrect(r, r.h / 2.0, f);
        let st = TextStyle::new(12.5).weight(Weight::Semibold).color(text).center();
        match icon {
            Some(i) => {
                let (tw, _) = self.c.measure(label, &st, r.w);
                let total = 14.0 + if label.is_empty() { 0.0 } else { 5.0 + tw };
                let x = r.x + (r.w - total) / 2.0;
                self.c.icon(i, Rect::new(x, r.center().y - 7.0, 14.0, 14.0), text);
                if !label.is_empty() {
                    self.c.text(label, Rect::new(x + 19.0, r.y, tw + 2.0, r.h), &st.align(Align::Left));
                }
            }
            None => self.c.text(label, r, &st),
        }
        resp
    }

    /// Width a [`Ui::pill`] needs for `label` (and an optional icon).
    pub fn pill_width(&mut self, label: &str, icon: bool, small: bool) -> f32 {
        let (size, isz, pad) = if small { (crate::gfx::ts::FOOTNOTE, 13.0, 12.0) } else { (crate::gfx::ts::BODY, 14.0, 16.0) };
        let tw = if label.is_empty() { 0.0 } else { self.c.measure(label, &TextStyle::new(size).weight(Weight::Semibold), 400.0).0 };
        let iw = if icon { isz + if label.is_empty() { 0.0 } else { 6.0 } } else { 0.0 };
        (2.0 * pad + iw + tw).ceil()
    }

    /// A capsule button from the button system. Height decides the size:
    /// `PILL_SMALL` (28) = 12 pt label / 13 icon, `PILL_REGULAR` (34) = 13 / 14.
    pub fn pill(&mut self, id: WidgetId, r: Rect, label: &str, icon: Option<Icon>, style: PillStyle) -> Resp {
        let resp = self.button(id, r);
        let small = r.h <= 30.0;
        let (fill, text) = match style {
            PillStyle::Utility => {
                let f = if resp.pressed { Color::white(0.22) } else if resp.hovered { Color::white(0.15) } else { palette::SURFACE };
                (f, if resp.hovered || resp.pressed { palette::TEXT } else { palette::TEXT_DIM })
            }
            PillStyle::Neutral => {
                let f = if resp.pressed { Color::white(0.22) } else if resp.hovered { Color::white(0.15) } else { palette::SURFACE };
                (f, palette::TEXT)
            }
            PillStyle::Primary(c) => {
                let a = if resp.pressed { 0.36 } else if resp.hovered { 0.28 } else { 0.20 };
                (c.with_alpha(a), c)
            }
            PillStyle::UtilityAccent(c) => {
                let f = if resp.pressed { Color::white(0.22) } else if resp.hovered { Color::white(0.15) } else { palette::SURFACE };
                (f, c)
            }
        };
        self.c.fill_rrect(r, r.h / 2.0, fill);
        let (size, isz) = if small { (crate::gfx::ts::FOOTNOTE, 13.0) } else { (crate::gfx::ts::BODY, 14.0) };
        let st = TextStyle::new(size).weight(Weight::Semibold).color(text);
        let tw = if label.is_empty() { 0.0 } else { self.c.measure(label, &st, r.w).0 };
        let iw = if icon.is_some() { isz + if label.is_empty() { 0.0 } else { 6.0 } } else { 0.0 };
        let x = r.x + (r.w - iw - tw) / 2.0;
        if let Some(i) = icon {
            self.c.icon(i, Rect::new(x, r.center().y - isz / 2.0, isz, isz), text);
        }
        if !label.is_empty() {
            self.c.text(label, Rect::new(x + iw, r.y, tw + 2.0, r.h), &st);
        }
        resp
    }

    /// A widget that takes the keyboard without being a text field (the
    /// Script page's reading view): `clicked` focuses it, Esc lets go.
    /// Returns the keys pressed while it has focus.
    pub fn key_focus(&mut self, id: WidgetId, clicked: bool) -> Vec<KeyInput> {
        if clicked && self.input.focused != Some(id) {
            self.input.focused = Some(id);
            self.wants_focus = Some(true);
        }
        if self.input.focused != Some(id) {
            return Vec::new();
        }
        let keys = std::mem::take(&mut self.input.keys);
        if keys.contains(&KeyInput::Escape) {
            self.input.focused = None;
            self.wants_focus = Some(false);
        }
        if !keys.is_empty() {
            self.repaint = true;
        }
        keys
    }

    /// Mouse-wheel notches over `r` (consumed so only one widget scrolls).
    pub fn wheel_in(&mut self, r: Rect) -> f32 {
        if self.hovered(r) && self.input.wheel != 0.0 {
            let w = self.input.wheel;
            self.input.wheel = 0.0;
            w
        } else {
            0.0
        }
    }

    /// Single-line text field. Click to focus, Enter submits, Esc cancels.
    pub fn text_input(&mut self, id: WidgetId, r: Rect, buf: &mut String, placeholder: &str, max_chars: usize) -> TextInputResp {
        self.text_input_styled(id, r, buf, placeholder, max_chars, InputLook::Box)
    }

    /// A list-row style field: no box until hovered/focused, a leading icon in
    /// the same column as list checkboxes (centred at x+18, text at x+36), and
    /// an accent focus ring.
    pub fn text_input_row(&mut self, id: WidgetId, r: Rect, buf: &mut String, placeholder: &str, max_chars: usize, accent: Color, icon: Option<Icon>) -> TextInputResp {
        self.text_input_styled(id, r, buf, placeholder, max_chars, InputLook::Row { accent, icon })
    }

    fn text_input_styled(&mut self, id: WidgetId, r: Rect, buf: &mut String, placeholder: &str, max_chars: usize, look: InputLook) -> TextInputResp {
        let resp = self.interact(id, r, CursorKind::Text);
        let mut out = TextInputResp::default();
        let st = TextStyle::new(crate::gfx::ts::BODY);
        let inner = match look {
            InputLook::Box => r.inset(10.0, 0.0),
            InputLook::Row { .. } => Rect::new(r.x + 36.0, r.y, (r.w - 46.0).max(0.0), r.h),
        };

        // Posizionamento cursore e selezione trascinando col mouse
        if resp.pressed {
            let idx = self.input.mouse.map(|m| char_index_at_x(self.c, buf, &st, m.x - inner.x)).unwrap_or_else(|| buf.chars().count());
            if self.input.just_pressed || self.input.focused != Some(id) {
                self.input.focused = Some(id);
                self.input.caret = idx;
                self.input.sel_anchor = Some(idx);
                self.wants_focus = Some(true);
            } else {
                self.input.caret = idx;
            }
            self.repaint = true;
        } else if resp.clicked && self.input.focused != Some(id) {
            self.input.focused = Some(id);
            self.input.caret = buf.chars().count();
            self.input.sel_anchor = None;
            self.wants_focus = Some(true);
        }

        let focused = self.input.focused == Some(id);
        out.focused = focused;
        if focused {
            let keys = std::mem::take(&mut self.input.keys);
            let mut chars: Vec<char> = buf.chars().collect();
            let mut caret = self.input.caret.min(chars.len());
            let mut anchor = self.input.sel_anchor.map(|a| a.min(chars.len()));
            let mut nav = false;
            let delete_sel = |chars: &mut Vec<char>, caret: &mut usize, anchor: &mut Option<usize>| -> bool {
                if let Some(a) = anchor.take() {
                    if a != *caret {
                        let (lo, hi) = (a.min(*caret), a.max(*caret));
                        chars.drain(lo..hi);
                        *caret = lo;
                        return true;
                    }
                }
                false
            };
            for k in keys {
                match k {
                    KeyInput::SelectAll => {
                        if !chars.is_empty() {
                            anchor = Some(0);
                            caret = chars.len();
                            nav = true;
                        }
                    }
                    KeyInput::Char(c) if !c.is_control() => {
                        if delete_sel(&mut chars, &mut caret, &mut anchor) {
                            out.changed = true;
                        }
                        if chars.len() < max_chars {
                            chars.insert(caret, c);
                            caret += 1;
                            out.changed = true;
                        }
                    }
                    KeyInput::Backspace => {
                        if delete_sel(&mut chars, &mut caret, &mut anchor) {
                            out.changed = true;
                        } else if caret > 0 {
                            caret -= 1;
                            chars.remove(caret);
                            out.changed = true;
                        }
                    }
                    KeyInput::BackspaceWord => {
                        if delete_sel(&mut chars, &mut caret, &mut anchor) {
                            out.changed = true;
                        } else if caret > 0 {
                            let end = caret;
                            while caret > 0 && chars[caret - 1].is_whitespace() {
                                caret -= 1;
                            }
                            while caret > 0 && !chars[caret - 1].is_whitespace() {
                                caret -= 1;
                            }
                            chars.drain(caret..end);
                            out.changed = true;
                        }
                    }
                    KeyInput::Delete => {
                        if delete_sel(&mut chars, &mut caret, &mut anchor) {
                            out.changed = true;
                        } else if caret < chars.len() {
                            chars.remove(caret);
                            out.changed = true;
                        }
                    }
                    KeyInput::DeleteWord => {
                        if delete_sel(&mut chars, &mut caret, &mut anchor) {
                            out.changed = true;
                        } else if caret < chars.len() {
                            let start = caret;
                            let mut end = caret;
                            while end < chars.len() && chars[end].is_whitespace() {
                                end += 1;
                            }
                            while end < chars.len() && !chars[end].is_whitespace() {
                                end += 1;
                            }
                            chars.drain(start..end);
                            out.changed = true;
                        }
                    }
                    KeyInput::Left => {
                        if let Some(a) = anchor.take().filter(|&a| a != caret) {
                            caret = a.min(caret);
                        } else {
                            caret = caret.saturating_sub(1);
                        }
                        nav = true;
                    }
                    KeyInput::WordLeft => {
                        anchor = None;
                        while caret > 0 && chars[caret - 1].is_whitespace() {
                            caret -= 1;
                        }
                        while caret > 0 && !chars[caret - 1].is_whitespace() {
                            caret -= 1;
                        }
                        nav = true;
                    }
                    KeyInput::Right => {
                        if let Some(a) = anchor.take().filter(|&a| a != caret) {
                            caret = a.max(caret);
                        } else {
                            caret = (caret + 1).min(chars.len());
                        }
                        nav = true;
                    }
                    KeyInput::WordRight => {
                        anchor = None;
                        while caret < chars.len() && !chars[caret].is_whitespace() {
                            caret += 1;
                        }
                        while caret < chars.len() && chars[caret].is_whitespace() {
                            caret += 1;
                        }
                        nav = true;
                    }
                    KeyInput::Home => {
                        caret = 0;
                        anchor = None;
                        nav = true;
                    }
                    KeyInput::End => {
                        caret = chars.len();
                        anchor = None;
                        nav = true;
                    }
                    KeyInput::Enter => {
                        anchor = None;
                        out.submitted = true;
                    }
                    KeyInput::Escape => {
                        anchor = None;
                        out.cancelled = true;
                    }
                    _ => {}
                }
            }
            *buf = chars.into_iter().collect();
            self.input.caret = caret;
            self.input.sel_anchor = anchor;
            if out.cancelled {
                self.input.focused = None;
                self.wants_focus = Some(false);
            }
            if out.changed || out.submitted || out.cancelled || nav {
                self.repaint = true;
            }
        }
        // Draw.
        match look {
            InputLook::Box => {
                let bg = if focused { Color::white(0.13) } else if resp.hovered { Color::white(0.1) } else { palette::SURFACE };
                self.c.fill_rrect(r, 9.0, bg);
                if focused {
                    self.c.stroke_rrect(r, 9.0, Color::white(0.28), 1.0);
                }
            }
            InputLook::Row { accent, icon } => {
                if focused || resp.hovered {
                    self.c.fill_rrect(r, 9.0, if focused { Color::white(0.10) } else { palette::SURFACE });
                }
                if focused {
                    self.c.stroke_rrect(r, 9.0, accent, 1.5);
                }
                if let Some(i) = icon {
                    let col = if buf.is_empty() { palette::TEXT_FAINT } else { accent };
                    self.c.icon(i, Rect::from_center(Point::new(r.x + 18.0, r.center().y), 16.0, 16.0), col);
                }
            }
        }
        if buf.is_empty() {
            self.c.text(placeholder, inner, &st.color(palette::TEXT_FAINT));
        } else {
            if focused {
                if let Some(a) = self.input.sel_anchor.filter(|&a| a != self.input.caret) {
                    let (lo, hi) = (a.min(self.input.caret), a.max(self.input.caret));
                    let s_lo: String = buf.chars().take(lo).collect();
                    let s_hi: String = buf.chars().take(hi).collect();
                    let x0 = if s_lo.is_empty() { 0.0 } else { self.c.measure(&s_lo, &st, 10_000.0).0 };
                    let x1 = if s_hi.is_empty() { 0.0 } else { self.c.measure(&s_hi, &st, 10_000.0).0 };
                    let sel_r = Rect::new(inner.x + x0, r.center().y - 9.0, (x1 - x0).max(2.0).min((inner.w - x0).max(0.0)), 18.0);
                    self.c.fill_rrect(sel_r, 4.0, palette::BLUE.with_alpha(0.38));
                }
            }
            self.c.text(buf, inner, &st);
        }
        if focused {
            let before: String = buf.chars().take(self.input.caret).collect();
            let (w, _) = if before.is_empty() { (0.0, 0.0) } else { self.c.measure(&before, &st, 10_000.0) };
            let x = (inner.x + w + 0.5).min(inner.right());
            let cy = r.center().y;
            self.c.line(Point::new(x, cy - 8.0), Point::new(x, cy + 8.0), Color::white(0.9), 1.5);
        }
        out
    }

    /// Horizontal progress bar. When `seekable` it can be clicked or dragged:
    /// while held it shows (and reports) the fraction under the pointer, and
    /// the release reports where to seek, even if the pointer slid off the bar.
    pub fn progress_bar(&mut self, id: WidgetId, r: Rect, progress: f32, fill: Color, seekable: bool) -> Scrub {
        let hit = Rect::new(r.x, r.y - 6.0, r.w, r.h + 12.0);
        let resp = if seekable { self.button(id, hit) } else { Resp::default() };
        let frac = |x: f32| if r.w > 0.0 { ((x - r.x) / r.w).clamp(0.0, 1.0) } else { 0.0 };
        let held = if resp.pressed { self.input.mouse.map(|m| frac(m.x)) } else { None };
        let began = resp.pressed && self.input.just_pressed;
        let released = self.input.released.filter(|(rid, _)| seekable && *rid == id).map(|(_, p)| frac(p.x));
        let thick = if (resp.hovered || held.is_some()) && seekable { r.h + 2.0 } else { r.h };
        let bar = Rect::new(r.x, r.center().y - thick / 2.0, r.w, thick);
        self.c.fill_rrect(bar, thick / 2.0, palette::TRACK);
        let p = held.or(released).unwrap_or(progress).clamp(0.0, 1.0);
        if p > 0.0 {
            self.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * p).max(thick), bar.h), thick / 2.0, fill);
        }
        if released.is_some() {
            self.repaint = true;
        }
        Scrub { held, released, began }
    }

    /// Release keyboard focus (e.g. after submitting).
    /// Gives the text field `id` the keyboard, its caret after `len`
    /// characters: a field that opens on a click elsewhere (the Stocks
    /// page's search) is ready to type in.
    pub fn focus_text(&mut self, id: WidgetId, len: usize) {
        if self.input.focused != Some(id) {
            self.input.focused = Some(id);
            self.input.caret = len;
            self.wants_focus = Some(true);
        }
    }

    pub fn blur(&mut self) {
        if self.input.focused.is_some() {
            self.input.focused = None;
            self.wants_focus = Some(false);
        }
    }
}

fn char_index_at_x(c: &mut Canvas, buf: &str, st: &TextStyle, rel_x: f32) -> usize {
    if rel_x <= 0.0 || buf.is_empty() {
        return 0;
    }
    let mut best_idx = 0;
    let mut best_dist = rel_x;
    let mut prefix = String::new();
    for (i, ch) in buf.chars().enumerate() {
        prefix.push(ch);
        let w = c.measure(&prefix, st, 10_000.0).0;
        let d = (w - rel_x).abs();
        if d <= best_dist {
            best_dist = d;
            best_idx = i + 1;
        } else {
            break;
        }
    }
    best_idx
}

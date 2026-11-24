//! Pagina Gemini: query rapide con il proprio account / API Key Google AI Studio.

use crate::commands::{Command, GeminiCmd};
use crate::gfx::{ts, TextStyle};
use crate::icons::Icon;
use crate::services::gemini_chat::MODELS;
use crate::state::AppState;
use crate::ui::{wid, PillStyle, Ui, PILL_GAP, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};

const HEIGHT: f32 = 248.0;
const HEADER_H: f32 = 30.0;
const INPUT_H: f32 = 34.0;
const WHEEL_STEP: f32 = 42.0;

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let area = Rect::new(area.x, area.y, area.w, HEIGHT);
    let accent = st.accent;

    let idx = st.gemini.model_idx % MODELS.len();
    let (model_label, model_id) = MODELS[idx];

    // Riga superiore: icona + selettore modello cliccabile + pulsanti Copia / API Key
    let top = Rect::new(area.x, area.y, area.w, HEADER_H);
    ui.c.icon(Icon::Gemini, Rect::new(top.x, top.center().y - 8.0, 16.0, 16.0), accent);

    let mw = ui.pill_width(model_label, false, true);
    let model_btn = Rect::new(top.x + 22.0, top.center().y - PILL_SMALL / 2.0, mw, PILL_SMALL);
    if ui.pill(wid("gemini.model_cycle"), model_btn, model_label, None, PillStyle::UtilityAccent(accent)).clicked {
        st.gemini.model_idx = (idx + 1) % MODELS.len();
    }

    let key_label = if st.gemini.editing_key {
        "Chiudi"
    } else if st.gemini.has_key {
        "API Key"
    } else {
        "Imposta API Key"
    };
    let kw = ui.pill_width(key_label, true, true);
    let key_btn = Rect::new(top.right() - kw, top.center().y - PILL_SMALL / 2.0, kw, PILL_SMALL);
    let key_style = if st.gemini.has_key && !st.gemini.editing_key {
        PillStyle::Utility
    } else {
        PillStyle::Primary(accent)
    };
    if ui.pill(wid("gemini.key_toggle"), key_btn, key_label, Some(Icon::Sliders), key_style).clicked {
        st.gemini.editing_key = !st.gemini.editing_key;
        if st.gemini.editing_key {
            st.gemini.key_input.clear();
        }
    }

    if st.gemini.answer.is_some() && !st.gemini.editing_key {
        let just_copied = st.now < st.gemini.copied_until;
        if just_copied {
            ui.request_anim();
        }
        let (lbl, ico, style) = if just_copied {
            ("Copiato!", Icon::Check, PillStyle::Primary(palette::GREEN))
        } else {
            ("Copia", Icon::Copy, PillStyle::Utility)
        };
        let cw = ui.pill_width(lbl, true, true);
        let copy_btn = Rect::new(key_btn.x - PILL_GAP - cw, key_btn.y, cw, PILL_SMALL);
        if ui.pill(wid("gemini.copy"), copy_btn, lbl, Some(ico), style).clicked {
            if let Some(ans) = &st.gemini.answer {
                st.gemini.copied_until = st.now + 1.8;
                st.cmd(Command::Gemini(GeminiCmd::Copy(ans.clone())));
            }
        }
    }

    let mut y = top.bottom() + 8.0;

    // Pannello inserimento API Key
    if st.gemini.editing_key || !st.gemini.has_key {
        ui.c.text(
            "Incolla una o più Gemini API Key separate da virgola (aistudio.google.com/apikey):",
            Rect::new(area.x, y, area.w, 16.0),
            &TextStyle::footnote().color(palette::TEXT_DIM),
        );
        y += 20.0;

        let save_w = ui.pill_width("Salva", false, true);
        let del_w = if st.gemini.has_key { ui.pill_width("Rimuovi", false, true) + PILL_GAP } else { 0.0 };
        let field_w = (area.w - save_w - PILL_GAP - del_w).max(80.0);
        let field_r = Rect::new(area.x, y, field_w, INPUT_H);

        let mut masked: String = "\u{2022}".repeat(st.gemini.key_input.chars().count());
        let prev_len = st.gemini.key_input.chars().count();
        let resp = ui.text_input(wid("gemini.key_field"), field_r, &mut masked, "AIzaSy..., AIzaSy...", 1024);
        if resp.changed {
            let new_len = masked.chars().count();
            if new_len < prev_len {
                for _ in 0..(prev_len - new_len) {
                    st.gemini.key_input.pop();
                }
            } else {
                for ch in masked.chars().filter(|&c| c != '\u{2022}') {
                    st.gemini.key_input.push(ch);
                }
            }
        }

        let save_r = Rect::new(field_r.right() + PILL_GAP, y + (INPUT_H - PILL_SMALL) / 2.0, save_w, PILL_SMALL);
        let can_save = !st.gemini.key_input.trim().is_empty();
        if ui.pill(wid("gemini.key_save"), save_r, "Salva", None, if can_save { PillStyle::Primary(accent) } else { PillStyle::Utility }).clicked && can_save {
            let k = st.gemini.key_input.trim().to_string();
            st.gemini.key_input.clear();
            st.gemini.editing_key = false;
            st.cmd(Command::Gemini(GeminiCmd::SetKey(k)));
        }

        if st.gemini.has_key {
            let rem_r = Rect::new(save_r.right() + PILL_GAP, save_r.y, ui.pill_width("Rimuovi", false, true), PILL_SMALL);
            if ui.pill(wid("gemini.key_clear"), rem_r, "Rimuovi", None, PillStyle::Utility).clicked {
                st.gemini.key_input.clear();
                st.gemini.editing_key = false;
                st.cmd(Command::Gemini(GeminiCmd::ClearKey));
            }
        }
        return;
    }

    // Campo input query rapida + pulsante Invia
    let ask_label = if st.gemini.busy { "..." } else { "Chiedi" };
    let ask_w = ui.pill_width(ask_label, false, true).max(68.0);
    let q_w = (area.w - ask_w - PILL_GAP).max(100.0);
    let q_rect = Rect::new(area.x, y, q_w, INPUT_H);
    let inp = ui.text_input(wid("gemini.prompt"), q_rect, &mut st.gemini.prompt_input, "Fai una domanda rapida a Gemini...", 2000);
    let ask_rect = Rect::new(q_rect.right() + PILL_GAP, y + (INPUT_H - PILL_SMALL) / 2.0, ask_w, PILL_SMALL);
    let clicked = ui.pill(wid("gemini.ask"), ask_rect, ask_label, None, PillStyle::Primary(accent)).clicked;

    if (inp.submitted || clicked) && !st.gemini.busy && !st.gemini.prompt_input.trim().is_empty() {
        let q = st.gemini.prompt_input.trim().to_string();
        st.gemini.busy = true;
        st.gemini.error = None;
        st.gemini.scroll = 0.0;
        st.cmd(Command::Gemini(GeminiCmd::Ask { prompt: q, model: model_id }));
    }

    y += INPUT_H + 10.0;
    let out_box = Rect::new(area.x, y, area.w, (area.bottom() - y).max(0.0));
    ui.c.fill_rrect(out_box, 12.0, Color::white(0.05));
    let inner = out_box.inset(12.0, 10.0);

    if st.gemini.busy {
        let c = inner.center();
        ui.c.icon(Icon::Gemini, Rect::from_center(Point::new(c.x, c.y - 10.0), 18.0, 18.0), accent);
        ui.c.text(
            &format!("{model_label} sta elaborando..."),
            Rect::new(inner.x, c.y + 6.0, inner.w, 18.0),
            &TextStyle::footnote().color(palette::TEXT_DIM).center(),
        );
        return;
    }

    if let Some(err) = &st.gemini.error {
        ui.c.text(err, inner, &TextStyle::footnote().color(palette::ORANGE).wrap().top());
        return;
    }

    let Some(ans) = &st.gemini.answer else {
        ui.c.text(
            "Scrivi una domanda sopra e premi Invio. Clicca sul nome del modello in alto per cambiarlo.",
            inner,
            &TextStyle::footnote().color(palette::TEXT_FAINT).center(),
        );
        return;
    };

    // Animazione typewriter fluida (~110 caratteri/sec)
    let elapsed = (st.now - st.gemini.answer_at).max(0.0);
    let total_chars = ans.chars().count();
    let shown_chars = ((elapsed * 110.0) as usize).min(total_chars);
    let animating = shown_chars < total_chars;
    let display_text: String = if animating {
        ui.request_anim();
        let mut s: String = ans.chars().take(shown_chars).collect();
        s.push('\u{258D}');
        s
    } else {
        ans.clone()
    };

    let body_st = TextStyle::new(ts::BODY).wrap().top();
    let (_, text_h) = ui.c.measure(&display_text, &body_st, inner.w);
    let max_scroll = (text_h - inner.h).max(0.0);
    if max_scroll > 0.0 {
        let w = ui.wheel_in(out_box);
        if w != 0.0 {
            st.gemini.scroll = (st.gemini.scroll - w * WHEEL_STEP).clamp(0.0, max_scroll);
        }
    } else {
        st.gemini.scroll = 0.0;
    }

    ui.c.clip(inner);
    ui.c.text(&display_text, Rect::new(inner.x, inner.y - st.gemini.scroll, inner.w, text_h + 4.0), &body_st);
    ui.c.unclip();
}

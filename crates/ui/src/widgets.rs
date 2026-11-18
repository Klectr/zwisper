//! Tiny, consistent building blocks. Capsules, keycaps, cards, toggles.

use crate::theme::Theme;
use gpui::{div, prelude::*, px, App, ClickEvent, Div, ElementId, Hsla, Rgba, SharedString, Stateful, Window};

pub fn hsla(c: Rgba) -> Hsla {
    c.into()
}

/// Format "Alt+Shift+M" as macOS symbols "⌥⇧M".
pub fn shortcut_glyphs(chord: &str) -> Vec<String> {
    chord
        .split('+')
        .map(|k| match k.trim().to_lowercase().as_str() {
            "alt" | "option" => "⌥".into(),
            "shift" => "⇧".into(),
            "ctrl" | "control" => "⌃".into(),
            "cmd" | "super" | "meta" | "command" => "⌘".into(),
            "space" => "Space".into(),
            "escape" | "esc" => "esc".into(),
            other => other.to_uppercase(),
        })
        .collect()
}

pub fn keycap(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = Theme::get(cx);
    div()
        .px(px(6.))
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(5.))
        .bg(t.inset)
        .border_1()
        .border_color(t.hairline)
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.text_2)
        .child(text.into())
}

pub fn keycaps(chord: &str, cx: &App) -> Div {
    div().flex().gap(px(3.)).children(shortcut_glyphs(chord).into_iter().map(|g| keycap(g, cx)))
}

/// Rounded capsule button.
pub fn capsule(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Theme::get(cx);
    div()
        .id(id.into())
        .px(px(12.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded_full()
        .bg(t.raised)
        .border_1()
        .border_color(t.hairline)
        .text_size(px(13.))
        .text_color(t.text)
        .cursor_pointer()
        .hover(move |s| s.bg(t.inset))
        .on_click(on_click)
        .child(label.into())
}

/// Primary (filled) capsule.
pub fn primary(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Theme::get(cx);
    div()
        .id(id.into())
        .px(px(14.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(t.text)
        .text_size(px(13.))
        .text_color(t.window)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(move |s| s.opacity(0.85))
        .on_click(on_click)
        .child(label.into())
}

/// Round icon button with a text glyph.
pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: impl Into<SharedString>,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Theme::get(cx);
    div()
        .id(id.into())
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_size(px(14.))
        .text_color(t.text_2)
        .cursor_pointer()
        .hover(move |s| s.bg(t.inset).text_color(t.text))
        .on_click(on_click)
        .child(glyph.into())
}

pub fn card(cx: &App) -> Div {
    let t = Theme::get(cx);
    div().rounded(px(12.)).bg(t.raised).border_1().border_color(t.hairline)
}

pub fn section_label(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = Theme::get(cx);
    div()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.text_3)
        .child(text.into())
}

/// iOS-style switch.
pub fn toggle(id: impl Into<ElementId>, on: bool, cx: &App, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let t = Theme::get(cx);
    div()
        .id(id.into())
        .w(px(34.))
        .h(px(20.))
        .rounded_full()
        .p(px(2.))
        .flex()
        .when(on, |d| d.justify_end().bg(t.accent))
        .when(!on, |d| d.justify_start().bg(t.inset))
        .cursor_pointer()
        .on_click(on_click)
        .child(div().size(px(16.)).rounded_full().bg(gpui::white()))
}

/// Segmented control; returns the row; `on_select(i)` per segment.
pub fn segmented(
    id: &str,
    labels: &[&str],
    selected: usize,
    cx: &App,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> Div {
    let t = Theme::get(cx);
    let on_select = std::rc::Rc::new(on_select);
    div()
        .flex()
        .p(px(2.))
        .gap(px(2.))
        .rounded_full()
        .bg(t.inset)
        .children(labels.iter().enumerate().map(|(i, l)| {
            let f = on_select.clone();
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .px(px(12.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded_full()
                .text_size(px(12.))
                .cursor_pointer()
                .when(i == selected, |d| d.bg(t.raised).text_color(t.text).font_weight(gpui::FontWeight::SEMIBOLD))
                .when(i != selected, |d| d.text_color(t.text_2))
                .on_click(move |_, w, cx| f(i, w, cx))
                .child(l.to_string())
        }))
}

pub fn progress_bar(fraction: f32, cx: &App) -> Div {
    let t = Theme::get(cx);
    div()
        .h(px(4.))
        .w_full()
        .rounded_full()
        .bg(t.inset)
        .child(div().h_full().rounded_full().bg(t.accent).w(gpui::relative(fraction.clamp(0., 1.))))
}

/// Two bars (pause) drawn with divs, so we don't depend on font glyphs.
pub fn pause_glyph(color: Hsla) -> Div {
    div()
        .flex()
        .gap(px(3.))
        .child(div().w(px(3.)).h(px(11.)).rounded(px(1.)).bg(color))
        .child(div().w(px(3.)).h(px(11.)).rounded(px(1.)).bg(color))
}

pub fn stop_glyph(color: Hsla) -> Div {
    div().size(px(10.)).rounded(px(2.)).bg(color)
}

pub fn play_glyph(color: Hsla) -> Div {
    // A small filled circle is calmer than a triangle without an icon font.
    div().size(px(10.)).rounded_full().bg(color)
}

pub fn rec_dot(cx: &App) -> Div {
    let t = Theme::get(cx);
    div().size(px(8.)).rounded_full().bg(t.recording)
}

pub fn fmt_bytes(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.1} GB", b as f64 / 1e9)
    } else {
        format!("{} MB", (b as f64 / 1e6).round() as u64)
    }
}

/// Toast overlay anchored at the bottom of a window.
pub fn toast(cx: &App) -> Option<Div> {
    let state = crate::state::state(cx);
    let s = state.read(cx);
    let toast = s.toast.as_ref()?;
    let t = Theme::get(cx);
    Some(
        div().absolute().bottom(px(16.)).left_0().right_0().flex().justify_center().child(
            div()
                .max_w(px(460.))
                .px(px(14.))
                .py(px(8.))
                .rounded(px(10.))
                .bg(t.inset)
                .border_1()
                .border_color(if toast.error { hsla(t.recording) } else { t.hairline })
                .text_size(px(12.5))
                .text_color(t.text)
                .shadow_lg()
                .child(toast.text.clone()),
        ),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn glyphs() {
        assert_eq!(super::shortcut_glyphs("Alt+Shift+M"), vec!["⌥", "⇧", "M"]);
        assert_eq!(super::shortcut_glyphs("Alt+Space"), vec!["⌥", "Space"]);
    }
}

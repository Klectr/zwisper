//! Minimal Markdown renderer for generated notes: headings, bullets,
//! checkboxes (clickable), paragraphs. Deliberately small.

use crate::theme::Theme;
use gpui::{div, prelude::*, px, AnyElement, App, Div, SharedString};
use std::rc::Rc;

enum Line<'a> {
    H1,
    H2(&'a str),
    Check(bool, &'a str),
    Bullet(&'a str),
    Para(&'a str),
    Blank,
}

fn classify(l: &str) -> Line<'_> {
    let t = l.trim();
    if t.is_empty() {
        Line::Blank
    } else if t.starts_with("# ") {
        Line::H1
    } else if let Some(r) = t.strip_prefix("## ").or_else(|| t.strip_prefix("### ")) {
        Line::H2(r)
    } else if let Some(r) = t.strip_prefix("- [ ] ").or_else(|| t.strip_prefix("* [ ] ")) {
        Line::Check(false, r)
    } else if let Some(r) = t
        .strip_prefix("- [x] ")
        .or_else(|| t.strip_prefix("- [X] "))
        .or_else(|| t.strip_prefix("* [x] "))
    {
        Line::Check(true, r)
    } else if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("• ")) {
        Line::Bullet(r)
    } else {
        Line::Para(t)
    }
}

/// Strip simple inline emphasis markers.
fn inline(s: &str) -> String {
    s.replace("**", "").replace("__", "").replace('`', "")
}

/// Flip `- [ ]` ↔ `- [x]` on the given line.
pub fn toggle_checkbox(md: &str, line_ix: usize) -> String {
    md.lines()
        .enumerate()
        .map(|(i, l)| {
            if i != line_ix {
                return l.to_string();
            }
            if l.contains("[ ]") {
                l.replacen("[ ]", "[x]", 1)
            } else {
                l.replacen("[x]", "[ ]", 1).replacen("[X]", "[ ]", 1)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render(md: &str, cx: &App, on_toggle: impl Fn(usize, &mut App) + 'static) -> Div {
    let t = Theme::get(cx);
    let on_toggle = Rc::new(on_toggle);
    let mut out: Vec<AnyElement> = Vec::new();
    for (i, raw) in md.lines().enumerate() {
        let el: AnyElement = match classify(raw) {
            Line::Blank => div().h(px(6.)).into_any_element(),
            Line::H1 => continue, // the window already shows the title
            Line::H2(h) => div()
                .pt(px(14.))
                .pb(px(4.))
                .text_size(px(16.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(t.text)
                .child(inline(h))
                .into_any_element(),
            Line::Check(done, text) => {
                let f = on_toggle.clone();
                div()
                    .id(SharedString::from(format!("chk-{i}")))
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .py(px(3.))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| f(i, cx))
                    .child(
                        div()
                            .mt(px(3.))
                            .size(px(15.))
                            .flex_none()
                            .rounded_full()
                            .border_1()
                            .border_color(if done { t.accent.into() } else { t.text_3 })
                            .when(done, |d| d.bg(t.accent).flex().items_center().justify_center().text_size(px(9.)).text_color(gpui::white()).child("✓")),
                    )
                    .child(
                        div()
                            .text_size(px(14.5))
                            .line_height(px(21.))
                            .text_color(if done { t.text_3 } else { t.text })
                            .when(done, |d| d.line_through())
                            .child(inline(text)),
                    )
                    .into_any_element()
            }
            Line::Bullet(text) => div()
                .flex()
                .gap(px(8.))
                .py(px(2.))
                .child(div().text_color(t.text_3).child("•"))
                .child(div().text_size(px(14.5)).line_height(px(21.)).text_color(t.text).child(inline(text)))
                .into_any_element(),
            Line::Para(text) => div()
                .py(px(2.))
                .text_size(px(14.5))
                .line_height(px(22.))
                .text_color(t.text)
                .when(text.starts_with("Subject:"), |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                .child(inline(text))
                .into_any_element(),
        };
        out.push(el);
    }
    div().flex().flex_col().children(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn toggles() {
        let md = "## A\n- [ ] one\n- [x] two";
        assert_eq!(super::toggle_checkbox(md, 1), "## A\n- [x] one\n- [x] two");
        assert_eq!(super::toggle_checkbox(md, 2), "## A\n- [ ] one\n- [ ] two");
    }
}

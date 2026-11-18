//! The library: meetings grouped by day, plus a plain dictation history.

use crate::editor::{EditorEvent, TextEditor};
use crate::state::{state, AppState};
use crate::theme::Theme;
use crate::widgets::*;
use chrono::{Local, NaiveDate};
use gpui::{
    div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, ClipboardItem, Context, Entity, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use zwisper_core::domain::*;
use zwisper_core::Command;

pub struct LibraryView {
    state: Entity<AppState>,
    search: Entity<TextEditor>,
    tab: usize,
    sessions: Vec<SessionSummary>,
    dictations: Vec<Dictation>,
    seen_version: u64,
}

pub fn open(cx: &mut App) -> Option<AnyWindowHandle> {
    let st = state(cx);
    // gpui-kit wraps the view in its Root (needed for dropdowns, dialogs…).
    gpui::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(560.), px(680.)), cx))),
            titlebar: Some(TitlebarOptions {
                title: Some("Zwisper".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(16.))),
            }),
            window_min_size: Some(size(px(420.), px(400.))),
            ..Default::default()
        },
        cx,
        |_, cx| {
            cx.new(|cx| {
                let search = cx.new(|cx| TextEditor::new(cx, "Search", false));
                cx.subscribe(&search, |_: &mut LibraryView, _, e: &EditorEvent, cx| {
                    if matches!(e, EditorEvent::Changed) {
                        cx.notify()
                    }
                })
                .detach();
                cx.observe(&st, |this: &mut LibraryView, st, cx| {
                    let v = st.read(cx).library_version;
                    if v != this.seen_version {
                        this.seen_version = v;
                        this.reload(cx);
                    }
                    cx.notify();
                })
                .detach();
                cx.on_release(|_, cx| cx.defer(|cx| state(cx).update(cx, |s, _| s.window_closed("library")))).detach();
                let mut v = LibraryView { state: st, search, tab: 0, sessions: vec![], dictations: vec![], seen_version: 0 };
                v.reload(cx);
                v
            })
        },
    )
    .ok()
    .map(|(handle, _)| handle)
}

fn day_label(d: NaiveDate) -> String {
    let today = Local::now().date_naive();
    if d == today {
        "Today".into()
    } else if Some(d) == today.pred_opt() {
        "Yesterday".into()
    } else if (today - d).num_days() < 7 {
        d.format("%A").to_string()
    } else {
        d.format("%B %-d, %Y").to_string()
    }
}

impl LibraryView {
    fn reload(&mut self, cx: &mut Context<Self>) {
        let core = &self.state.read(cx).core;
        self.sessions = core.library();
        self.dictations = core.dictations(200);
    }

    fn matches(&self, s: &SessionSummary, q: &str) -> bool {
        q.is_empty() || s.title.to_lowercase().contains(q) || s.preview.to_lowercase().contains(q)
    }

    fn meetings(&mut self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = Theme::get(cx);
        let q = self.search.read(cx).text().trim().to_lowercase();
        let live = self.state.read(cx).meeting.as_ref().map(|m| m.id);
        let mut out = Vec::new();
        let mut last_day: Option<NaiveDate> = None;
        let items: Vec<SessionSummary> = self.sessions.iter().filter(|s| self.matches(s, &q)).cloned().collect();
        if items.is_empty() {
            out.push(
                div()
                    .pt(px(80.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .text_color(t.text_3)
                    .text_size(px(13.))
                    .child(if q.is_empty() { "No meetings yet." } else { "Nothing matches." })
                    .when(q.is_empty(), |d| {
                        let chord = self.state.read(cx).settings.meeting_shortcut.clone();
                        d.child(div().flex().gap(px(6.)).items_center().child("Start one with").child(keycaps(&chord, cx)))
                    })
                    .into_any_element(),
            );
            return out;
        }
        for s in items {
            let local = s.created.with_timezone(&Local);
            let day = local.date_naive();
            if last_day != Some(day) {
                out.push(
                    div()
                        .pt(px(18.))
                        .pb(px(6.))
                        .px(px(4.))
                        .child(section_label(day_label(day), cx))
                        .into_any_element(),
                );
                last_day = Some(day);
            }
            let id = s.id;
            let is_live = live == Some(id);
            let processing = matches!(s.state, SessionState::Processing);
            out.push(
                card(cx)
                    .id(SharedString::from(format!("s-{id}")))
                    .mb(px(8.))
                    .px(px(14.))
                    .py(px(12.))
                    .cursor_pointer()
                    .hover(move |d| d.bg(t.inset))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.open_meeting(id, cx));
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .gap(px(12.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .min_w_0()
                                    .when(is_live, |d| d.child(rec_dot(cx)))
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(t.text)
                                            .truncate()
                                            .child(s.title.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.))
                                    .text_color(t.text_3)
                                    .child(local.format("%-I:%M %p").to_string().to_lowercase()),
                            ),
                    )
                    .child(
                        div()
                            .pt(px(4.))
                            .text_size(px(12.5))
                            .text_color(t.text_2)
                            .truncate()
                            .child(if is_live {
                                "Recording now".to_string()
                            } else if processing {
                                "Writing notes…".to_string()
                            } else if s.preview.is_empty() {
                                fmt_clock(s.duration_ms)
                            } else {
                                s.preview.clone()
                            }),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    fn dictation_rows(&mut self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = Theme::get(cx);
        let q = self.search.read(cx).text().trim().to_lowercase();
        let mut out = Vec::new();
        let mut last_day: Option<NaiveDate> = None;
        for (i, d) in self.dictations.iter().enumerate() {
            if !q.is_empty() && !d.text.to_lowercase().contains(&q) {
                continue;
            }
            let local = d.at.with_timezone(&Local);
            let day = local.date_naive();
            if last_day != Some(day) {
                out.push(div().pt(px(18.)).pb(px(6.)).px(px(4.)).child(section_label(day_label(day), cx)).into_any_element());
                last_day = Some(day);
            }
            let text = d.text.clone();
            out.push(
                card(cx)
                    .mb(px(8.))
                    .px(px(14.))
                    .py(px(10.))
                    .flex()
                    .gap(px(12.))
                    .items_start()
                    .child(div().flex_1().text_size(px(13.5)).line_height(px(20.)).text_color(t.text).child(d.text.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(4.))
                            .child(div().text_size(px(11.5)).text_color(t.text_3).child(local.format("%-I:%M %p").to_string().to_lowercase()))
                            .child(icon_button(SharedString::from(format!("cp-{i}")), "⧉", cx, {
                                let st = self.state.clone();
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                    st.update(cx, |s, cx| s.notify_toast("Copied", false, cx));
                                }
                            })),
                    )
                    .into_any_element(),
            );
        }
        if out.is_empty() {
            out.push(
                div()
                    .pt(px(80.))
                    .flex()
                    .justify_center()
                    .text_size(px(13.))
                    .text_color(t.text_3)
                    .child("Dictations you make appear here.")
                    .into_any_element(),
            );
        }
        out
    }
}

impl Render for LibraryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let rows = if self.tab == 0 { self.meetings(cx) } else { self.dictation_rows(cx) };
        let this = cx.entity();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.window)
            .text_color(t.text)
            .font_family(crate::theme::FONT)
            .child(
                div()
                    .id("lib-toolbar")
                    .h(px(48.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(84.))
                    .pr(px(12.))
                    .on_mouse_down(gpui::MouseButton::Left, |_, window, _| window.start_window_move())
                    .child(segmented("lib-tab", &["Meetings", "Dictations"], self.tab, cx, move |i, _, cx| {
                        this.update(cx, |v, cx| {
                            v.tab = i;
                            v.reload(cx);
                            cx.notify();
                        })
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(icon_button("settings", "⚙", cx, |_, _, cx| {
                                state(cx).update(cx, |s, cx| s.open_settings(cx));
                            }))
                            .child(icon_button("new", "+", cx, |_, _, cx| {
                                state(cx).read(cx).send(Command::StartMeeting { title: None });
                            })),
                    ),
            )
            .child(
                div().px(px(20.)).pb(px(4.)).child(
                    div()
                        .h(px(30.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .rounded(px(8.))
                        .bg(t.inset)
                        .text_size(px(13.))
                        .child(self.search.clone()),
                ),
            )
            .child(div().id("lib-list").flex_1().overflow_y_scroll().px(px(20.)).pb(px(24.)).children(rows))
            .children(toast(cx))
    }
}

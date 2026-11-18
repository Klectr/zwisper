//! The meeting window: your notes on the left, the live transcript on the
//! right, a floating control capsule while recording.

use crate::editor::{EditorEvent, TextEditor};
use crate::markdown;
use crate::state::{state, AppState, UiEvent};
use crate::theme::Theme;
use crate::widgets::*;
use gpui::{
    anchored, deferred, div, point, prelude::*, px, size, Animation, AnimationExt, AnyWindowHandle, App, Bounds,
    ClipboardItem, Context, Entity, FocusHandle, Focusable, MouseButton, ScrollHandle, SharedString, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use std::time::Duration;
use zwisper_core::domain::*;
use zwisper_core::Command;

#[derive(Clone, PartialEq)]
enum Menu {
    Templates,
    More,
    Reassign(u64),
}

pub struct MeetingView {
    id: SessionId,
    state: Entity<AppState>,
    session: Option<Session>,
    focus: FocusHandle,
    title: Entity<TextEditor>,
    my_notes: Entity<TextEditor>,
    note_editor: Entity<TextEditor>,
    editing_note: bool,
    streaming: Option<(Template, String)>,
    show_transcript: bool,
    menu: Option<Menu>,
    rename: Option<(SpeakerId, Entity<TextEditor>)>,
    scroll: ScrollHandle,
    follow: bool,
    save_notes: Option<Task<()>>,
    save_title: Option<Task<()>>,
}

pub fn open(id: SessionId, cx: &mut App) -> Option<AnyWindowHandle> {
    let bounds = Bounds::centered(None, size(px(940.), px(640.)), cx);
    let st = state(cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Meeting".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(16.))),
            }),
            window_min_size: Some(size(px(560.), px(420.))),
            focus: true,
            ..Default::default()
        },
        |window, cx| cx.new(|cx| MeetingView::new(id, st, window, cx)),
    )
    .ok()
    .map(|h| h.into())
}

impl MeetingView {
    fn new(id: SessionId, st: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session = st.read(cx).core.session(id);
        let title = cx.new(|cx| TextEditor::new(cx, "Untitled meeting", false));
        let my_notes = cx.new(|cx| TextEditor::new(cx, "Jot down notes… they guide the summary.", true));
        let note_editor = cx.new(|cx| TextEditor::new(cx, "", true));
        if let Some(s) = &session {
            title.update(cx, |e, cx| e.set_text(&s.title, cx));
            my_notes.update(cx, |e, cx| e.set_text(&s.my_notes, cx));
            window.set_window_title(&s.title);
        }

        cx.subscribe(&title, |this, _, e: &EditorEvent, cx| match e {
            EditorEvent::Changed => this.debounce_title(cx),
            EditorEvent::Submit => this.commit_title(cx),
            EditorEvent::Cancel => {}
        })
        .detach();
        cx.subscribe(&my_notes, |this, _, e: &EditorEvent, cx| {
            if matches!(e, EditorEvent::Changed) {
                this.debounce_notes(cx);
            }
        })
        .detach();
        cx.subscribe(&note_editor, |this, _, e: &EditorEvent, cx| {
            if matches!(e, EditorEvent::Cancel) {
                this.finish_edit_note(cx);
            }
        })
        .detach();
        cx.subscribe(&st, |this, _, e: &UiEvent, cx| this.on_ui_event(e, cx)).detach();
        cx.observe(&st, |_, _, cx| cx.notify()).detach();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| state(cx).update(cx, |s, _| s.meeting_closed(id)));
        })
        .detach();
        window.on_window_should_close(cx, move |window, cx| {
            let st = state(cx);
            let recording = st.read(cx).meeting.as_ref().map(|m| m.id) == Some(id);
            if recording {
                // Closing a recording meeting ends it (the summary is still written).
                st.read(cx).send(Command::EndMeeting);
            }
            let _ = window;
            true
        });

        // Focus "My notes" when recording starts.
        let recording = st.read(cx).meeting.as_ref().map(|m| m.id) == Some(id);
        if recording {
            let f = my_notes.focus_handle(cx);
            window.focus(&f, cx);
        }

        MeetingView {
            id,
            state: st,
            session,
            focus: cx.focus_handle(),
            title,
            my_notes,
            note_editor,
            editing_note: false,
            streaming: None,
            show_transcript: true,
            menu: None,
            rename: None,
            scroll: ScrollHandle::new(),
            follow: true,
            save_notes: None,
            save_title: None,
        }
    }

    fn send(&self, cmd: Command, cx: &App) {
        self.state.read(cx).send(cmd);
    }

    fn debounce_notes(&mut self, cx: &mut Context<Self>) {
        self.save_notes = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(600)).await;
            let _ = this.update(cx, |this, cx| {
                let text = this.my_notes.read(cx).text().to_string();
                this.send(Command::SetMyNotes { id: this.id, text }, cx);
            });
        }));
    }

    fn debounce_title(&mut self, cx: &mut Context<Self>) {
        self.save_title = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            let _ = this.update(cx, |this, cx| this.commit_title(cx));
        }));
    }

    fn commit_title(&mut self, cx: &mut Context<Self>) {
        let title = self.title.read(cx).text().trim().to_string();
        if !title.is_empty() && self.session.as_ref().map(|s| s.title != title).unwrap_or(false) {
            self.send(Command::SetTitle { id: self.id, title }, cx);
        }
    }

    fn on_ui_event(&mut self, e: &UiEvent, cx: &mut Context<Self>) {
        match e {
            UiEvent::Session(s) if s.id == self.id => {
                let s = (**s).clone();
                // Don't clobber text the user is typing.
                self.title.update(cx, |ed, cx| {
                    if ed.text() != s.title && self.save_title.is_none() {
                        ed.set_text(&s.title, cx)
                    }
                });
                self.my_notes.update(cx, |ed, cx| {
                    if ed.text().is_empty() && !s.my_notes.is_empty() {
                        ed.set_text(&s.my_notes, cx)
                    }
                });
                self.session = Some(s);
            }
            UiEvent::Live(id, seg) if *id == self.id => {
                if let Some(s) = self.session.as_mut() {
                    s.segments.push(seg.clone());
                }
                if self.follow {
                    self.scroll.scroll_to_bottom();
                }
            }
            UiEvent::NotesStarted(id, t) if *id == self.id => self.streaming = Some((*t, String::new())),
            UiEvent::NotesDelta(id, t, d) if *id == self.id => {
                if let Some((st, text)) = self.streaming.as_mut() {
                    if st == t {
                        text.push_str(d);
                    }
                }
            }
            UiEvent::NotesDone(id, _) if *id == self.id => {
                self.streaming = None;
                if let Some(s) = self.state.read(cx).core.session(self.id) {
                    self.session = Some(s);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn active_template(&self) -> Template {
        self.streaming
            .as_ref()
            .map(|(t, _)| *t)
            .or_else(|| self.session.as_ref().and_then(|s| s.active_template))
            .unwrap_or(Template::DEFAULT)
    }

    fn current_note(&self) -> Option<String> {
        let s = self.session.as_ref()?;
        s.notes.get(&self.active_template()).map(|n| n.markdown.clone())
    }

    fn copy_note(&mut self, cx: &mut Context<Self>) {
        let mut out = String::new();
        if let Some(s) = &self.session {
            out.push_str(&format!("# {}\n\n", s.title));
        }
        if let Some(n) = self.current_note() {
            out.push_str(n.trim());
            out.push('\n');
        }
        let mine = self.my_notes.read(cx).text().trim().to_string();
        if !mine.is_empty() {
            out.push_str(&format!("\n## My notes\n\n{mine}\n"));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(out));
        self.state.update(cx, |s, cx| s.notify_toast("Copied notes", false, cx));
    }

    fn copy_transcript(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            cx.write_to_clipboard(ClipboardItem::new_string(s.transcript_lines()));
            self.state.update(cx, |s, cx| s.notify_toast("Copied transcript", false, cx));
        }
    }

    fn toggle_checkbox(&mut self, line_ix: usize, cx: &mut Context<Self>) {
        let Some(md) = self.current_note() else { return };
        let new = markdown::toggle_checkbox(&md, line_ix);
        let t = self.active_template();
        if let Some(s) = self.session.as_mut() {
            if let Some(n) = s.notes.get_mut(&t) {
                n.markdown = new.clone();
            }
        }
        self.send(Command::EditNote { id: self.id, template: t, markdown: new }, cx);
        cx.notify();
    }

    fn start_edit_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let md = self.current_note().unwrap_or_default();
        self.note_editor.update(cx, |e, cx| e.set_text(&md, cx));
        self.editing_note = true;
        let f = self.note_editor.focus_handle(cx);
        window.focus(&f, cx);
        cx.notify();
    }

    fn finish_edit_note(&mut self, cx: &mut Context<Self>) {
        if !self.editing_note {
            return;
        }
        self.editing_note = false;
        let md = self.note_editor.read(cx).text().to_string();
        let t = self.active_template();
        if self.current_note().as_deref() != Some(md.as_str()) {
            if let Some(s) = self.session.as_mut() {
                if let Some(n) = s.notes.get_mut(&t) {
                    n.markdown = md.clone();
                }
            }
            self.send(Command::EditNote { id: self.id, template: t, markdown: md }, cx);
        }
        cx.notify();
    }

    fn begin_rename(&mut self, sp: SpeakerId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.session.as_ref().map(|s| s.speaker_name(sp).to_string()).unwrap_or_default();
        let ed = cx.new(|cx| {
            let mut e = TextEditor::new(cx, "Name", false);
            e.set_text(&name, cx);
            e
        });
        cx.subscribe(&ed, move |this, ed, e: &EditorEvent, cx| match e {
            EditorEvent::Submit => {
                let name = ed.read(cx).text().trim().to_string();
                if !name.is_empty() {
                    this.send(Command::RenameSpeaker { id: this.id, speaker: sp, name }, cx);
                }
                this.rename = None;
                cx.notify();
            }
            EditorEvent::Cancel => {
                this.rename = None;
                cx.notify();
            }
            EditorEvent::Changed => {}
        })
        .detach();
        let f = ed.focus_handle(cx);
        window.focus(&f, cx);
        self.rename = Some((sp, ed));
        self.menu = None;
        cx.notify();
    }

    // ───── rendering ─────

    fn toolbar(&mut self, recording: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let tpl = self.active_template();
        let title = self.session.as_ref().map(|s| s.title.clone()).unwrap_or_default();
        div()
            .id("toolbar")
            .h(px(48.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(84.))
            .pr(px(12.))
            .border_b_1()
            .border_color(t.hairline)
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_w_0()
                    .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.text_2).truncate().child(title))
                    .when(!recording, |d| {
                        d.child(
                            div()
                                .id("tpl")
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .px(px(10.))
                                .h(px(26.))
                                .rounded_full()
                                .bg(t.raised)
                                .border_1()
                                .border_color(t.hairline)
                                .text_size(px(12.))
                                .text_color(t.text)
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.menu = if this.menu == Some(Menu::Templates) { None } else { Some(Menu::Templates) };
                                    cx.notify();
                                }))
                                .child(tpl.label())
                                .child(div().text_color(t.text_3).child("⌄")),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(!recording, |d| {
                        d.child(capsule("copy", "Copy", cx, {
                            let this = cx.entity();
                            move |_, _, cx| this.update(cx, |v, cx| v.copy_note(cx))
                        }))
                        .child(icon_button("more", "⋯", cx, {
                            let this = cx.entity();
                            move |_, _, cx| {
                                this.update(cx, |v, cx| {
                                    v.menu = if v.menu == Some(Menu::More) { None } else { Some(Menu::More) };
                                    cx.notify();
                                })
                            }
                        }))
                    })
                    .child(icon_button("sidebar", if self.show_transcript { "▥" } else { "▤" }, cx, {
                        let this = cx.entity();
                        move |_, _, cx| {
                            this.update(cx, |v, cx| {
                                v.show_transcript = !v.show_transcript;
                                cx.notify();
                            })
                        }
                    })),
            )
    }

    fn menu_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.menu.clone()?;
        let t = Theme::get(cx);
        let item = |id: SharedString, label: String, danger: bool| {
            div()
                .id(id)
                .px(px(10.))
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(13.))
                .text_color(if danger { hsla(t.recording) } else { t.text })
                .cursor_pointer()
                .hover(move |s| s.bg(t.inset))
                .child(label)
        };
        let s = self.session.as_ref();
        let (pos, items): (gpui::Point<gpui::Pixels>, Vec<gpui::AnyElement>) = match menu {
            Menu::Templates => {
                let current = self.active_template();
                let have: Vec<Template> = s.map(|s| s.notes.keys().copied().collect()).unwrap_or_default();
                (
                    point(px(250.), px(44.)),
                    Template::ALL
                        .iter()
                        .map(|tp| {
                            let tp = *tp;
                            let mark = if tp == current { "✓  " } else if have.contains(&tp) { "•  " } else { "    " };
                            item(SharedString::from(format!("tpl-{tp:?}")), format!("{mark}{}", tp.label()), false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.menu = None;
                                    this.send(Command::SelectTemplate { id: this.id, template: tp }, cx);
                                    cx.notify();
                                }))
                                .into_any_element()
                        })
                        .collect(),
                )
            }
            Menu::More => {
                let w = window.bounds().size.width;
                let folder = self.state.read(cx).core.session_folder(self.id);
                (
                    point(w - px(230.), px(44.)),
                    vec![
                        item("m-regen".into(), "Regenerate notes".into(), false)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                let t = this.active_template();
                                this.send(Command::GenerateNotes { id: this.id, template: t, force: true }, cx);
                                cx.notify();
                            }))
                            .into_any_element(),
                        item("m-edit".into(), "Edit notes".into(), false)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.menu = None;
                                this.start_edit_note(window, cx);
                            }))
                            .into_any_element(),
                        item("m-copyt".into(), "Copy transcript".into(), false)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                this.copy_transcript(cx);
                                cx.notify();
                            }))
                            .into_any_element(),
                        item("m-folder".into(), "Show in folder".into(), false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                if let Some(f) = &folder {
                                    cx.reveal_path(f);
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                        item("m-del".into(), "Delete meeting".into(), true)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.menu = None;
                                let id = this.id;
                                let answer = window.prompt(
                                    gpui::PromptLevel::Warning,
                                    "Delete this meeting?",
                                    Some("The recording, transcript and notes will be removed."),
                                    &["Delete", "Cancel"],
                                    cx,
                                );
                                let st = this.state.clone();
                                cx.spawn(async move |_, cx| {
                                    if answer.await == Ok(0) {
                                        let _ = st.update(cx, |s, _| s.send(Command::DeleteSession { id }));
                                    }
                                })
                                .detach();
                                cx.notify();
                            }))
                            .into_any_element(),
                    ],
                )
            }
            Menu::Reassign(_) if s.is_none() => return None,
            Menu::Reassign(seg) => {
                let w = window.bounds().size.width;
                let speakers: Vec<Speaker> = s.unwrap().speakers.clone();
                (
                    point(w - px(260.), px(90.)),
                    speakers
                        .into_iter()
                        .map(|sp| {
                            let id = sp.id;
                            item(SharedString::from(format!("ra-{}", id.0)), format!("Move to {}", sp.name), false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.menu = None;
                                    this.send(Command::ReassignSegment { id: this.id, segment: seg, speaker: id }, cx);
                                    cx.notify();
                                }))
                                .into_any_element()
                        })
                        .collect(),
                )
            }
        };
        Some(
            deferred(
                anchored().position(pos).child(
                    div()
                        .id("menu")
                        .occlude()
                        .min_w(px(200.))
                        .p(px(4.))
                        .rounded(px(10.))
                        .bg(t.raised)
                        .border_1()
                        .border_color(t.hairline)
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(1),
        )
    }

    fn notes_pane(&mut self, recording: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let processing = self.state.read(cx).pipeline.get(&self.id).cloned();
        let note_md = if let Some((_, text)) = &self.streaming {
            Some(text.clone())
        } else {
            self.current_note()
        };

        let generated = if recording {
            None
        } else if self.editing_note {
            Some(
                div()
                    .rounded(px(8.))
                    .bg(t.raised)
                    .p(px(12.))
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .font_family("monospace")
                    .text_color(t.text)
                    .child(self.note_editor.clone())
                    .child(
                        div().pt(px(8.)).flex().justify_end().child(primary("done-edit", "Done", cx, {
                            let this = cx.entity();
                            move |_, _, cx| this.update(cx, |v, cx| v.finish_edit_note(cx))
                        })),
                    )
                    .into_any_element(),
            )
        } else if let Some(md) = note_md.filter(|m| !m.trim().is_empty()) {
            let this = cx.entity();
            let streaming = self.streaming.is_some();
            Some(
                div()
                    .id("note")
                    .on_click(cx.listener(|this, e: &gpui::ClickEvent, window, cx| {
                        if e.click_count() == 2 && this.streaming.is_none() {
                            this.start_edit_note(window, cx);
                        }
                    }))
                    .child(markdown::render(&md, cx, move |ix, cx| {
                        if !streaming {
                            this.update(cx, |v, cx| v.toggle_checkbox(ix, cx));
                        }
                    }))
                    .into_any_element(),
            )
        } else if let Some(step) = processing.clone() {
            Some(
                div()
                    .text_size(px(14.))
                    .text_color(t.text_2)
                    .child(format!("{step}…"))
                    .with_animation(
                        "proc",
                        Animation::new(Duration::from_millis(1100)).repeat(),
                        |el, d| el.opacity(0.4 + 0.6 * (1.0 - (2.0 * d - 1.0).abs())),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        let has_note = generated.is_some();
        div()
            .id("notes-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(40.))
            .pt(px(28.))
            .pb(px(96.))
            .child(
                div()
                    .text_size(px(24.))
                    .line_height(px(32.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(t.text)
                    .child(self.title.clone()),
            )
            .when_some(self.session.as_ref(), |d, s| {
                d.child(
                    div()
                        .pt(px(4.))
                        .text_size(px(12.))
                        .text_color(t.text_3)
                        .child(s.created.with_timezone(&chrono::Local).format("%A, %B %-d · %-I:%M %p").to_string()),
                )
            })
            .when_some(generated, |d, g| d.child(div().pt(px(20.)).child(g)))
            .child(
                div()
                    .pt(px(28.))
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(if has_note { t.text_3 } else { t.text })
                            .child("My notes"),
                    )
                    .child(
                        div()
                            .pt(px(8.))
                            .text_size(px(14.5))
                            .line_height(px(22.))
                            .text_color(if has_note { t.text_2 } else { t.text })
                            .child(self.my_notes.clone()),
                    ),
            )
    }

    fn transcript_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        if let Some(s) = &self.session {
            let mut last: Option<SpeakerId> = None;
            for seg in &s.segments {
                let sp = seg.speaker_id;
                let new_speaker = last != Some(sp);
                last = Some(sp);
                let color = if sp == SpeakerId::UNKNOWN {
                    hsla(t.speakers[1]).opacity(0.6)
                } else {
                    hsla(t.speaker(s.speaker(sp).map(|x| x.color_idx).unwrap_or(0)))
                };
                let renaming = self.rename.as_ref().filter(|(r, _)| *r == sp && new_speaker).map(|(_, e)| e.clone());
                let seg_id = seg.id;
                let header = if !new_speaker {
                    None
                } else if let Some(ed) = renaming {
                    Some(
                        div()
                            .pt(px(14.))
                            .w(px(180.))
                            .text_size(px(12.5))
                            .text_color(color)
                            .child(div().px(px(6.)).rounded(px(4.)).bg(t.inset).child(ed))
                            .into_any_element(),
                    )
                } else {
                    Some(
                        div()
                            .pt(px(14.))
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .id(SharedString::from(format!("sp-{seg_id}")))
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(color)
                                    .cursor_pointer()
                                    .when(sp != SpeakerId::UNKNOWN, |d| {
                                        d.on_click(cx.listener(move |this, _, window, cx| this.begin_rename(sp, window, cx)))
                                    })
                                    .child(s.speaker_name(sp).to_string()),
                            )
                            .child(div().text_size(px(11.5)).text_color(t.text_3).child(fmt_clock(seg.start_ms)))
                            .into_any_element(),
                    )
                };
                rows.push(
                    div()
                        .id(SharedString::from(format!("seg-{seg_id}")))
                        .children(header)
                        .child(
                            div()
                                .pt(px(2.))
                                .text_size(px(14.))
                                .line_height(px(21.))
                                .text_color(t.text)
                                .child(seg.text.clone()),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, _, cx| {
                                this.menu = Some(Menu::Reassign(seg_id));
                                cx.notify();
                            }),
                        )
                        .into_any_element(),
                );
            }
        }
        let empty = rows.is_empty();
        let recording = self.state.read(cx).meeting.as_ref().map(|m| m.id) == Some(self.id);
        div()
            .w(px(340.))
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(t.hairline)
            .flex()
            .flex_col()
            .child(
                div()
                    .id("transcript")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(20.))
                    .pb(px(24.))
                    .on_scroll(cx.listener(|this, _, _, _| {
                        let off = this.scroll.offset().y;
                        let max = this.scroll.max_offset().y;
                        this.follow = -off >= max - px(24.);
                    }))
                    .when(empty, |d| {
                        d.child(
                            div()
                                .pt(px(24.))
                                .text_size(px(13.))
                                .text_color(t.text_3)
                                .child(if recording { "Listening… the transcript appears here." } else { "No transcript." }),
                        )
                    })
                    .children(rows),
            )
    }

    fn controls(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = Theme::get(cx);
        let st = self.state.read(cx);
        let live = st.meeting.as_ref().filter(|m| m.id == self.id)?;
        let paused = live.state == SessionState::Paused;
        let elapsed = fmt_clock(live.elapsed_ms);
        let level = st.meeting_levels.values().cloned().fold(0.0f32, f32::max);
        Some(
            div().absolute().bottom(px(20.)).left_0().right_0().flex().justify_center().child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .px(px(16.))
                    .h(px(44.))
                    .rounded_full()
                    .bg(t.raised)
                    .border_1()
                    .border_color(t.hairline)
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(rec_dot(cx).opacity(if paused { 0.3 } else { 0.5 + 0.5 * level }))
                            .child(div().text_size(px(13.)).text_color(t.text).w(px(52.)).child(elapsed)),
                    )
                    .child(
                        div()
                            .id("pause")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(13.))
                            .text_color(t.text_2)
                            .cursor_pointer()
                            .hover(move |s| s.text_color(t.text))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(if paused { Command::ResumeMeeting } else { Command::PauseMeeting }, cx)
                            }))
                            .child(if paused { play_glyph(t.text_2) } else { pause_glyph(t.text_2) })
                            .child(if paused { "Resume" } else { "Pause" }),
                    )
                    .child(
                        div()
                            .id("end")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(12.))
                            .h(px(28.))
                            .rounded_full()
                            .bg(t.recording)
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(gpui::white())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                // Save what the user typed before ending.
                                let text = this.my_notes.read(cx).text().to_string();
                                this.send(Command::SetMyNotes { id: this.id, text }, cx);
                                this.commit_title(cx);
                                this.send(Command::EndMeeting, cx);
                            }))
                            .child(stop_glyph(gpui::white().into()))
                            .child("End"),
                    ),
            ),
        )
    }
}

impl Focusable for MeetingView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MeetingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let recording = self.state.read(cx).meeting.as_ref().map(|m| m.id) == Some(self.id);
        if let Some(s) = &self.session {
            window.set_window_title(&s.title);
        }
        let toolbar = self.toolbar(recording, cx);
        let notes = self.notes_pane(recording, cx);
        let transcript = if self.show_transcript { Some(self.transcript_pane(cx)) } else { None };
        let controls = self.controls(cx);
        let menu = self.menu_overlay(window, cx);
        div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(t.window)
            .text_color(t.text)
            .font_family(crate::theme::FONT)
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .relative()
                    .child(div().flex_1().min_w_0().h_full().relative().flex().child(notes).children(controls))
                    .children(transcript),
            )
            .children(menu)
            .children(toast(cx))
    }
}


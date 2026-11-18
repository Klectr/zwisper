//! Settings: one screen of grouped cards. Nothing more.

use crate::state::{state, AppState};
use crate::theme::Theme;
use crate::widgets::*;
use gpui::{
    div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Div, Entity, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};
use zwisper_core::domain::*;
use zwisper_core::Command;

pub struct SettingsView {
    state: Entity<AppState>,
    mics: Vec<String>,
}

pub fn open(cx: &mut App) -> Option<AnyWindowHandle> {
    let st = state(cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(520.), px(720.)), cx))),
            titlebar: Some(TitlebarOptions {
                title: Some("Settings".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(16.))),
            }),
            is_resizable: false,
            ..Default::default()
        },
        |_, cx| {
            cx.new(|cx| {
                cx.observe(&st, |_, _, cx| cx.notify()).detach();
                cx.on_release(|_, cx| cx.defer(|cx| state(cx).update(cx, |s, _| s.window_closed("settings")))).detach();
                let mics = st.read(cx).core.microphones();
                SettingsView { state: st, mics }
            })
        },
    )
    .ok()
    .map(|h| h.into())
}

fn row(label: impl Into<SharedString>, right: impl IntoElement, cx: &App) -> Div {
    let t = Theme::get(cx);
    div()
        .min_h(px(44.))
        .px(px(14.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .text_size(px(13.5))
        .text_color(t.text)
        .child(label.into())
        .child(right)
}

fn divider(cx: &App) -> Div {
    let t = Theme::get(cx);
    div().mx(px(14.)).h(px(1.)).bg(t.hairline)
}

impl SettingsView {
    fn group(&self, title: &str, rows: Vec<gpui::AnyElement>, cx: &App) -> Div {
        let mut inner: Vec<gpui::AnyElement> = Vec::new();
        let n = rows.len();
        for (i, r) in rows.into_iter().enumerate() {
            inner.push(r);
            if i + 1 < n {
                inner.push(divider(cx).into_any_element());
            }
        }
        div()
            .pt(px(18.))
            .child(div().px(px(4.)).pb(px(6.)).child(section_label(title.to_string(), cx)))
            .child(card(cx).children(inner))
    }

    fn pack_row(&self, pack: Pack, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::get(cx);
        let status = self.state.read(cx).packs.get(&pack).cloned().unwrap_or(PackStatus::NotDownloaded { bytes: 0 });
        let desc = match pack {
            Pack::Speech => "Dictation and transcripts",
            Pack::Speakers => "Labels who said what",
            Pack::Notes => "Writes summaries and to-dos",
        };
        let right: gpui::AnyElement = match &status {
            PackStatus::NotDownloaded { bytes } => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().text_size(px(12.)).text_color(t.text_3).child(fmt_bytes(*bytes)))
                .child(capsule(SharedString::from(format!("dl-{pack:?}")), "Download", cx, move |_, _, cx| {
                    state(cx).read(cx).send(Command::DownloadPack(pack))
                }))
                .into_any_element(),
            PackStatus::Downloading { done, total } => div()
                .w(px(170.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().flex_1().child(progress_bar(*done as f32 / (*total).max(1) as f32, cx)))
                .child(icon_button(SharedString::from(format!("cx-{pack:?}")), "×", cx, move |_, _, cx| {
                    state(cx).read(cx).send(Command::CancelDownload(pack))
                }))
                .into_any_element(),
            PackStatus::Ready { bytes } => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().text_size(px(12.)).text_color(t.text_3).child(format!("Ready · {}", fmt_bytes(*bytes))))
                .child(icon_button(SharedString::from(format!("rm-{pack:?}")), "⌫", cx, move |_, _, cx| {
                    state(cx).read(cx).send(Command::RemovePack(pack))
                }))
                .into_any_element(),
            PackStatus::Failed(e) => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().max_w(px(160.)).truncate().text_size(px(12.)).text_color(hsla(t.recording)).child(e.clone()))
                .child(capsule(SharedString::from(format!("re-{pack:?}")), "Retry", cx, move |_, _, cx| {
                    state(cx).read(cx).send(Command::DownloadPack(pack))
                }))
                .into_any_element(),
        };
        div()
            .min_h(px(52.))
            .px(px(14.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(13.5)).text_color(t.text).child(pack.label()))
                    .child(div().text_size(px(12.)).text_color(t.text_3).child(desc)),
            )
            .child(right)
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let s = self.state.read(cx);
        let settings = s.settings.clone();
        let tier = s.tier;
        let mic_label = settings.microphone.clone().unwrap_or_else(|| "System default".into());
        let mics = self.mics.clone();
        let acks = s.core.acknowledgements();
        let folder = s.core.library_root();

        let shortcuts = self.group(
            "Shortcuts",
            vec![
                row("Dictate", keycaps(&settings.dictate_shortcut, cx), cx).into_any_element(),
                row("Start meeting", keycaps(&settings.meeting_shortcut, cx), cx).into_any_element(),
            ],
            cx,
        );

        let audio = self.group(
            "Audio",
            vec![
                row(
                    "Microphone",
                    capsule("mic", format!("{mic_label}  ⌄"), cx, move |_, _, cx| {
                        // Cycle: System default → each device → default.
                        let st = state(cx);
                        st.update(cx, |s, _| {
                            let cur = s.settings.microphone.clone();
                            let next = match cur {
                                None => mics.first().cloned(),
                                Some(c) => {
                                    let i = mics.iter().position(|m| *m == c);
                                    i.and_then(|i| mics.get(i + 1).cloned())
                                }
                            };
                            s.update_settings(|x| x.microphone = next);
                        });
                    }),
                    cx,
                )
                .into_any_element(),
                row(
                    "Record system audio in meetings",
                    toggle("sysaudio", settings.record_system_audio, cx, |_, _, cx| {
                        state(cx).update(cx, |s, _| s.update_settings(|x| x.record_system_audio = !x.record_system_audio));
                    }),
                    cx,
                )
                .into_any_element(),
            ],
            cx,
        );

        let tier_ix = if tier == Tier::Standard { 0 } else { 1 };
        let mut model_rows = vec![div()
            .px(px(14.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(13.5)).child("Notes model"))
                    .child(segmented("tier", &["Standard", "Lite"], tier_ix, cx, |i, _, cx| {
                        let t = if i == 0 { Tier::Standard } else { Tier::Lite };
                        state(cx).read(cx).send(Command::SetTier(t));
                    })),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.text_3)
                    .child("Lite uses less memory; notes are a little less polished."),
            )
            .into_any_element()];
        for p in Pack::ALL {
            model_rows.push(self.pack_row(p, cx));
        }
        let models = self.group("Models", model_rows, cx);

        let storage = self.group(
            "Storage",
            vec![
                row(
                    "Keep dictation history",
                    toggle("hist", settings.keep_dictation_history, cx, |_, _, cx| {
                        state(cx).update(cx, |s, _| s.update_settings(|x| x.keep_dictation_history = !x.keep_dictation_history));
                    }),
                    cx,
                )
                .into_any_element(),
                row(
                    "Library folder",
                    capsule("reveal", "Show", cx, move |_, _, cx| {
                        if let Some(f) = &folder {
                            cx.reveal_path(f);
                        }
                    }),
                    cx,
                )
                .into_any_element(),
                row(
                    "Dictation history",
                    capsule("clear", "Clear", cx, |_, _, cx| {
                        let st = state(cx);
                        st.update(cx, |s, cx| {
                            s.send(Command::ClearDictations);
                            s.library_version += 1;
                            s.notify_toast("Dictation history cleared", false, cx);
                        });
                    }),
                    cx,
                )
                .into_any_element(),
            ],
            cx,
        );

        let general = self.group(
            "General",
            vec![row(
                "Launch at login",
                toggle("login", settings.launch_at_login, cx, |_, _, cx| {
                    state(cx).update(cx, |s, cx| {
                        let on = !s.settings.launch_at_login;
                        match s.platform.set_launch_at_login(on) {
                            Ok(()) => s.update_settings(|x| x.launch_at_login = on),
                            Err(e) => s.notify_toast(format!("Couldn't change login item: {e}"), true, cx),
                        }
                    });
                }),
                cx,
            )
            .into_any_element()],
            cx,
        );

        let about = div()
            .pt(px(18.))
            .child(div().px(px(4.)).pb(px(6.)).child(section_label("About", cx)))
            .child(
                card(cx)
                    .px(px(14.))
                    .py(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(px(13.5)).child(format!("Zwisper {}", env!("CARGO_PKG_VERSION"))))
                    .child(div().text_size(px(12.)).text_color(t.text_3).child("Everything runs on this computer. Audio never leaves it."))
                    .children(acks.into_iter().map(|a| div().pt(px(4.)).text_size(px(11.)).text_color(t.text_3).child(a))),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.window)
            .text_color(t.text)
            .font_family(crate::theme::FONT)
            .child(
                div()
                    .h(px(48.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(t.text_2)
                    .on_mouse_down(gpui::MouseButton::Left, |_, window, _| window.start_window_move())
                    .child("Settings"),
            )
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .px(px(20.))
                    .pb(px(24.))
                    .child(shortcuts)
                    .child(audio)
                    .child(models)
                    .child(storage)
                    .child(general)
                    .child(about),
            )
            .children(toast(cx))
    }
}

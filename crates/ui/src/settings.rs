//! Settings: one screen of grouped cards. Nothing more.

use crate::state::{state, AppState};
use crate::theme::Theme;
use crate::widgets::*;
use gpui::{
    div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Div, Entity, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};
use gpui::component::tab::{Tab, TabBar};
use gpui::component::select::{Select, SelectEvent, SelectState};
use gpui::component::switch::Switch;
use gpui::component::{IndexPath, Sizable};
use zwisper_core::domain::*;
use zwisper_core::Command;

const SYSTEM_DEFAULT_MIC: &str = "System default";

pub struct SettingsView {
    state: Entity<AppState>,
    mic_select: Entity<SelectState<Vec<SharedString>>>,
}

/// A gpui-component switch bound to a settings flag.
fn switch(id: &'static str, on: bool, f: impl Fn(&mut Settings, bool) + 'static) -> Switch {
    Switch::new(id).checked(on).on_click(move |checked, _, cx| {
        let checked = *checked;
        state(cx).update(cx, |s, _| s.update_settings(|x| f(x, checked)));
    })
}

/// gpui-component's segmented control (replaces our `segmented`).
fn choice(id: &'static str, labels: &[&'static str], selected: usize, on_pick: impl Fn(usize, &mut App) + 'static) -> TabBar {
    TabBar::new(id)
        .segmented()
        .small()
        .selected_index(selected)
        .children(labels.iter().map(|l| Tab::new().label(*l)))
        .on_click(move |i: &usize, _, cx| on_pick(*i, cx))
}

pub fn open(cx: &mut App) -> Option<AnyWindowHandle> {
    let st = state(cx);
    // gpui-kit wraps the view in its Root (needed for dropdowns, dialogs…).
    gpui::open_window(
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
        cx,
        |window, cx| {
            let view = cx.new(|cx| {
                cx.observe(&st, |_, _, cx| cx.notify()).detach();
                cx.on_release(|_, cx| cx.defer(|cx| state(cx).update(cx, |s, _| s.window_closed("settings")))).detach();
                // Re-check permissions while open (no OS callback exists).
                cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                })
                .detach();
                // Microphone dropdown: "System default" + every input device.
                let current = st.read(cx).settings.microphone.clone();
                let mut items: Vec<SharedString> = vec![SYSTEM_DEFAULT_MIC.into()];
                items.extend(st.read(cx).core.microphones().into_iter().map(SharedString::from));
                let selected = current
                    .as_deref()
                    .and_then(|c| items.iter().position(|i| i.as_ref() == c))
                    .unwrap_or(0);
                let mic_select = cx.new(|cx| SelectState::new(items, Some(IndexPath::new(selected)), window, cx));
                cx.subscribe(&mic_select, |_, _, e: &SelectEvent<Vec<SharedString>>, cx| {
                    let SelectEvent::Confirm(Some(name)) = e else { return };
                    let mic = (name.as_ref() != SYSTEM_DEFAULT_MIC).then(|| name.to_string());
                    state(cx).update(cx, |s, _| s.update_settings(|x| x.microphone = mic));
                })
                .detach();
                SettingsView { state: st, mic_select }
            });
            view
        },
    )
    .ok()
    .map(|(handle, _)| handle)
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

        // Muffling captures other apps' audio, so it needs System audio access;
        // without it dictation quietly lowers the volume instead. Say so.
        let perms = s.platform.permissions();
        let muffle_hint = (settings.dictation_ducking == Ducking::Muffle
            && perms.state(PermissionKind::SystemAudio) != PermissionState::Granted)
            .then(|| {
                div()
                    .px(px(14.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_3)
                            .child("Muffling needs System audio access. Until then, other audio is just lowered."),
                    )
                    .child(capsule("muffle-allow", "Allow…", cx, |_, _, cx| {
                        let p = state(cx).read(cx).platform.permissions();
                        if p.state(PermissionKind::SystemAudio) == PermissionState::NotDetermined {
                            p.request(PermissionKind::SystemAudio);
                        } else {
                            p.open_settings(PermissionKind::SystemAudio);
                        }
                    }))
                    .into_any_element()
            });

        let audio = self.group(
            "Audio",
            vec![
                row("Microphone", div().w(px(220.)).child(Select::new(&self.mic_select).small().menu_width(px(260.))), cx)
                .into_any_element(),
                row(
                    "Record system audio in meetings",
                    switch("sysaudio", settings.record_system_audio, |x, on| x.record_system_audio = on),
                    cx,
                )
                .into_any_element(),
                row(
                    "Other audio while dictating",
                    choice(
                        "ducking",
                        &["Muffle", "Lower", "Leave"],
                        match settings.dictation_ducking {
                            Ducking::Muffle => 0,
                            Ducking::Lower => 1,
                            Ducking::Off => 2,
                        },
                        |i, cx| {
                            let how = [Ducking::Muffle, Ducking::Lower, Ducking::Off][i];
                            state(cx).update(cx, |s, _| s.update_settings(|x| x.dictation_ducking = how));
                        },
                    ),
                    cx,
                )
                .into_any_element(),
            ]
            .into_iter()
            .chain(muffle_hint)
            .collect(),
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
                    .child(choice("tier", &["Standard", "Lite"], tier_ix, |i, cx| {
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
                    switch("hist", settings.keep_dictation_history, |x, on| x.keep_dictation_history = on),
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
                Switch::new("login").checked(settings.launch_at_login).on_click(|on, _, cx| {
                    let on = *on;
                    state(cx).update(cx, |s, cx| {
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

//! First launch: welcome + Speech download, permissions, try it.

use crate::editor::TextEditor;
use crate::state::{state, AppState};
use crate::theme::Theme;
use crate::widgets::*;
use gpui::{
    div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Entity, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};
use std::time::Duration;
use zwisper_core::domain::*;
use zwisper_core::Command;

pub struct Onboarding {
    state: Entity<AppState>,
    step: usize,
    try_it: Entity<TextEditor>,
}

pub fn open(cx: &mut App) -> Option<AnyWindowHandle> {
    let st = state(cx);
    // gpui-kit wraps the view in its Root (needed for dropdowns, dialogs…).
    gpui::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(520.), px(520.)), cx))),
            titlebar: Some(TitlebarOptions {
                title: Some("Welcome to Zwisper".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(16.))),
            }),
            is_resizable: false,
            ..Default::default()
        },
        cx,
        |_, cx| {
            cx.new(|cx| {
                cx.observe(&st, |_, _, cx| cx.notify()).detach();
                cx.on_release(|_, cx| cx.defer(|cx| state(cx).update(cx, |s, _| s.window_closed("onboarding")))).detach();
                // Re-check permissions every second while visible (no OS callback exists).
                cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                })
                .detach();
                let try_it = cx.new(|cx| TextEditor::new(cx, "Click here, then press the shortcut and speak…", true));
                Onboarding { state: st, step: 0, try_it }
            })
        },
    )
    .ok()
    .map(|(handle, _)| handle)
}

impl Onboarding {
    fn perm_row(&self, kind: PermissionKind, label: &str, why: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::get(cx);
        let perms = self.state.read(cx).platform.permissions();
        let st = perms.state(kind);
        let right: gpui::AnyElement = match st {
            PermissionState::Granted => div().text_size(px(12.5)).text_color(hsla(t.speakers[0])).child("✓ Allowed").into_any_element(),
            _ => capsule(SharedString::from(format!("perm-{kind:?}")), "Allow…", cx, move |_, _, cx| {
                let p = state(cx).read(cx).platform.permissions();
                if p.state(kind) == PermissionState::NotDetermined {
                    p.request(kind);
                } else {
                    p.open_settings(kind);
                }
            })
            .into_any_element(),
        };
        div()
            .px(px(14.))
            .py(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(13.5)).text_color(t.text).child(label.to_string()))
                    .child(div().text_size(px(12.)).text_color(t.text_3).child(why.to_string())),
            )
            .child(right)
            .into_any_element()
    }
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let s = self.state.read(cx);
        let speech = s.packs.get(&Pack::Speech).cloned().unwrap_or(PackStatus::NotDownloaded { bytes: 0 });
        let chord = s.settings.dictate_shortcut.clone();
        let this = cx.entity();

        let body: gpui::AnyElement = match self.step {
            0 => div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(div().text_size(px(26.)).font_weight(gpui::FontWeight::BOLD).child("Just speak."))
                .child(
                    div()
                        .text_size(px(14.))
                        .line_height(px(21.))
                        .text_color(t.text_2)
                        .child("Zwisper turns your voice into text anywhere you type, and your meetings into clean notes. Everything runs on this Mac — nothing is uploaded."),
                )
                .child(
                    card(cx).mt(px(10.)).px(px(14.)).py(px(12.)).flex().flex_col().gap(px(8.)).child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(div().text_size(px(13.5)).child("Speech model"))
                            .child(match &speech {
                                PackStatus::Ready { .. } => div().text_size(px(12.5)).text_color(hsla(t.speakers[0])).child("✓ Ready"),
                                PackStatus::Downloading { done, total } => div()
                                    .text_size(px(12.5))
                                    .text_color(t.text_3)
                                    .child(format!("{} of {}", fmt_bytes(*done), fmt_bytes(*total))),
                                PackStatus::NotDownloaded { bytes } => div().text_size(px(12.5)).text_color(t.text_3).child(fmt_bytes(*bytes)),
                                PackStatus::Failed(e) => div().text_size(px(12.)).text_color(hsla(t.recording)).child(e.clone()),
                            }),
                    )
                    .when_some(
                        match &speech {
                            PackStatus::Downloading { done, total } => Some(*done as f32 / (*total).max(1) as f32),
                            _ => None,
                        },
                        |d, f| d.child(progress_bar(f, cx)),
                    ),
                )
                .into_any_element(),
            1 => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().text_size(px(22.)).font_weight(gpui::FontWeight::BOLD).child("A few permissions"))
                .child(div().text_size(px(13.5)).text_color(t.text_2).child("macOS asks before any app can hear or type."))
                .child(card(cx).mt(px(6.)).children([
                    self.perm_row(PermissionKind::Microphone, "Microphone", "To hear you", cx),
                    self.perm_row(PermissionKind::Accessibility, "Accessibility", "To paste text where you're typing", cx),
                    self.perm_row(PermissionKind::SystemAudio, "System audio", "To transcribe the other side of calls", cx),
                ]))
                .into_any_element(),
            _ => div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(div().text_size(px(22.)).font_weight(gpui::FontWeight::BOLD).child("Try it"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(13.5))
                        .text_color(t.text_2)
                        .child("Press")
                        .child(keycaps(&chord, cx))
                        .child("say something, then press it again. Hold it to talk instead."),
                )
                .child(
                    div()
                        .mt(px(6.))
                        .min_h(px(120.))
                        .p(px(12.))
                        .rounded(px(10.))
                        .bg(t.inset)
                        .text_size(px(14.))
                        .line_height(px(21.))
                        .child(self.try_it.clone()),
                )
                .into_any_element(),
        };

        let step = self.step;
        let ready = matches!(speech, PackStatus::Ready { .. });
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .child(div().flex().gap(px(6.)).children((0..3).map(|i| {
                div().size(px(6.)).rounded_full().bg(if i == step { t.text } else { t.text_3 })
            })))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .when(step > 0, |d| {
                        let this = this.clone();
                        d.child(capsule("back", "Back", cx, move |_, _, cx| this.update(cx, |v, cx| {
                            v.step -= 1;
                            cx.notify();
                        })))
                    })
                    .child({
                        let this = this.clone();
                        let label = match step {
                            0 if !ready && !matches!(speech, PackStatus::Downloading { .. }) => "Download & continue",
                            2 => "Done",
                            _ => "Continue",
                        };
                        primary("next", label, cx, move |_, window, cx| {
                            let st = state(cx);
                            this.update(cx, |v, cx| {
                                if v.step == 0 && matches!(st.read(cx).packs.get(&Pack::Speech), Some(PackStatus::NotDownloaded { .. } | PackStatus::Failed(_))) {
                                    st.read(cx).send(Command::DownloadPack(Pack::Speech));
                                }
                                if v.step == 2 {
                                    st.update(cx, |s, _| s.update_settings(|x| x.onboarded = true));
                                    window.remove_window();
                                    return;
                                }
                                v.step += 1;
                                if v.step == 2 {
                                    let f = gpui::Focusable::focus_handle(v.try_it.read(cx), cx);
                                    window.focus(&f, cx);
                                }
                                cx.notify();
                            });
                        })
                    }),
            );

        let _ = window;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.window)
            .text_color(t.text)
            .font_family(crate::theme::FONT)
            .child(div().h(px(40.)).flex_none().on_mouse_down(gpui::MouseButton::Left, |_, w, _| w.start_window_move()))
            .child(div().flex_1().px(px(36.)).pt(px(20.)).child(body))
            .child(div().px(px(28.)).pb(px(24.)).child(footer))
            .children(toast(cx))
    }
}

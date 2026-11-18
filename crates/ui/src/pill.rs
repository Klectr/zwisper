//! The dictation pill: a small floating, non-activating panel near the bottom
//! of the screen. Waveform on top, an inset footer with keyboard hints below.

use crate::state::{state, AppState};
use crate::theme::Theme;
use crate::widgets::*;
use gpui::{
    div, point, prelude::*, px, size, Animation, AnimationExt, App, Bounds, Context, Entity, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use std::time::Duration;
use zwisper_core::{Command, DictationPhase};

pub struct Pill {
    state: Entity<AppState>,
}

pub const W: f32 = 420.;
pub const H: f32 = 110.;
/// Transparent margin around the pill so its shadow isn't clipped by the window.
const MARGIN: f32 = 16.;

pub fn open(cx: &mut App) -> Option<WindowHandle<Pill>> {
    let display = cx.primary_display();
    let area = display.as_ref().map(|d| d.bounds()).unwrap_or(Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.))));
    let origin = point(
        area.origin.x + (area.size.width - px(W)) / 2. - px(MARGIN),
        area.origin.y + area.size.height - px(H) - px(96.) - px(MARGIN),
    );
    let st = state(cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, size(px(W + 2. * MARGIN), px(H + 2. * MARGIN))))),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: display.map(|d| d.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        |window, cx| {
            st.read(cx).platform.make_frameless(window);
            cx.new(|cx| {
                cx.observe(&st, |_, _, cx| cx.notify()).detach();
                Pill { state: st }
            })
        },
    )
    .ok()
}

fn waveform(levels: &[f32], active: bool, cx: &App) -> gpui::Div {
    let t = Theme::get(cx);
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(3.))
        .h(px(36.))
        .children(levels.iter().map(move |l| {
            let h = if active { 2. + 30. * l.powf(1.4) } else { 2. };
            div().w(px(3.)).h(px(h)).rounded_full().bg(if active { t.text } else { t.text_3 })
        }))
}

impl Render for Pill {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let s = self.state.read(cx);
        let phase = s.dictation.clone();
        let shortcut = s.settings.dictate_shortcut.clone();
        let levels = s.levels.clone();

        let top = match &phase {
            DictationPhase::Listening => waveform(&levels, true, cx).into_any_element(),
            DictationPhase::Transcribing => div()
                .text_size(px(14.))
                .text_color(t.text_2)
                .child("Transcribing…")
                .with_animation(
                    "shimmer",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |el, d| el.opacity(0.45 + 0.55 * (1.0 - (2.0 * d - 1.0).abs())),
                )
                .into_any_element(),
            DictationPhase::Error(msg) => div()
                .px(px(16.))
                .text_size(px(13.))
                .text_color(hsla(t.recording))
                .child(msg.clone())
                .into_any_element(),
            DictationPhase::Idle => waveform(&levels, false, cx).into_any_element(),
        };

        div()
            .size_full()
            .p(px(MARGIN))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .rounded(px(22.))
                    .bg(t.window)
                    .border_1()
                    .border_color(t.hairline)
                    .shadow_lg()
                    .p(px(8.))
                    .child(div().flex_1().flex().items_center().justify_center().child(top))
                    .child(
                        div()
                            .h(px(36.))
                            .px(px(12.))
                            .rounded(px(14.))
                            .bg(t.raised)
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(13.))
                            .text_color(t.text_2)
                            .child(div().flex().items_center().gap(px(8.)).child(rec_dot(cx)).child("Dictation"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(14.))
                                    .child(
                                        div()
                                            .id("stop")
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, _, cx| this.state.read(cx).send(Command::StopDictation)))
                                            .child("Stop")
                                            .child(keycaps(&shortcut, cx)),
                                    )
                                    .child(
                                        div()
                                            .id("cancel")
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, _, cx| this.state.read(cx).send(Command::CancelDictation)))
                                            .child("Cancel")
                                            .child(keycap("esc", cx)),
                                    ),
                            ),
                    ),
            )
    }
}

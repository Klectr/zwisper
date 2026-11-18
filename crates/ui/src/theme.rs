//! Design tokens. Derived from the active GPUI Kit theme (Catppuccin Mocha /
//! Latte, see `state.rs`) so our custom widgets match the kit's components.
//! `dark()`/`light()` are fallbacks (and handy in tests).

use gpui::{hsla, rgb, App, Global, Hsla, Rgba, WindowAppearance};

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub window: Rgba,
    pub raised: Rgba,
    pub inset: Rgba,
    pub hairline: Hsla,
    pub text: Hsla,
    pub text_2: Hsla,
    pub text_3: Hsla,
    pub accent: Rgba,
    pub recording: Rgba,
    pub selection: Hsla,
    pub cursor: Hsla,
    pub speakers: [Rgba; 6],
}

impl Global for Theme {}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            dark: true,
            window: rgb(0x2b2b2b),
            raised: rgb(0x333333),
            inset: rgb(0x3a3a3a),
            hairline: hsla(0., 0., 1., 0.08),
            text: hsla(0., 0., 1., 0.92),
            text_2: hsla(0., 0., 1., 0.60),
            text_3: hsla(0., 0., 1., 0.40),
            accent: rgb(0x8ab4ff),
            recording: rgb(0xff5f57),
            selection: hsla(215. / 360., 0.9, 0.6, 0.35),
            cursor: hsla(215. / 360., 1.0, 0.7, 1.0),
            speakers: [rgb(0xb5e853), rgb(0x6cb6ff), rgb(0xc39bff), rgb(0xffb86c), rgb(0xff8fc6), rgb(0x5fd7c7)],
        }
    }

    pub fn light() -> Self {
        Theme {
            dark: false,
            window: rgb(0xf7f7f5),
            raised: rgb(0xffffff),
            inset: rgb(0xececea),
            hairline: hsla(0., 0., 0., 0.08),
            text: hsla(0., 0., 0., 0.88),
            text_2: hsla(0., 0., 0., 0.56),
            text_3: hsla(0., 0., 0., 0.36),
            accent: rgb(0x1f6feb),
            recording: rgb(0xe5484d),
            selection: hsla(215. / 360., 0.9, 0.55, 0.25),
            cursor: hsla(215. / 360., 1.0, 0.45, 1.0),
            speakers: [rgb(0x4d9a06), rgb(0x1f6feb), rgb(0x8250df), rgb(0xd97706), rgb(0xd6336c), rgb(0x0f9d8a)],
        }
    }

    /// Our palette from a GPUI Kit theme's colours.
    pub fn from_kit(c: &gpui::component::ThemeColor, dark: bool) -> Self {
        let rgba = |h: Hsla| -> Rgba { h.into() };
        Theme {
            dark,
            // Catppuccin: mantle behind, base for cards, surface for controls.
            window: rgba(c.background),
            raised: rgba(if dark { c.popover } else { c.list_even }),
            inset: rgba(c.secondary),
            hairline: c.border,
            text: c.foreground,
            text_2: c.foreground.opacity(0.72),
            text_3: c.muted_foreground,
            accent: rgba(c.primary),
            recording: rgba(c.red),
            selection: c.primary.opacity(0.3),
            cursor: c.primary,
            speakers: [c.green, c.blue, c.magenta, c.yellow, c.red, c.cyan].map(rgba),
        }
    }

    pub fn for_appearance(a: WindowAppearance) -> Self {
        match a {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
            _ => Self::dark(),
        }
    }

    pub fn speaker(&self, color_idx: u8) -> Rgba {
        self.speakers[color_idx as usize % self.speakers.len()]
    }

    pub fn get(cx: &App) -> Theme {
        *cx.global::<Theme>()
    }
}

pub const FONT: &str = if cfg!(target_os = "macos") { ".SystemUIFont" } else { "Inter" };

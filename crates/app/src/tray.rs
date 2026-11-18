//! Menu-bar icon (macOS). Menu events are forwarded onto the GPUI thread.

use gpui::App;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// A 36×36 template image: three rounded bars (a tiny waveform).
fn icon() -> Icon {
    let (w, h) = (36u32, 36u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let bars = [(9u32, 14u32), (17, 26), (25, 18)];
    for (cx, len) in bars {
        let top = (h - len) / 2;
        for y in top..top + len {
            for x in cx - 2..=cx + 2 {
                let i = ((y * w + x) * 4) as usize;
                rgba[i + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, w, h).expect("icon")
}

pub fn install(cx: &mut App) {
    let menu = Menu::new();
    let dictate = MenuItem::new("Dictate", true, None);
    let meeting = MenuItem::new("Start meeting", true, None);
    let library = MenuItem::new("Open Library", true, None);
    let settings = MenuItem::new("Settings…", true, None);
    let quit = MenuItem::new("Quit Zwisper", true, None);
    let _ = menu.append_items(&[
        &dictate,
        &meeting,
        &PredefinedMenuItem::separator(),
        &library,
        &settings,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon())
        .with_icon_as_template(true)
        .with_tooltip("Zwisper")
        .build();
    match tray {
        Ok(t) => {
            Box::leak(Box::new(t));
        }
        Err(e) => {
            tracing::error!(%e, "could not create menu-bar icon");
            return;
        }
    }

    let ids = [dictate.id().clone(), meeting.id().clone(), library.id().clone(), settings.id().clone(), quit.id().clone()];
    let (tx, rx) = async_channel::unbounded::<usize>();
    std::thread::spawn(move || {
        while let Ok(ev) = MenuEvent::receiver().recv() {
            if let Some(i) = ids.iter().position(|id| *id == ev.id) {
                if tx.send_blocking(i).is_err() {
                    break;
                }
            }
        }
    });
    cx.spawn(async move |cx| {
        while let Ok(i) = rx.recv().await {
            let _ = cx.update(|cx| match i {
                0 => zwisper_ui::toggle_dictation(cx),
                1 => zwisper_ui::start_meeting(cx),
                2 => zwisper_ui::open_library(cx),
                3 => zwisper_ui::open_settings(cx),
                _ => cx.dispatch_action(&zwisper_ui::actions::Quit),
            });
        }
    })
    .detach();
}

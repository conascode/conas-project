//! System tray icon and menu.

use crate::{autostart, windows as win};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let new = MenuItem::with_id(app, "new", "새 메모", true, None::<&str>)?;
    let list = MenuItem::with_id(app, "list", "메모 목록", true, None::<&str>)?;
    let auto = CheckMenuItem::with_id(app, "autostart", "컴퓨터 켤 때 자동 실행", true, autostart::is_enabled(), None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&new, &list, &sep1, &auto, &sep2, &quit])?;

    let auto_item = auto.clone();
    let mut builder = TrayIconBuilder::with_id("gaeunmemo")
        .tooltip("GaeunMemo")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, ev| match ev.id().as_ref() {
            "new" => {
                win::new_note(app, None, true);
            }
            "list" => win::open_list(app, true),
            "autostart" => {
                let on = !autostart::is_enabled();
                autostart::set_enabled(on);
                let st = app.state::<crate::AppState>();
                st.with(|d| d.settings.autostart = Some(on));
                st.saver.schedule(std::time::Duration::from_millis(10));
                let _ = auto_item.set_checked(autostart::is_enabled());
            }
            "quit" => crate::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                win::show_all(tray.app_handle(), true);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

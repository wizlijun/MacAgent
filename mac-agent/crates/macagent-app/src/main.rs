//! macagent menu-bar entry point.
//!
//! M0：仅在 macOS 菜单栏显示图标 + Quit 菜单项；不打开任何窗口。
//! M1 起替换为完整的 egui 菜单栏 / 设置 UI。

use anyhow::{Context, Result};
use std::time::Duration;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

#[derive(Debug)]
enum UserEvent {
    MenuEvent(MenuEvent),
}

fn load_icon() -> Result<tray_icon::Icon> {
    let bytes = include_bytes!("../assets/tray-icon.png");
    let img = image::load_from_memory(bytes)
        .context("decode tray-icon.png")?
        .into_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h)
        .context("build tray icon from rgba")
}

fn build_tray() -> Result<(TrayIcon, MenuItem)> {
    let menu = Menu::new();
    let quit_item = MenuItem::new("Quit macagent", true, None);
    menu.append(&quit_item).context("append Quit menu item")?;

    let icon = load_icon()?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip(format!("macagent v{}", macagent_core::version()))
        .build()
        .context("build tray icon")?;

    Ok((tray, quit_item))
}

fn main() -> Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    // 在事件循环跑起来之前装载托盘
    let (_tray, quit_item) = build_tray()?;
    let quit_id = quit_item.id().clone();

    // 让 tray-icon 的菜单事件转发到 tao 的 user event 通道
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |evt| {
        let _ = proxy.send_event(UserEvent::MenuEvent(evt));
    }));

    eprintln!("macagent v{} started; tray icon should be visible", macagent_core::version());

    event_loop.run(move |event, _window_target, control_flow| {
        // 默认让事件循环等待事件，不空转 CPU
        *control_flow = ControlFlow::WaitUntil(std::time::Instant::now() + Duration::from_secs(60));

        if let Event::UserEvent(UserEvent::MenuEvent(evt)) = event {
            if evt.id == quit_id {
                eprintln!("Quit requested");
                *control_flow = ControlFlow::Exit;
            }
        }
    });
}

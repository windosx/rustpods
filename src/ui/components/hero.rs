use crate::core::device::AirPodsDeviceState;
use gtk4::prelude::*;
use gtk4::{Align, Box, Button, Entry, Image, Label, Orientation, Popover};
use std::sync::Arc;

pub fn build_hero_card<F, C>(
    state: &AirPodsDeviceState,
    on_rename: F,
    on_connect: C,
) -> (Box, Label, Label, Label, Button)
where
    F: Fn(String) + Send + Sync + 'static,
    C: Fn() + Send + Sync + 'static,
{
    let container = Box::new(Orientation::Vertical, 12);
    container.add_css_class("card");
    container.set_margin_start(16);
    container.set_margin_end(16);
    container.set_margin_top(16);
    container.set_margin_bottom(8);
    container.set_halign(Align::Fill);

    // Padding inside the card
    let inner_box = Box::new(Orientation::Vertical, 8);
    inner_box.set_margin_top(20);
    inner_box.set_margin_bottom(20);
    inner_box.set_margin_start(20);
    inner_box.set_margin_end(20);

    // AirPods Icon
    let icon_image = Image::from_icon_name("audio-headphones-symbolic");
    icon_image.set_pixel_size(72);
    icon_image.add_css_class("accent");
    inner_box.append(&icon_image);

    // Title Row with Name & Edit Button
    let title_row = Box::new(Orientation::Horizontal, 8);
    title_row.set_halign(Align::Center);

    let title_label = Label::new(Some(&state.name));
    title_label.add_css_class("title-1");
    title_row.append(&title_label);

    let edit_btn = Button::from_icon_name("document-edit-symbolic");
    edit_btn.add_css_class("flat");
    edit_btn.add_css_class("circular");
    edit_btn.set_tooltip_text(Some("重命名 AirPods"));

    // Popover for rename
    let popover = Popover::new();
    let pop_box = Box::new(Orientation::Horizontal, 6);
    pop_box.set_margin_top(8);
    pop_box.set_margin_bottom(8);
    pop_box.set_margin_start(8);
    pop_box.set_margin_end(8);

    let entry = Entry::new();
    entry.set_text(&state.name);
    entry.set_width_chars(20);

    let confirm_btn = Button::with_label("保存");
    confirm_btn.add_css_class("suggested-action");

    let popover_clone = popover.clone();
    let title_label_clone = title_label.clone();
    let on_rename_arc = Arc::new(on_rename);
    let on_rename_entry = on_rename_arc.clone();

    let save_action = {
        let entry = entry.clone();
        let popover = popover_clone.clone();
        let title_label = title_label_clone.clone();
        move || {
            let new_name = entry.text().trim().to_string();
            if !new_name.is_empty() {
                title_label.set_text(&new_name);
                on_rename_entry(new_name);
                popover.popdown();
            }
        }
    };

    let save_clone = save_action.clone();
    confirm_btn.connect_clicked(move |_| save_clone());
    entry.connect_activate(move |_| save_action());

    pop_box.append(&entry);
    pop_box.append(&confirm_btn);
    popover.set_child(Some(&pop_box));
    popover.set_parent(&edit_btn);

    edit_btn.connect_clicked(move |_| {
        popover.popup();
    });

    title_row.append(&edit_btn);
    inner_box.append(&title_row);

    // Subtitle: Model name & MAC address
    let subtitle_text = format!("{} • {}", state.capabilities.name, state.mac_address);
    let subtitle_label = Label::new(Some(&subtitle_text));
    subtitle_label.add_css_class("dim-label");
    subtitle_label.add_css_class("caption");
    inner_box.append(&subtitle_label);

    // Status Pill & Connect Button Row
    let status_row = Box::new(Orientation::Horizontal, 10);
    status_row.set_halign(Align::Center);
    status_row.set_margin_top(4);

    let status_pill = Label::new(if state.is_connected {
        Some("● 已连接")
    } else {
        Some("○ 未连接")
    });
    status_pill.add_css_class(if state.is_connected {
        "success"
    } else {
        "dim-label"
    });
    status_pill.add_css_class("heading");
    status_row.append(&status_pill);

    let connect_btn = Button::with_label("点击连接");
    connect_btn.add_css_class("suggested-action");
    connect_btn.add_css_class("pill");
    connect_btn.set_visible(!state.is_connected);
    let on_connect_arc = Arc::new(on_connect);
    connect_btn.connect_clicked(move |_| {
        on_connect_arc();
    });
    status_row.append(&connect_btn);

    inner_box.append(&status_row);

    container.append(&inner_box);
    (container, title_label, subtitle_label, status_pill, connect_btn)
}

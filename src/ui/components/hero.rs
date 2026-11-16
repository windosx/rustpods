use crate::core::aacp::events::EarStatus;
use crate::ui::state::RustPodsState;
use crate::ui::theme::{control_card, ApplePalette};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, Anchor, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _,
};

pub fn render_hero_card(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    rename_input: &Entity<InputState>,
    p: &ApplePalette,
) -> impl IntoElement {
    let dev = &state.device;
    let both_in_case = !dev.is_wearing_any()
        && (dev.left_in_ear == EarStatus::InCase || dev.right_in_ear == EarStatus::InCase);

    let (status_text, status_color, status_bg) = if !dev.is_connected {
        (format!("○ {}", rust_i18n::t!("hero.disconnected")), p.text_secondary, p.pill_bg)
    } else if both_in_case {
        (format!("● {}", rust_i18n::t!("hero.in_case")), p.accent_blue, p.accent_blue.opacity(0.16))
    } else {
        (format!("● {}", rust_i18n::t!("hero.connected")), p.accent_green, p.accent_green.opacity(0.16))
    };

    let subtitle_text = format!("{} • {}", dev.capabilities.name, dev.mac_address);

    let app_for_rename = app_state.clone();
    let input_for_rename = rename_input.clone();
    let app_for_connect = app_state.clone();
    let input_for_open = rename_input.clone();
    let current_name = dev.name.clone();
    let is_connecting = state.is_connecting;

    control_card(p)
        .py(px(22.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .w(px(76.))
                .h(px(76.))
                .rounded(px(999.))
                .bg(p.accent_blue.opacity(if p.is_dark { 0.16 } else { 0.12 }))
                .border_1()
                .border_color(p.accent_blue.opacity(0.28))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(gpui_kit::assets::IconName::Headphones)
                        .size(px(38.))
                        .text_color(p.accent_blue),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.text_primary)
                        .child(dev.name.clone()),
                )
                .child(
                    Popover::new("hero-rename-popover")
                        .anchor(Anchor::TopCenter)
                        .on_open_change(move |open, window, cx| {
                            if *open {
                                input_for_open.update(cx, |input, cx| {
                                    input.set_value(&current_name, window, cx);
                                });
                            }
                        })
                        .trigger(
                            Button::new("hero-rename-btn")
                                .ghost()
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .tooltip(rust_i18n::t!("hero.rename_btn"))
                                .icon(Icon::new(gpui_kit::assets::IconName::Pencil).size(px(14.))),
                        )
                        .content(move |_popover_state, _window, cx| {
                            let popover_entity = cx.entity().clone();
                            let input_save = input_for_rename.clone();
                            let app_save = app_for_rename.clone();
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.))
                                .child(Input::new(&input_for_rename).w(px(190.)))
                                .child(
                                    Button::new("hero-rename-save")
                                        .primary()
                                        .label(rust_i18n::t!("hero.save"))
                                        .on_click(move |_, window, cx| {
                                            let new_name =
                                                input_save.read(cx).value().trim().to_string();
                                            if !new_name.is_empty() {
                                                app_save.update(cx, |s, cx| {
                                                    s.device.name = new_name.clone();
                                                    s.controller.rename(new_name);
                                                    cx.notify();
                                                });
                                                popover_entity.update(cx, |ps, cx| {
                                                    ps.dismiss(window, cx);
                                                });
                                            }
                                        }),
                                )
                        }),
                ),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(p.text_secondary)
                .child(subtitle_text),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .pt(px(2.))
                .child(
                    div()
                        .px(px(12.))
                        .py(px(4.))
                        .rounded(px(999.))
                        .bg(status_bg)
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(status_color)
                        .child(status_text),
                )
                .when(!dev.is_connected, |this| {
                    this.child(
                        Button::new("hero-connect-btn")
                            .primary()
                            .rounded(ButtonRounded::Size(px(999.)))
                            .loading(is_connecting)
                            .label(if is_connecting {
                                rust_i18n::t!("hero.connecting")
                            } else {
                                rust_i18n::t!("hero.connect_btn")
                            })
                            .on_click(move |_, _, cx| {
                                app_for_connect.update(cx, |s, cx| {
                                    if !s.is_connecting && !s.device.is_connected {
                                        s.is_connecting = true;
                                        s.controller.connect();
                                        cx.notify();
                                    }
                                });
                            }),
                    )
                }),
        )
}

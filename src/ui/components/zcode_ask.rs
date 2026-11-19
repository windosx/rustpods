use crate::system::zcode_ask::{AskProgress, ResolvedNote};
use crate::ui::theme::{control_card, ApplePalette};
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{div, px, FontWeight, IntoElement, ParentElement as _, Styled as _};

pub fn render_zcode_ask_card(
    progress: Option<&AskProgress>,
    resolved: Option<&ResolvedNote>,
    p: &ApplePalette,
) -> Option<impl IntoElement> {
    match (progress, resolved) {
        (Some(ask), _) => {
            if let Some(perm) = &ask.permission {
                let preview_allow = ask.decision_preview == Some(true);
                let preview_deny = ask.decision_preview == Some(false);
                let (status_text, active_color) = if preview_allow {
                    (rust_i18n::t!("agents.nod_allowed").to_string(), p.accent_green)
                } else if preview_deny {
                    (rust_i18n::t!("agents.shake_denied").to_string(), p.accent_red)
                } else {
                    (rust_i18n::t!("agents.waiting_gesture").to_string(), p.accent_blue)
                };

                let card = control_card(p)
                    .border_color(if preview_allow {
                        p.accent_green.opacity(0.55)
                    } else if preview_deny {
                        p.accent_red.opacity(0.55)
                    } else {
                        p.accent_blue.opacity(0.45)
                    })
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        Icon::new(
                                            gpui_kit::assets::IconName::MessageCircleQuestionMark,
                                        )
                                        .size(px(17.))
                                        .text_color(p.accent_blue),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(p.text_primary)
                                            .child(rust_i18n::t!("agents.permission_title", agent = ask.agent.as_str())),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(10.))
                                    .py(px(3.))
                                    .rounded(px(999.))
                                    .bg(active_color.opacity(0.18))
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(active_color)
                                    .child(status_text),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .px(px(7.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .bg(p.accent_blue.opacity(0.18))
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.accent_blue)
                                    .child(perm.tool_name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .text_color(p.text_primary)
                                    .child(ask.question.clone()),
                            ),
                    )
                    .when(!perm.command_or_arg.is_empty(), |el| {
                        el.child(
                            div()
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(6.))
                                .bg(p.card_elevated_bg)
                                .border_1()
                                .border_color(p.card_border)
                                .text_size(px(12.))
                                .text_color(p.text_secondary)
                                .child(perm.command_or_arg.clone()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .px(px(10.))
                                    .py(px(6.))
                                    .rounded(px(8.))
                                    .bg(if preview_allow { p.accent_green.opacity(0.2) } else { p.card_elevated_bg })
                                    .border_1()
                                    .border_color(if preview_allow { p.accent_green } else { p.card_border })
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.accent_green)
                                    .child(rust_i18n::t!("agents.permission_nod_allow")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .px(px(10.))
                                    .py(px(6.))
                                    .rounded(px(8.))
                                    .bg(if preview_deny { p.accent_red.opacity(0.2) } else { p.card_elevated_bg })
                                    .border_1()
                                    .border_color(if preview_deny { p.accent_red } else { p.card_border })
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.accent_red)
                                    .child(rust_i18n::t!("agents.permission_shake_deny")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(p.text_secondary)
                            .child(rust_i18n::t!("agents.permission_footer_hint")),
                    );

                return Some(card);
            }
            None
        }
        (None, Some(resolved_note)) => {
            let is_denied = resolved_note.text.starts_with('✗') || resolved_note.text.contains("拒绝");
            let accent_color = if is_denied {
                p.accent_red
            } else {
                p.accent_green
            };
            let icon_name = if is_denied {
                gpui_kit::assets::IconName::X
            } else {
                gpui_kit::assets::IconName::Check
            };
            let card = control_card(p)
                .border_color(accent_color.opacity(0.45))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Icon::new(icon_name)
                                .size(px(17.))
                                .text_color(accent_color),
                        )
                        .child(
                            div()
                                .text_size(px(14.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_primary)
                                .child(rust_i18n::t!("agents.ask_resolved", agent = resolved_note.agent.as_str())),
                        ),
                )
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(p.text_primary)
                        .child(resolved_note.text.clone()),
                );
            Some(card)
        }
        (None, None) => None,
    }
}

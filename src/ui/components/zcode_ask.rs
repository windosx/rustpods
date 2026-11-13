use crate::system::zcode_ask::AskProgress;
use crate::ui::theme::{control_card, ApplePalette};
use gpui_kit::component::Icon;
use gpui_kit::{div, px, FontWeight, IntoElement, ParentElement as _, Styled as _};

pub fn render_zcode_ask_card(
    progress: Option<&AskProgress>,
    resolved: Option<&str>,
    p: &ApplePalette,
) -> Option<impl IntoElement> {
    match (progress, resolved) {
        (Some(ask), _) => {
            let count_text = if ask.count == 0 {
                rust_i18n::t!("agents.waiting_nod").to_string()
            } else {
                format!("{}: {}", rust_i18n::t!("agents.nod_count"), ask.count)
            };

            let card = control_card(p)
                .border_color(p.accent_blue.opacity(0.45))
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
                                        .child(rust_i18n::t!("agents.ask_title")),
                                ),
                        )
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(3.))
                                .rounded(px(999.))
                                .bg(if ask.count > 0 {
                                    p.accent_blue.opacity(0.18)
                                } else {
                                    p.pill_bg
                                })
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if ask.count > 0 {
                                    p.accent_blue
                                } else {
                                    p.text_secondary
                                })
                                .child(count_text),
                        ),
                )
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(p.text_primary)
                        .child(ask.question.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .children(ask.options.iter().enumerate().map(|(i, label)| {
                            let highlighted = i < ask.count;
                            div()
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(8.))
                                .bg(if highlighted {
                                    p.accent_blue.opacity(0.16)
                                } else {
                                    p.card_elevated_bg
                                })
                                .border_1()
                                .border_color(if highlighted {
                                    p.accent_blue.opacity(0.4)
                                } else {
                                    p.card_border
                                })
                                .text_size(px(13.))
                                .font_weight(if highlighted {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(if highlighted {
                                    p.accent_blue
                                } else {
                                    p.text_primary
                                })
                                .child(format!("{}. {}", i + 1, label))
                        })),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(p.text_secondary)
                        .child("点头 N 次选择第 N 项 · 摇头取消 · 超时自动回退"),
                );

            Some(card)
        }
        (None, Some(resolved_text)) => {
            let card = control_card(p)
                .border_color(p.accent_green.opacity(0.45))
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
                            Icon::new(gpui_kit::assets::IconName::MessageCircleQuestionMark)
                                .size(px(17.))
                                .text_color(p.accent_green),
                        )
                        .child(
                            div()
                                .text_size(px(14.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_primary)
                                .child("ZCode 提问"),
                        ),
                )
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(p.text_primary)
                        .child(resolved_text.to_string()),
                );
            Some(card)
        }
        (None, None) => None,
    }
}

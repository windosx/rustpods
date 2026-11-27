use crate::ui::theme::{
    grouped_card, ApplePalette, CAPTION_SIZE, PAGE_TITLE_SIZE, ROW_DESC_SIZE, ROW_TITLE_SIZE,
    SECTION_TITLE_SIZE, SELECT_W_MD, SLIDER_W,
};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, App, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window,
};

/// Unified page header used across all settings pages.
pub fn page_header(
    p: &ApplePalette,
    title: &str,
    subtitle: Option<&str>,
    trailing: Option<AnyElement>,
) -> gpui_kit::Div {
    let mut left = div().flex().flex_col().gap(px(4.)).flex_1().min_w_0();
    left = left.child(
        div()
            .text_size(px(PAGE_TITLE_SIZE))
            .font_weight(FontWeight::BOLD)
            .text_color(p.text_primary)
            .child(title.to_string()),
    );
    if let Some(sub) = subtitle {
        left = left.child(
            div()
                .text_size(px(ROW_DESC_SIZE))
                .text_color(p.text_secondary)
                .child(sub.to_string()),
        );
    }

    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .gap(px(12.))
        .px(px(4.));

    row = row.child(left);
    if let Some(t) = trailing {
        row = row.child(div().flex_shrink_0().pt(px(2.)).child(t));
    }
    row
}

/// Builder for grouped settings cards that inserts hairlines between rows automatically.
pub struct SettingGroup<'a> {
    p: &'a ApplePalette,
    rows: Vec<AnyElement>,
}

impl<'a> SettingGroup<'a> {
    pub fn new(p: &'a ApplePalette) -> Self {
        Self { p, rows: Vec::new() }
    }

    pub fn row(mut self, elem: impl IntoElement) -> Self {
        self.rows.push(elem.into_any_element());
        self
    }

    pub fn row_if<E: IntoElement>(mut self, cond: bool, make: impl FnOnce() -> E) -> Self {
        if cond {
            self.rows.push(make().into_any_element());
        }
        self
    }

    pub fn build(self) -> gpui_kit::Div {
        let mut card = grouped_card(self.p);
        for (idx, row) in self.rows.into_iter().enumerate() {
            let wrapper = div()
                .w_full()
                .when(idx > 0, |this| {
                    this.border_t_1().border_color(self.p.separator)
                })
                .child(row);
            card = card.child(wrapper);
        }
        card
    }
}

fn row_shell(
    p: &ApplePalette,
    title: &str,
    desc: Option<&str>,
    trailing: impl IntoElement,
    disabled: bool,
) -> gpui_kit::Div {
    let mut left = div().flex().flex_col().gap(px(2.)).flex_1().min_w_0();
    left = left.child(
        div()
            .text_size(px(ROW_TITLE_SIZE))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.text_primary)
            .child(title.to_string()),
    );
    if let Some(d) = desc {
        left = left.child(
            div()
                .text_size(px(ROW_DESC_SIZE))
                .text_color(p.text_secondary)
                .child(d.to_string()),
        );
    }

    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .px(px(18.))
        .py(px(13.))
        .when(!disabled, |this| this.hover(|h| h.bg(p.row_hover)))
        .when(disabled, |this| this.opacity(0.55))
        .child(left)
        .child(div().flex_shrink_0().child(trailing))
}

/// Settings row with a toggle switch on the right.
pub fn toggle_row(
    p: &ApplePalette,
    id: impl Into<gpui_kit::ElementId>,
    title: &str,
    desc: Option<&str>,
    checked: bool,
    disabled: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> gpui_kit::Div {
    let sw = Switch::new(id)
        .checked(checked)
        .disabled(disabled)
        .on_change(on_change);
    row_shell(p, title, desc, sw, disabled)
}

/// Settings row with a dropdown selector on the right.
pub fn select_row(
    p: &ApplePalette,
    title: &str,
    desc: Option<&str>,
    select: &Entity<SelectState<Vec<String>>>,
    width: f32,
    disabled: bool,
) -> gpui_kit::Div {
    let w = if width <= 0. { SELECT_W_MD } else { width };
    let sel = Select::new(select).w(px(w)).disabled(disabled);
    row_shell(p, title, desc, sel, disabled)
}

/// Settings row with an inline value badge and horizontal slider.
pub fn slider_row(
    p: &ApplePalette,
    title: &str,
    desc: Option<&str>,
    value_badge: impl Into<String>,
    slider: &Entity<SliderState>,
) -> gpui_kit::Div {
    let trailing = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .px(px(8.))
                .py(px(3.))
                .rounded(px(6.))
                .bg(p.badge_bg)
                .text_size(px(CAPTION_SIZE))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.text_primary)
                .child(value_badge.into()),
        )
        .child(div().w(px(SLIDER_W)).child(Slider::new(slider).horizontal()));
    row_shell(p, title, desc, trailing, false)
}

/// Settings row with arbitrary action buttons or badges on the right.
pub fn action_row(
    p: &ApplePalette,
    title: &str,
    desc: Option<&str>,
    trailing: impl IntoElement,
) -> gpui_kit::Div {
    row_shell(p, title, desc, trailing, false)
}

/// Danger-highlighted action row used for destructive confirmation prompts.
pub fn danger_action_row(
    p: &ApplePalette,
    title: &str,
    desc: Option<&str>,
    trailing: impl IntoElement,
) -> gpui_kit::Div {
    row_shell(p, title, desc, trailing, false)
        .bg(p.danger_bg)
        .border_t_1()
        .border_color(p.danger_border)
}

/// Read-only key-value row with right-aligned value.
pub fn info_row(p: &ApplePalette, label: &str, value: &str, muted: bool) -> gpui_kit::Div {
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .px(px(18.))
        .py(px(12.))
        .child(
            div()
                .text_size(px(ROW_TITLE_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.text_primary)
                .flex_shrink_0()
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_end()
                .text_size(px(ROW_DESC_SIZE))
                .text_color(if muted { p.text_muted } else { p.text_secondary })
                .child(value.to_string()),
        )
}

/// Pill segmented control shared by Preferences tabs and Theme switcher.
pub fn segmented_control(
    p: &ApplePalette,
    id_prefix: &'static str,
    items: Vec<(&'static str, String, bool)>,
    on_select: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
) -> gpui_kit::Div {
    let mut container = div()
        .flex()
        .flex_row()
        .items_center()
        .p(px(3.))
        .rounded(px(10.))
        .bg(p.pill_bg)
        .border_1()
        .border_color(p.card_border)
        .gap(px(3.));

    for (idx, (item_id, label, active)) in items.into_iter().enumerate() {
        let cb = on_select.clone();
        container = container.child(
            div()
                .id((id_prefix, idx))
                .cursor_pointer()
                .px(px(12.))
                .py(px(5.))
                .rounded(px(7.))
                .text_size(px(ROW_DESC_SIZE))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .bg(if active {
                    p.pill_active_bg
                } else {
                    gpui_kit::transparent_black()
                })
                .text_color(if active {
                    p.sidebar_item_active_fg
                } else {
                    p.text_primary
                })
                .when(!active, |this| this.hover(|h| h.bg(p.row_hover)))
                .on_click(move |_, window, cx| {
                    let _ = item_id;
                    cb(idx, window, cx);
                })
                .child(label),
        );
    }

    container
}

/// Collapsible card section for secondary diagnostics or advanced options.
pub fn collapsible_section(
    p: &ApplePalette,
    id: &'static str,
    title: &str,
    summary: &str,
    expanded: bool,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    content: impl IntoElement,
) -> gpui_kit::Div {
    let chevron = if expanded {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    };

    let header = div()
        .id(id)
        .cursor_pointer()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .px(px(18.))
        .py(px(13.))
        .hover(|h| h.bg(p.row_hover))
        .on_click(move |_, window, cx| on_toggle(window, cx))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .flex_1()
                .min_w_0()
                .child(Icon::new(chevron).size(px(14.)).text_color(p.text_secondary))
                .child(
                    div()
                        .text_size(px(SECTION_TITLE_SIZE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_primary)
                        .child(title.to_string()),
                ),
        )
        .child(
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(999.))
                .bg(p.badge_bg)
                .text_size(px(CAPTION_SIZE))
                .text_color(p.text_secondary)
                .flex_shrink_0()
                .child(summary.to_string()),
        );

    grouped_card(p).child(header).when(expanded, |this| {
        this.child(
            div()
                .w_full()
                .border_t_1()
                .border_color(p.separator)
                .child(content),
        )
    })
}

/// Lightweight inline feedback pill shown in page headers after saving or resetting.
pub fn feedback_banner(p: &ApplePalette, message: &str, is_error: bool) -> gpui_kit::Div {
    let (icon, fg, bg, border) = if is_error {
        (
            IconName::CircleAlert,
            p.accent_red,
            p.danger_bg,
            p.danger_border,
        )
    } else {
        (
            IconName::CircleCheck,
            p.accent_green,
            p.badge_bg,
            p.card_border,
        )
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(4.))
        .rounded(px(999.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .child(Icon::new(icon).size(px(13.)).text_color(fg))
        .child(
            div()
                .text_size(px(CAPTION_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg)
                .child(message.to_string()),
        )
}

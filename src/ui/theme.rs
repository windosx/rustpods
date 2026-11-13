use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::{div, hsla, px, App, Div, Hsla, IntoElement, ParentElement as _, Styled as _};

/// Semantic Apple Control Center tokens derived from the active Light/Dark theme.
#[derive(Clone, Copy)]
pub struct ApplePalette {
    pub is_dark: bool,
    pub window_bg: Hsla,
    pub sidebar_bg: Hsla,
    pub sidebar_border: Hsla,
    pub sidebar_item_hover: Hsla,
    pub sidebar_item_active: Hsla,
    pub sidebar_item_active_fg: Hsla,
    pub card_bg: Hsla,
    pub card_elevated_bg: Hsla,
    pub card_border: Hsla,
    pub separator: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub accent_blue: Hsla,
    pub accent_green: Hsla,
    pub accent_amber: Hsla,
    pub accent_red: Hsla,
    pub track_bg: Hsla,
    pub pill_bg: Hsla,
    pub pill_active_bg: Hsla,
}

pub fn palette(cx: &App) -> ApplePalette {
    if cx.theme().is_dark() {
        ApplePalette {
            is_dark: true,
            window_bg: hsla(228. / 360., 0.16, 0.09, 1.0),
            sidebar_bg: hsla(228. / 360., 0.18, 0.12, 0.92),
            sidebar_border: hsla(0., 0., 1.0, 0.08),
            sidebar_item_hover: hsla(0., 0., 1.0, 0.07),
            sidebar_item_active: hsla(211. / 360., 1.0, 0.52, 1.0),
            sidebar_item_active_fg: hsla(0., 0., 1.0, 1.0),
            card_bg: hsla(226. / 360., 0.16, 0.15, 0.88),
            card_elevated_bg: hsla(226. / 360., 0.16, 0.19, 0.92),
            card_border: hsla(0., 0., 1.0, 0.09),
            separator: hsla(0., 0., 1.0, 0.08),
            text_primary: hsla(0., 0., 0.96, 1.0),
            text_secondary: hsla(220. / 360., 0.10, 0.72, 1.0),
            text_muted: hsla(220. / 360., 0.08, 0.55, 1.0),
            accent_blue: hsla(211. / 360., 1.0, 0.52, 1.0),
            accent_green: hsla(135. / 360., 0.65, 0.51, 1.0),
            accent_amber: hsla(48. / 360., 1.0, 0.52, 1.0),
            accent_red: hsla(3. / 360., 1.0, 0.61, 1.0),
            track_bg: hsla(0., 0., 1.0, 0.12),
            pill_bg: hsla(0., 0., 1.0, 0.08),
            pill_active_bg: hsla(211. / 360., 1.0, 0.52, 1.0),
        }
    } else {
        ApplePalette {
            is_dark: false,
            window_bg: hsla(225. / 360., 0.20, 0.96, 1.0),
            sidebar_bg: hsla(225. / 360., 0.18, 0.93, 0.95),
            sidebar_border: hsla(220. / 360., 0.15, 0.82, 0.7),
            sidebar_item_hover: hsla(220. / 360., 0.18, 0.0, 0.05),
            sidebar_item_active: hsla(211. / 360., 1.0, 0.50, 1.0),
            sidebar_item_active_fg: hsla(0., 0., 1.0, 1.0),
            card_bg: hsla(0., 0., 1.0, 0.94),
            card_elevated_bg: hsla(220. / 360., 0.16, 0.97, 1.0),
            card_border: hsla(220. / 360., 0.16, 0.80, 0.65),
            separator: hsla(220. / 360., 0.15, 0.86, 0.7),
            text_primary: hsla(222. / 360., 0.20, 0.12, 1.0),
            text_secondary: hsla(220. / 360., 0.10, 0.40, 1.0),
            text_muted: hsla(220. / 360., 0.08, 0.54, 1.0),
            accent_blue: hsla(211. / 360., 1.0, 0.50, 1.0),
            accent_green: hsla(135. / 360., 0.62, 0.42, 1.0),
            accent_amber: hsla(38. / 360., 0.95, 0.46, 1.0),
            accent_red: hsla(3. / 360., 0.90, 0.54, 1.0),
            track_bg: hsla(220. / 360., 0.15, 0.50, 0.16),
            pill_bg: hsla(220. / 360., 0.16, 0.50, 0.10),
            pill_active_bg: hsla(211. / 360., 1.0, 0.50, 1.0),
        }
    }
}

/// Applies the Apple Control Center palette onto `gpui-component`'s global `Theme`
/// so built-in `Switch`, `Slider`, `Select`, `Input`, `Popover`, and `Button`
/// harmonize with our custom cards.
pub fn apply_apple_theme(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    Theme::update(cx, |theme| {
        let dark = theme.is_dark();
        theme.radius = px(10.);
        theme.radius_lg = px(14.);

        if dark {
            let bg = hsla(228. / 360., 0.16, 0.09, 1.0);
            let card = hsla(226. / 360., 0.16, 0.15, 1.0);
            let card_alt = hsla(226. / 360., 0.16, 0.21, 1.0);
            let border = hsla(0., 0., 1.0, 0.10);
            let blue = hsla(211. / 360., 1.0, 0.52, 1.0);
            let blue_hover = hsla(211. / 360., 1.0, 0.58, 1.0);
            let blue_active = hsla(211. / 360., 1.0, 0.45, 1.0);
            let green = hsla(135. / 360., 0.65, 0.51, 1.0);

            theme.background = bg;
            theme.foreground = hsla(0., 0., 0.96, 1.0);
            theme.muted = hsla(226. / 360., 0.14, 0.18, 1.0);
            theme.muted_foreground = hsla(220. / 360., 0.10, 0.65, 1.0);
            theme.border = border;
            theme.input = hsla(0., 0., 1.0, 0.14);
            theme.title_bar = hsla(228. / 360., 0.17, 0.11, 1.0);
            theme.title_bar_border = border;
            theme.popover = hsla(226. / 360., 0.18, 0.17, 1.0);
            theme.popover_foreground = hsla(0., 0., 0.96, 1.0);
            theme.primary = blue;
            theme.primary_hover = blue_hover;
            theme.primary_active = blue_active;
            theme.primary_foreground = hsla(0., 0., 1.0, 1.0);
            theme.button_primary = blue;
            theme.button_primary_hover = blue_hover;
            theme.button_primary_active = blue_active;
            theme.button_primary_foreground = hsla(0., 0., 1.0, 1.0);
            theme.secondary = card_alt;
            theme.secondary_hover = hsla(226. / 360., 0.16, 0.26, 1.0);
            theme.secondary_active = hsla(226. / 360., 0.16, 0.18, 1.0);
            theme.secondary_foreground = hsla(0., 0., 0.94, 1.0);
            theme.button_secondary = card_alt;
            theme.button_secondary_hover = hsla(226. / 360., 0.16, 0.26, 1.0);
            theme.button_secondary_active = hsla(226. / 360., 0.16, 0.18, 1.0);
            theme.button_secondary_foreground = hsla(0., 0., 0.94, 1.0);
            theme.switch = hsla(0., 0., 1.0, 0.20);
            theme.switch_thumb = hsla(0., 0., 1.0, 1.0);
            theme.slider_bar = blue;
            theme.slider_thumb = hsla(0., 0., 1.0, 1.0);
            theme.group_box = card;
            theme.group_box_foreground = hsla(0., 0., 0.96, 1.0);
            theme.colors.list = hsla(226. / 360., 0.18, 0.16, 1.0);
            theme.list_hover = hsla(0., 0., 1.0, 0.08);
            theme.list_active = hsla(211. / 360., 1.0, 0.52, 0.22);
            theme.list_active_border = blue;
            theme.success = green;
            theme.ring = blue;
        } else {
            let bg = hsla(225. / 360., 0.20, 0.96, 1.0);
            let card = hsla(0., 0., 1.0, 1.0);
            let border = hsla(220. / 360., 0.16, 0.82, 0.8);
            let blue = hsla(211. / 360., 1.0, 0.50, 1.0);
            let blue_hover = hsla(211. / 360., 1.0, 0.45, 1.0);
            let blue_active = hsla(211. / 360., 1.0, 0.40, 1.0);
            let green = hsla(135. / 360., 0.62, 0.42, 1.0);

            theme.background = bg;
            theme.foreground = hsla(222. / 360., 0.20, 0.12, 1.0);
            theme.muted = hsla(220. / 360., 0.16, 0.92, 1.0);
            theme.muted_foreground = hsla(220. / 360., 0.10, 0.44, 1.0);
            theme.border = border;
            theme.input = hsla(220. / 360., 0.16, 0.80, 1.0);
            theme.title_bar = hsla(225. / 360., 0.18, 0.94, 1.0);
            theme.title_bar_border = border;
            theme.popover = card;
            theme.popover_foreground = hsla(222. / 360., 0.20, 0.12, 1.0);
            theme.primary = blue;
            theme.primary_hover = blue_hover;
            theme.primary_active = blue_active;
            theme.primary_foreground = hsla(0., 0., 1.0, 1.0);
            theme.button_primary = blue;
            theme.button_primary_hover = blue_hover;
            theme.button_primary_active = blue_active;
            theme.button_primary_foreground = hsla(0., 0., 1.0, 1.0);
            theme.secondary = hsla(220. / 360., 0.14, 0.90, 1.0);
            theme.secondary_hover = hsla(220. / 360., 0.14, 0.85, 1.0);
            theme.secondary_active = hsla(220. / 360., 0.14, 0.80, 1.0);
            theme.secondary_foreground = hsla(222. / 360., 0.20, 0.14, 1.0);
            theme.button_secondary = hsla(220. / 360., 0.14, 0.90, 1.0);
            theme.button_secondary_hover = hsla(220. / 360., 0.14, 0.85, 1.0);
            theme.button_secondary_active = hsla(220. / 360., 0.14, 0.80, 1.0);
            theme.button_secondary_foreground = hsla(222. / 360., 0.20, 0.14, 1.0);
            theme.switch = hsla(220. / 360., 0.12, 0.78, 1.0);
            theme.switch_thumb = hsla(0., 0., 1.0, 1.0);
            theme.slider_bar = blue;
            theme.slider_thumb = hsla(0., 0., 1.0, 1.0);
            theme.group_box = card;
            theme.group_box_foreground = hsla(222. / 360., 0.20, 0.12, 1.0);
            theme.colors.list = card;
            theme.list_hover = hsla(220. / 360., 0.16, 0.94, 1.0);
            theme.list_active = hsla(211. / 360., 1.0, 0.50, 0.12);
            theme.list_active_border = blue;
            theme.success = green;
            theme.ring = blue;
        }
    });
}

/// Standard Apple Control Center rounded card container.
pub fn control_card(p: &ApplePalette) -> Div {
    div()
        .w_full()
        .rounded(px(16.))
        .bg(p.card_bg)
        .border_1()
        .border_color(p.card_border)
        .p(px(18.))
}

/// Grouped list card with zero internal padding (rows manage their own padding
/// and hairlines between rows).
pub fn grouped_card(p: &ApplePalette) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(16.))
        .bg(p.card_bg)
        .border_1()
        .border_color(p.card_border)
        .overflow_hidden()
}

/// Single settings row inside a `grouped_card`: title + optional subtitle on the
/// left, trailing control on the right.
pub fn setting_row(
    p: &ApplePalette,
    title: &str,
    subtitle: Option<&str>,
    trailing: impl IntoElement,
    with_top_divider: bool,
) -> Div {
    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .px(px(18.))
        .py(px(13.));

    if with_top_divider {
        row = row.border_t_1().border_color(p.separator);
    }

    let mut left = div().flex().flex_col().gap(px(2.)).flex_1().min_w_0();
    left = left.child(
        div()
            .text_size(px(14.))
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .text_color(p.text_primary)
            .child(title.to_string()),
    );
    if let Some(sub) = subtitle {
        left = left.child(
            div()
                .text_size(px(12.))
                .text_color(p.text_secondary)
                .child(sub.to_string()),
        );
    }

    row.child(left).child(div().flex_shrink_0().child(trailing))
}

/// Section header placed above a `grouped_card`.
pub fn section_header(p: &ApplePalette, title: &str, subtitle: Option<&str>) -> Div {
    let mut box_ = div().flex().flex_col().gap(px(3.)).px(px(4.));
    box_ = box_.child(
        div()
            .text_size(px(15.))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(p.text_primary)
            .child(title.to_string()),
    );
    if let Some(sub) = subtitle {
        box_ = box_.child(
            div()
                .text_size(px(12.))
                .text_color(p.text_secondary)
                .child(sub.to_string()),
        );
    }
    box_
}

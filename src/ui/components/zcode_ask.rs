//! The "ZCode 提问" card on the spatial page.
//!
//! While a ZCode `AskUserQuestion` is pending, this card mirrors what the
//! wearer can answer by nodding: the question, each option with the count
//! reached so far, and a short confirmation once a choice is made. The state
//! is polled from the process-wide [`ASK_HUB`](crate::system::zcode_ask) - the
//! card is a status surface, not the source of truth, so polling at 5 Hz is
//! plenty and keeps the GTK side free of channel plumbing.

use crate::system::zcode_ask::AskProgress;
use gtk4::prelude::*;
use gtk4::{Align, Box, Label, Orientation};
use std::cell::RefCell;
use std::rc::Rc;

/// Widgets the poll loop refreshes.
pub struct ZcodeAskWidgets {
    pub container: Box,
    question_label: Label,
    count_label: Label,
    options_box: Box,
    hint_label: Label,
    state: Rc<RefCell<ZcodeAskViewState>>,
}

#[derive(Debug, Clone, Default)]
struct ZcodeAskViewState {
    progress: Option<AskProgress>,
    resolved: Option<String>,
}

/// Builds the card, hidden until a question arrives.
pub fn build_zcode_ask_card() -> ZcodeAskWidgets {
    let container = Box::new(Orientation::Vertical, 10);
    container.add_css_class("card");
    container.set_margin_start(16);
    container.set_margin_end(16);
    container.set_margin_top(8);
    container.set_margin_bottom(8);
    container.set_visible(false);

    let header_row = Box::new(Orientation::Horizontal, 8);
    let icon = gtk4::Image::from_icon_name("dialog-question-symbolic");
    header_row.append(&icon);
    let title = Label::new(Some("ZCode 提问"));
    title.add_css_class("heading");
    title.set_halign(Align::Start);
    header_row.append(&title);
    container.append(&header_row);

    let question_label = Label::new(None);
    question_label.set_wrap(true);
    question_label.set_xalign(0.0);
    question_label.set_halign(Align::Start);
    container.append(&question_label);

    let count_label = Label::new(None);
    count_label.add_css_class("caption");
    count_label.set_halign(Align::Start);
    count_label.set_visible(false);
    container.append(&count_label);

    let options_box = Box::new(Orientation::Vertical, 4);
    options_box.set_margin_top(4);
    container.append(&options_box);

    let hint_label = Label::new(Some("点头 N 次选择第 N 项 · 摇头取消 · 超时自动回退"));
    hint_label.add_css_class("caption");
    hint_label.add_css_class("dim-label");
    hint_label.set_halign(Align::Start);
    container.append(&hint_label);

    ZcodeAskWidgets {
        container,
        question_label,
        count_label,
        options_box,
        hint_label,
        state: Rc::new(RefCell::new(ZcodeAskViewState::default())),
    }
}

/// Refreshes the card from the latest hub snapshot.
pub fn update_zcode_ask_widgets(
    widgets: &ZcodeAskWidgets,
    progress: Option<AskProgress>,
    resolved: Option<String>,
) {
    // Skip redundant work: the poll fires even when nothing changes.
    {
        let mut state = widgets.state.borrow_mut();
        if state.progress == progress && state.resolved == resolved {
            return;
        }
        state.progress = progress.clone();
        state.resolved = resolved.clone();
    }

    // Rebuild the option list; at card rates this is cheap and keeps the
    // highlight logic trivial.
    while let Some(child) = widgets.options_box.first_child() {
        widgets.options_box.remove(&child);
    }

    match (progress, resolved) {
        (Some(p), _) => {
            widgets.container.set_visible(true);
            widgets.question_label.set_text(&p.question);
            widgets.hint_label.set_visible(true);

            let count_text = if p.count == 0 {
                "等待点头…".to_string()
            } else {
                format!("已点头 {} 次", p.count)
            };
            widgets.count_label.set_text(&count_text);
            widgets.count_label.set_visible(true);

            for (i, label) in p.options.iter().enumerate() {
                let row = Label::new(Some(&format!("{}. {}", i + 1, label)));
                row.set_xalign(0.0);
                row.set_halign(Align::Start);
                if i < p.count {
                    row.add_css_class("accent");
                    row.add_css_class("bold");
                }
                widgets.options_box.append(&row);
            }
        }
        (None, Some(resolved)) => {
            widgets.container.set_visible(true);
            widgets.question_label.set_text(&resolved);
            widgets.count_label.set_visible(false);
            widgets.hint_label.set_visible(false);
        }
        (None, None) => {
            widgets.container.set_visible(false);
        }
    }
}

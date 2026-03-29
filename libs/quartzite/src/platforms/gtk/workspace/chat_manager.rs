use gtk4::prelude::*;
use gtk4::{Box, ScrolledWindow, Orientation, PolicyType, Button, Label, Expander};
use std::cell::RefCell;
use std::rc::Rc;

use async_channel::Sender;

use crate::Event;
use crate::widgets::model::HistoryObject;

pub struct ChatBoxManager {
    pub chat_box: Box,
    pub scrolled_window: ScrolledWindow,
    pub node_count: Rc<RefCell<usize>>,
    decay_timer: Rc<RefCell<Option<gtk4::glib::SourceId>>>,
    is_prepending: Rc<RefCell<bool>>,
    is_fetching: Rc<RefCell<bool>>,
    history_exhausted: Rc<RefCell<bool>>,
    scroll_behavior: crate::tetra::ScrollBehavior,
}

const MAX_UI_NODES: usize = 100;

impl ChatBoxManager {
    pub fn new(tx_event: Sender<Event>, tetra: &crate::tetra::StreamTetra) -> Rc<RefCell<Self>> {
        let chat_box = Box::new(Orientation::Vertical, 0);
        chat_box.set_vexpand(true);
        chat_box.set_hexpand(true);
        chat_box.add_css_class("console");

        let scrolled_window = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&chat_box)
            .build();

        let decay_timer: Rc<RefCell<Option<gtk4::glib::SourceId>>> = Rc::new(RefCell::new(None));
        let node_count = Rc::new(RefCell::new(0));
        let is_prepending = Rc::new(RefCell::new(false));
        let is_fetching = Rc::new(RefCell::new(false));
        let history_exhausted = Rc::new(RefCell::new(false));

        let adj = scrolled_window.vadjustment();

        let tx_clone = tx_event.clone();
        let is_fetching_bind = is_fetching.clone();
        let is_prepending_bind = is_prepending.clone();
        let history_exhausted_bind = history_exhausted.clone();
        let decay_timer_bind = decay_timer.clone();
        let chat_box_bind = chat_box.clone();
        let node_count_bind = node_count.clone();

        let was_at_top = Rc::new(RefCell::new(true));

        adj.connect_value_notify(move |a| {
            let val = a.value();
            let upper = a.upper();
            let page_size = a.page_size();
            let lower = a.lower();

            if upper <= page_size || upper == 0.0 { return; }

            // Decay timer logic
            let is_away_from_top = val > lower + 500.0;
            if is_away_from_top {
                // If not running, start it
                if decay_timer_bind.borrow().is_none() {
                    let decay_box = chat_box_bind.clone();
                    let decay_count = node_count_bind.clone();
                    let decay_timer_inner = decay_timer_bind.clone();
                    let adj_inner = a.clone();

                    let timer = gtk4::glib::timeout_add_local(
                        std::time::Duration::from_secs(600), // 10 minutes
                        move || {
                            // If still away from top after 10 minutes, CULL from top
                            let current_val = adj_inner.value();
                            let current_lower = adj_inner.lower();
                            if current_val > current_lower + 500.0 {
                                let mut count = *decay_count.borrow();
                                while count > MAX_UI_NODES {
                                    if let Some(child) = decay_box.first_child() {
                                        decay_box.remove(&child);
                                        count -= 1;
                                    } else {
                                        break;
                                    }
                                }
                                *decay_count.borrow_mut() = count;
                            }
                            *decay_timer_inner.borrow_mut() = None;
                            gtk4::glib::ControlFlow::Break
                        }
                    );
                    *decay_timer_bind.borrow_mut() = Some(timer);
                }
            } else {
                // We scrolled near top, reset/cancel decay timer
                if let Some(timer) = decay_timer_bind.borrow_mut().take() {
                    timer.remove();
                }
            }
        });

        // The clamp logic based on upper and page_size changes
        let last_upper = Rc::new(RefCell::new(0.0));
        let last_page_size = Rc::new(RefCell::new(0.0));

        let is_prepending_upper = is_prepending.clone();
        let last_upper_ref = last_upper.clone();
        let last_page_size_ref = last_page_size.clone();
        let is_fetching_for_upper = is_fetching.clone();
        let behavior = tetra.scroll_behavior.clone();

        let handle_geometry_change = move |a: &gtk4::Adjustment| {
            let a_clone = a.clone();

            let last_upper_idle = last_upper_ref.clone();
            let last_page_size_idle = last_page_size_ref.clone();
            let is_prepending_idle = is_prepending_upper.clone();
            let fetching_for_idle = is_fetching_for_upper.clone();
            let b = behavior.clone();

            gtk4::glib::idle_add_local(move || {
                let current_upper = a_clone.upper();
                let current_page_size = a_clone.page_size();
                let current_lower = a_clone.lower();
                let current_value = a_clone.value();

                let old_upper = *last_upper_idle.borrow();
                let old_page_size = *last_page_size_idle.borrow();
                let is_prepending = *is_prepending_idle.borrow();
                let fetching = *fetching_for_idle.borrow();

                let delta = current_upper - old_upper;

                *last_upper_idle.borrow_mut() = current_upper;
                *last_page_size_idle.borrow_mut() = current_page_size;

                if current_upper == 0.0 || fetching {
                    return gtk4::glib::ControlFlow::Break;
                }

                let is_at_bottom = current_value >= (old_upper - old_page_size - 1.0);

                if current_upper >= current_lower + current_page_size {
                    if is_at_bottom {
                        match b {
                            crate::tetra::ScrollBehavior::AutoScroll => {
                                a_clone.set_value(current_upper - current_page_size);
                            }
                            crate::tetra::ScrollBehavior::Manual => {}
                        }
                    } else if is_prepending && delta > 0.0 {
                        let max_valid_value = (current_upper - current_page_size).max(current_lower);
                        let target_val = a_clone.value() + delta;
                        a_clone.set_value(target_val.clamp(current_lower, max_valid_value));
                        *is_prepending_idle.borrow_mut() = false;
                    }
                } else {
                    a_clone.set_value(current_lower);
                }
                gtk4::glib::ControlFlow::Break
            });
        };

        let adj2 = scrolled_window.vadjustment();
        let handler_clone1 = handle_geometry_change.clone();
        adj2.connect_upper_notify(move |a| handler_clone1(a));

        let adj3 = scrolled_window.vadjustment();
        adj3.connect_page_size_notify(move |a| handle_geometry_change(a));

        let boot_fetched = Rc::new(RefCell::new(false));
        let tx_clone_load_hist = tx_event.clone();
        let is_fetching_boot = is_fetching.clone();
        let is_prepending_boot = is_prepending.clone();

        let sw_clone = scrolled_window.clone();
        sw_clone.connect_map(move |_| {
            if !*boot_fetched.borrow() {
                *boot_fetched.borrow_mut() = true;
                *is_fetching_boot.borrow_mut() = true;
                *is_prepending_boot.borrow_mut() = true;

                let tx_async = tx_clone_load_hist.clone();
                gtk4::glib::MainContext::default().spawn_local(async move {
                    let _ = tx_async.send(Event::LoadHistory { offset: 0 }).await;
                });
            }
        });

        Rc::new(RefCell::new(Self {
            chat_box,
            scrolled_window,
            node_count,
            decay_timer,
            is_prepending,
            is_fetching,
            history_exhausted,
            scroll_behavior: tetra.scroll_behavior.clone(),
        }))
    }

    pub fn set_fetching(&mut self, state: bool) {
        *self.is_fetching.borrow_mut() = state;
    }

    pub fn set_prepending(&mut self, state: bool) {
        *self.is_prepending.borrow_mut() = state;
    }

    pub fn set_history_exhausted(&mut self, state: bool) {
        *self.history_exhausted.borrow_mut() = state;
    }

    pub fn append_batch(&mut self, messages: Vec<HistoryObject>) {
        for msg in messages {
            let widget = Self::create_message_widget(&msg);
            self.chat_box.append(&widget);
            *self.node_count.borrow_mut() += 1;
        }

        let mut count = *self.node_count.borrow();
        while count > MAX_UI_NODES {
            if let Some(first) = self.chat_box.first_child() {
                self.chat_box.remove(&first);
                count -= 1;
            } else {
                break;
            }
        }
        *self.node_count.borrow_mut() = count;
    }

    pub fn prepend_history(&mut self, messages: Vec<HistoryObject>) {
        // We iterate and render historical messages.
        // Note: prepending works from the bottom up if we iterate forwards and insert at 0
        // Or we iterate backwards. Given HistorySeed provides messages in chronological order or reverse?
        // Wait, normally `messages` are older batch. If they are older, we prepend.
        // If we want oldest at the top, we must iterate backwards and prepend.
        for msg in messages.into_iter().rev() {
            let widget = Self::create_message_widget(&msg);
            self.chat_box.prepend(&widget);
            *self.node_count.borrow_mut() += 1;
        }
        // Do not cull from the bottom for deep history traversal
        /*
        let mut count = *self.node_count.borrow();
        while count > MAX_UI_NODES {
            if let Some(last) = self.chat_box.last_child() {
                self.chat_box.remove(&last);
                count -= 1;
            } else {
                break;
            }
        }
        *self.node_count.borrow_mut() = count;
        */
    }

    pub fn clear(&mut self) {
        while let Some(child) = self.chat_box.first_child() {
            self.chat_box.remove(&child);
        }
        *self.node_count.borrow_mut() = 0;
    }

    fn create_message_widget(obj: &HistoryObject) -> Box {
        let root = Box::new(Orientation::Vertical, 0);
        root.set_halign(gtk4::Align::Fill);
        root.set_hexpand(true);
        root.add_css_class("console-row");

        let bubble = Box::new(Orientation::Vertical, 4);
        bubble.add_css_class("card");
        bubble.add_css_class("bubble-box");
        bubble.set_hexpand(false);
        bubble.set_margin_top(4);
        bubble.set_margin_bottom(4);

        let header_box = Box::new(Orientation::Horizontal, 8);
        let left_expand_btn = Button::builder().icon_name("pan-down-symbolic").css_classes(vec!["flat"]).build();

        let meta_label = Label::builder()
            .xalign(0.0)
            .css_classes(vec!["dim-label"])
            .hexpand(false)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();

        let right_expand_btn = Button::builder().icon_name("pan-down-symbolic").css_classes(vec!["flat"]).build();

        header_box.append(&left_expand_btn);
        header_box.append(&meta_label);
        header_box.append(&right_expand_btn);
        bubble.append(&header_box);

        let msg_label = Label::builder()
            .wrap(true)
            .hexpand(false)
            .max_width_chars(85)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .xalign(0.0)
            .build();
        msg_label.add_css_class("view");

        bubble.append(&msg_label);

        let expander = Expander::new(None);
        expander.set_hexpand(false);

        let payload_content_buffer = gtk4::TextBuffer::new(None);
        let payload_content_view = gtk4::TextView::with_buffer(&payload_content_buffer);
        payload_content_view.set_editable(false);
        payload_content_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        payload_content_view.set_monospace(true);
        payload_content_view.set_cursor_visible(false); // Disable ghost cursor
        payload_content_view.add_css_class("view");
        payload_content_view.set_size_request(-1, 300);

        let payload_scroll = ScrolledWindow::builder()
            .child(&payload_content_view)
            .hexpand(false)
            .build();

        expander.set_child(Some(&payload_scroll));
        bubble.append(&expander);

        root.append(&bubble);

        let is_chat = obj.is_chat();
        let sender = obj.sender();
        let timestamp = obj.timestamp();
        let content = obj.content();
        let subject = obj.subject();

        msg_label.set_visible(false);
        expander.set_visible(false);
        left_expand_btn.set_visible(false);
        right_expand_btn.set_visible(false);

        if is_chat {
            msg_label.set_visible(true);
            meta_label.set_text(&format!("{} • {}", sender, timestamp));

            let is_user = sender == "Architect";

            if is_user {
                bubble.set_halign(gtk4::Align::End);
                bubble.add_css_class("bubble-user");
                meta_label.set_halign(gtk4::Align::End);
                meta_label.set_xalign(1.0);
            } else {
                bubble.set_halign(gtk4::Align::Start);
                bubble.add_css_class("bubble-ai");
                meta_label.set_halign(gtk4::Align::Start);
                meta_label.set_xalign(0.0);
            }

            let explicit_lines = content.trim_end().lines().count();
            let is_long_message = content.len() > 500 || explicit_lines > 7;

            let is_expanded = Rc::new(RefCell::new(false));

            if is_long_message {
                left_expand_btn.set_visible(is_user);
                right_expand_btn.set_visible(!is_user);

                let apply_expansion = {
                    let btn_l = left_expand_btn.clone();
                    let btn_r = right_expand_btn.clone();
                    let msg_lbl = msg_label.clone();
                    let exp = is_expanded.clone();
                    let cont = content.clone();

                    move || {
                        let expanded = *exp.borrow();
                        let icon = if expanded { "pan-up-symbolic" } else { "pan-down-symbolic" };
                        btn_l.set_icon_name(icon);
                        btn_r.set_icon_name(icon);

                        if expanded {
                            msg_lbl.set_label(&cont);
                            msg_lbl.set_selectable(true);
                        } else {
                            let mut byte_idx = 0;
                            let mut line_count = 0;

                            for (idx, c) in cont.char_indices() {
                                if c == '\n' { line_count += 1; }
                                if line_count >= 7 || idx >= 500 {
                                    byte_idx = idx;
                                    break;
                                }
                                byte_idx = idx + c.len_utf8();
                            }

                            let mut truncated = String::with_capacity(550);
                            if byte_idx < cont.len() {
                                truncated.push_str(&cont[..byte_idx]);
                                truncated.push_str("\n...");
                            } else {
                                truncated.push_str(&cont);
                            }

                            msg_lbl.set_label(&truncated);
                            // To prevent selection lockout when collapsed but still provide truncation:
                            msg_lbl.set_selectable(true);
                        }
                    }
                };

                apply_expansion();

                let apply_clone_l = apply_expansion.clone();
                let exp_l = is_expanded.clone();
                left_expand_btn.connect_clicked(move |_| {
                    *exp_l.borrow_mut() = !*exp_l.borrow();
                    apply_clone_l();
                });

                let apply_clone_r = apply_expansion.clone();
                let exp_r = is_expanded.clone();
                right_expand_btn.connect_clicked(move |_| {
                    *exp_r.borrow_mut() = !*exp_r.borrow();
                    apply_clone_r();
                });

            } else {
                msg_label.set_label(&content);
                msg_label.set_selectable(true);
            }

        } else {
            bubble.set_halign(gtk4::Align::Start);
            expander.set_visible(true);
            bubble.add_css_class("una-bubble");
            expander.set_label(Some(&format!("{} | {} | {}", sender, subject, timestamp)));
            payload_content_buffer.set_text(&content);
            expander.set_expanded(false);
        }

        root
    }
}

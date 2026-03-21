fn setup_input_area(tx_event: &Sender<Event>, window: &NativeWindow, active_target: &Rc<RefCell<String>>) -> InputAreaData {
    let input_container = Box::new(Orientation::Horizontal, 8);
    input_container.set_valign(Align::End);
    input_container.set_margin_start(16);
    input_container.set_margin_end(16);
    input_container.set_margin_bottom(16);
    input_container.set_margin_top(16);

    let attach_btn = Button::builder()
        .valign(Align::End)
        .icon_name("share-symbolic")
        .css_classes(vec!["attach-action"])
        .tooltip_text("Attach File")
        .build();
    let tx_clone_file = tx_event.clone();
    let window_clone = window.clone();
    let target_file = active_target.clone();
    attach_btn.connect_clicked(move |_| {
        let tx = tx_clone_file.clone();
        let parent_window = window_clone.clone();
        let target = target_file.clone();
        glib::MainContext::default().spawn_local(async move {
            let dialog = FileDialog::new();
            let result = dialog.open_future(Some(&parent_window)).await;
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    let path_str = path.to_string_lossy().to_string();
                    let _ = tx
                        .send(Event::Input {
                            target: target.borrow().clone(),
                            text: format!("/upload {}", path_str),
                        })
                        .await;
                }
            }
        });
    });

    let input_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .has_frame(false)
        .propagate_natural_height(true)
        .max_content_height(150)
        .build();
    input_scroll.set_hexpand(true);
    let text_view = SourceView::builder()
        .wrap_mode(gtk4::WrapMode::WordChar)
        .show_line_numbers(false)
        .auto_indent(true)
        .accepts_tab(false)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(10)
        .right_margin(10)
        .build();
    enable_spelling(&text_view);
    text_view.add_css_class("view");
    input_scroll.set_child(Some(&text_view));

    let draft_path = gneiss_pal::paths::UnaPaths::root().join(".lumen_draft.txt");
    if let Ok(draft) = std::fs::read_to_string(&draft_path) {
        text_view.buffer().set_text(&draft);
    }
    let pending_save: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let draft_path_clone = draft_path.clone();
    let buffer_for_save = text_view.buffer();
    buffer_for_save.connect_changed(move |buf: &gtk4::TextBuffer| {
        if let Some(source) = pending_save.borrow_mut().take() {
            source.remove();
        }
        let (start, end) = buf.bounds();
        let text = buf.text(&start, &end, false).to_string();
        let path = draft_path_clone.clone();
        let pending_timeout = pending_save.clone();
        *pending_save.borrow_mut() = Some(glib::timeout_add_local(
            std::time::Duration::from_millis(500),
            move || {
                let path_for_task = path.clone();
                let text_for_task = text.clone();
                // Offload disk I/O to the Tokio blocking thread pool to protect the UI render cycle
                tokio::task::spawn_blocking(move || {
                    let _ = std::fs::write(&path_for_task, &text_for_task);
                });
                *pending_timeout.borrow_mut() = None;
                glib::ControlFlow::Break
            },
        ));
    });

    let send_btn = Button::builder()
        .valign(Align::End)
        .icon_name("paper-plane-symbolic")
        .css_classes(vec!["suggested-action"])
        .tooltip_text("Send Message (Ctrl+Enter)")
        .build();
    let tx_clone_send = tx_event.clone();
    let buffer = text_view.buffer();
    let btn_send_clone = send_btn.clone();
    buffer.connect_changed(move |buf: &gtk4::TextBuffer| {
        if buf.line_count() > 1 {
            btn_send_clone.remove_css_class("suggested-action");
        } else {
            btn_send_clone.add_css_class("suggested-action");
        }
    });

    let key_controller = EventControllerKey::new();
    key_controller.set_propagation_phase(PropagationPhase::Capture);
    let tx_clone_key = tx_event.clone();
    let buffer_key = buffer.clone();
    let target_key = active_target.clone();
    let draft_wipe_path1 = draft_path.clone();
    key_controller.connect_key_pressed(move |_ctrl, key, _keycode, state| {
        if key != Key::Return {
            return glib::Propagation::Proceed;
        }
        if state.contains(ModifierType::SHIFT_MASK) {
            return glib::Propagation::Proceed;
        }
        let is_ctrl = state.contains(ModifierType::CONTROL_MASK);
        if is_ctrl || buffer_key.line_count() <= 1 {
            let (start, end) = buffer_key.bounds();
            let text = buffer_key.text(&start, &end, false).to_string();
            if !text.trim().is_empty() {
                let wipe_path = draft_wipe_path1.clone();
                // Offload disk I/O to prevent UI stutter when clearing the draft
                tokio::task::spawn_blocking(move || {
                    let _ = std::fs::remove_file(&wipe_path);
                });

                let tx_async = tx_clone_key.clone();
                let target_val = target_key.borrow().clone();
                glib::MainContext::default().spawn_local(async move {
                    let _ = tx_async
                        .send(Event::Input {
                            target: target_val,
                            text,
                        })
                        .await;
                });
                buffer_key.set_text("");
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    text_view.add_controller(key_controller);

    let target_send = active_target.clone();
    let buffer_send = buffer.clone();
    let draft_wipe_path2 = draft_path.clone();
    send_btn.connect_clicked(move |_| {
        let (start, end) = buffer_send.bounds();
        let text = buffer_send.text(&start, &end, false).to_string();
        if !text.trim().is_empty() {
            let wipe_path = draft_wipe_path2.clone();
            // Offload disk I/O to prevent UI stutter when clearing the draft
            tokio::task::spawn_blocking(move || {
                let _ = std::fs::remove_file(&wipe_path);
            });

            let tx_async = tx_clone_send.clone();
            let target_val = target_send.borrow().clone();
            glib::MainContext::default().spawn_local(async move {
                let _ = tx_async
                    .send(Event::Input {
                        target: target_val,
                        text,
                    })
                    .await;
            });
            buffer_send.set_text("");
        }
    });

    input_container.append(&attach_btn);
    input_container.append(&input_scroll);
    input_container.append(&send_btn);

    let chat_input_buffer = text_view.buffer().downcast::<sourceview5::Buffer>().unwrap();

    InputAreaData {
        input_container,
        chat_input_buffer,
    }
}

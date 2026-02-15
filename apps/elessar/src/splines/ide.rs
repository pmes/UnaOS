use gtk4::prelude::*;
use gtk4::{
    Box, Orientation, Label, Button, Stack, ScrolledWindow,
    Align, ListBox, StackSwitcher,
    TextView, TextBuffer, Paned, Window, Widget, Image, StackTransitionType, CssProvider, StyleContext
};
use sourceview5::prelude::*;
use sourceview5::View as SourceView;
use sourceview5::{Buffer, StyleSchemeManager};
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::thread_local;

use gneiss_pal::types::*;

// --- S40: ELESSAR MUTATION ---

thread_local! {
    static TABULA_BUFFER: RefCell<Option<TextBuffer>> = RefCell::new(None);
}

pub struct IdeSpline {
    // State could go here
}

impl IdeSpline {
    pub fn new() -> Self {
        Self {}
    }

    pub fn bootstrap<W: IsA<Window>>(&self, _window: &W, tx_event: async_channel::Sender<Event>) -> Widget {
        // --- MAIN CONTAINER ---
        let main_box = Box::new(Orientation::Horizontal, 0);

        // --- LEFT: TRINITY SIDEBAR ---
        let sidebar_box = Box::new(Orientation::Vertical, 0);
        sidebar_box.set_width_request(250);
        sidebar_box.add_css_class("sidebar");

        let sidebar_stack = Stack::new();
        sidebar_stack.set_vexpand(true);
        sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);

        // 1. MATRIX (Files)
        let matrix_list = ListBox::new();
        matrix_list.set_selection_mode(gtk4::SelectionMode::None);

        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_file() {
                        if let Some(name) = entry.file_name().to_str() {
                            let row = Box::new(Orientation::Horizontal, 10);
                            row.set_margin_start(10); row.set_margin_end(10);
                            row.set_margin_top(5); row.set_margin_bottom(5);
                            row.append(&Image::from_icon_name("text-x-generic-symbolic"));
                            let label = Label::new(Some(name));
                            label.set_hexpand(true);
                            label.set_xalign(0.0);
                            row.append(&label);
                            matrix_list.append(&row);
                        }
                    }
                }
            }
        }

        let tx_clone_matrix = tx_event.clone();
        matrix_list.connect_row_activated(move |_list, row| {
            if let Some(box_widget) = row.child().and_then(|c| c.downcast::<Box>().ok()) {
                let mut children = box_widget.first_child();
                while let Some(child) = children {
                    if let Some(label) = child.downcast_ref::<Label>() {
                        let text = label.text();
                        let _ = tx_clone_matrix.send_blocking(Event::MatrixFileClick(PathBuf::from(text.as_str())));
                        break;
                    }
                    children = child.next_sibling();
                }
            }
        });

        let matrix_scroll = ScrolledWindow::builder().child(&matrix_list).build();
        sidebar_stack.add_titled(&matrix_scroll, Some("matrix"), "Matrix");

        // 2. VAIRE (Git)
        let vaire_box = Box::new(Orientation::Vertical, 10);
        vaire_box.set_valign(Align::Center);
        vaire_box.append(&Label::new(Some("No Git Repository Detected")));
        sidebar_stack.add_titled(&vaire_box, Some("vaire"), "Vairë");

        // 3. AULE (Forge)
        let aule_box = Box::new(Orientation::Vertical, 10);
        aule_box.set_margin_top(20);

        let ignite_btn = Button::with_label("Ignite");
        ignite_btn.set_icon_name("hammer-symbolic");
        ignite_btn.add_css_class("suggested-action");

        let tx_clone_aule = tx_event.clone();
        ignite_btn.connect_clicked(move |_| {
            let _ = tx_clone_aule.send_blocking(Event::AuleIgnite);
        });

        aule_box.append(&ignite_btn);
        sidebar_stack.add_titled(&aule_box, Some("aule"), "Aulë");

        sidebar_box.append(&sidebar_stack);

        let stack_switcher = StackSwitcher::builder().stack(&sidebar_stack).build();
        let switcher_box = Box::new(Orientation::Horizontal, 0);
        switcher_box.set_halign(Align::Center);
        switcher_box.append(&stack_switcher);
        sidebar_box.append(&switcher_box);

        main_box.append(&sidebar_box);

        // --- RIGHT: WORKSPACE ---
        let paned = Paned::new(Orientation::Vertical);
        paned.set_hexpand(true);
        paned.set_vexpand(true);
        paned.set_position(400);

        // TOP: TABULA (Editor)
        let tabula_scroll = ScrolledWindow::new();
        tabula_scroll.set_vexpand(true);
        let tabula_view = SourceView::builder()
            .monospace(true)
            .show_line_numbers(true)
            .auto_indent(true)
            .build();

        let tb_buffer = tabula_view.buffer().upcast::<TextBuffer>();
        TABULA_BUFFER.with(|b| *b.borrow_mut() = Some(tb_buffer));

        tabula_scroll.set_child(Some(&tabula_view));
        paned.set_start_child(Some(&tabula_scroll));

        // BOTTOM: MIDDEN (Terminal)
        let midden_scroll = ScrolledWindow::new();
        midden_scroll.set_vexpand(true);
        let midden_view = TextView::builder()
            .monospace(true)
            .editable(false)
            .build();
        midden_view.add_css_class("console");

        let midden_buf = midden_view.buffer();
        let midden_adj = midden_scroll.vadjustment();
        let _ = tx_event.send_blocking(Event::TextBufferUpdate(midden_buf, midden_adj));

        midden_scroll.set_child(Some(&midden_view));
        paned.set_end_child(Some(&midden_scroll));

        main_box.append(&paned);

        let provider = CssProvider::new();
        provider.load_from_data("
            .sidebar { background: #1e1e1e; }
            .console { background: #000000; color: #00FF00; }
            textview { font-family: 'Monospace'; font-size: 11pt; }
        ");
        StyleContext::add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        main_box.into()
    }
}

pub fn load_tabula_text(text: &str) {
    TABULA_BUFFER.with(|b| {
        if let Some(buf) = b.borrow().as_ref() {
            buf.set_text(text);
        }
    });
}

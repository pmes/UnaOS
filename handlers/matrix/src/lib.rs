use async_channel::Sender;
use elessar::gneiss_pal::Event;
use gtk4::prelude::*;
use gtk4::{Box, Image, Label, ListBox, Orientation, ScrolledWindow, Widget};
use std::fs;
use std::path::PathBuf;

pub fn create_view(tx: Sender<Event>) -> Widget {
    let matrix_list = ListBox::new();
    matrix_list.set_selection_mode(gtk4::SelectionMode::None);

    if let Ok(entries) = fs::read_dir(".") {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_file() {
                    if let Some(name) = entry.file_name().to_str() {
                        let row = Box::new(Orientation::Horizontal, 10);
                        row.set_margin_start(10);
                        row.set_margin_end(10);
                        row.set_margin_top(5);
                        row.set_margin_bottom(5);
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

    let tx_clone_matrix = tx.clone();
    matrix_list.connect_row_activated(move |_list, row| {
        if let Some(child) = row.child() {
            if let Some(box_widget) = child.downcast_ref::<Box>() {
                let mut children = box_widget.first_child();
                while let Some(c) = children {
                    if let Some(label) = c.downcast_ref::<Label>() {
                        let text = label.text();
                        let _ = tx_clone_matrix
                            .send_blocking(Event::MatrixFileClick(PathBuf::from(text.as_str())));
                        break;
                    }
                    children = c.next_sibling();
                }
            }
        }
    });

    let scroll = ScrolledWindow::builder().child(&matrix_list).build();
    scroll.upcast::<Widget>()
}

use async_channel::Sender;
use elessar::gneiss_pal::Event;
use elessar::{Context, Spline};
use gtk4::prelude::*;
use gtk4::{Box, Image, Label, ListBox, Orientation, ScrolledWindow, Widget};
use std::fs;
use std::path::{Path, PathBuf};

pub struct ProjectView {
    pub root_path: PathBuf,
    pub spline: Spline,
}

impl ProjectView {
    pub fn new(path: &Path) -> Self {
        // 1. DETECT REALITY
        let context = Context::new(path);

        println!("[MATRIX] Loading Project: {:?}", path);
        println!("[MATRIX] Detected Spline: {:?}", context.spline);

        Self {
            root_path: path.to_path_buf(),
            spline: context.spline,
        }
    }

    pub fn get_icon_name(&self) -> &str {
        match self.spline {
            Spline::UnaOS => "computer-symbolic", // The Monolith
            Spline::Rust => "applications-engineering-symbolic", // The Gear
            Spline::Web => "network-server-symbolic", // The Web
            Spline::Python => "media-playlist-shuffle-symbolic", // The Snake (Abstract)
            Spline::Void => "folder-symbolic", // Generic
        }
    }
}

pub fn create_view(tx: Sender<Event>) -> Widget {
    let matrix_list = ListBox::new();
    matrix_list.set_selection_mode(gtk4::SelectionMode::None);

    // Initialize ProjectView to detect Spline
    let project_view = ProjectView::new(Path::new("."));
    // We could use project_view.get_icon_name() to decorate the root if we displayed a root node.
    // For now, it just logs to stdout as requested.

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

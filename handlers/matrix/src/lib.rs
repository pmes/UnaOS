use bandy::{SMessage, MatrixEvent};
use crossbeam_channel::Sender;
use elessar::{Context, Spline};
use gtk4::prelude::*;
use gtk4::{Box, Image, Label, ListBox, Orientation, ScrolledWindow, Widget};
use std::path::{Path, PathBuf};

use crate::MatrixScanner; // The DAG builder we just wrote

pub struct ProjectView {
    pub root_path: PathBuf,
    pub spline: Spline,
}

impl ProjectView {
    pub fn new(path: &Path) -> Self {
        let context = Context::new(path);
        println!("[MATRIX] 👁️ Reality Detected: {:?}", context.spline);
        Self {
            root_path: path.to_path_buf(),
            spline: context.spline,
        }
    }

    pub fn get_icon_name(&self) -> &str {
        match self.spline {
            Spline::UnaOS => "computer-symbolic",
            Spline::Rust => "applications-engineering-symbolic",
            Spline::Web => "network-server-symbolic",
            Spline::Python => "media-playlist-shuffle-symbolic",
            Spline::Void => "folder-symbolic",
        }
    }
}

/// The UI Builder. It takes the Nerve Transmitter and binds it to the GTK event loop.
pub fn create_view(nerve_tx: Sender<SMessage>, root_path: &Path) -> Widget {
    let matrix_list = ListBox::new();
    matrix_list.set_selection_mode(gtk4::SelectionMode::Single);

    let _project_view = ProjectView::new(root_path);

    // 1. BLITZ THE TOPOLOGY
    // Instead of a flat read_dir, we use the Scanner to get the spatial nodes.
    if let Ok(MatrixEvent::IngestTopology { nodes, .. }) = MatrixScanner::map_topology(root_path) {
        // Filter to just the files (modules) for the visual list
        for node in nodes.into_iter().filter(|n| n.kind == "module") {
            let row = Box::new(Orientation::Horizontal, 10);
            row.set_margin_start(10);
            row.set_margin_end(10);
            row.set_margin_top(5);
            row.set_margin_bottom(5);

            row.append(&Image::from_icon_name("text-x-generic-symbolic"));

            let label = Label::new(Some(&node.id));
            label.set_hexpand(true);
            label.set_xalign(0.0);

            // HACK/ELEGANCE: Store the absolute path in the widget's internal name string.
            // This avoids complex GTK subclassing just to hold a PathBuf.
            row.set_widget_name(&node.path.to_string_lossy());

            row.append(&label);
            matrix_list.append(&row);
        }
    }

    // 2. WIRE THE SYNAPSE
    let tx_clone = nerve_tx.clone();
    matrix_list.connect_row_activated(move |_list, row| {
        if let Some(child) = row.child() {
            // Extract the path we hid in the widget name
            let path_str = child.widget_name();
            let path = PathBuf::from(path_str.as_str());

            println!("[MATRIX] ⚡ Firing Synapse: Node Selected -> {:?}", path);

            // Fire the impulse across the OS bus. Una (the IDE) will catch this.
            let _ = tx_clone.send(SMessage::Matrix(MatrixEvent::NodeSelected(path)));
        }
    });

    let scroll = ScrolledWindow::builder().child(&matrix_list).build();
    scroll.upcast::<Widget>()
}

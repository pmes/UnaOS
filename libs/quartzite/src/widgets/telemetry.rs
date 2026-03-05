use gtk4::prelude::*;
use gtk4::{
    gio, glib, Box, Label, ListView, Orientation, PolicyType, ScrolledWindow, SignalListItemFactory,
    SingleSelection, ListItem, Align, Window, TextView, WrapMode,
};
use gtk4::subclass::prelude::ObjectSubclassIsExt; // Required for imp()
use bandy::WeightedSkeleton;
use std::cell::RefCell;

// ==================================================================================
// 1. THE OBJECT WRAPPER (GObject Subclassing)
// ==================================================================================
// GTK4's ListView requires items to be GObjects. Since our `WeightedSkeleton` is a
// pure Rust struct, we must wrap it in a `glib::Object` subclass.
// This is the bridge between the Rust type system and the GObject type system.

mod imp {
    use super::*;
    use glib::subclass::prelude::*;

    /// The internal implementation state of the SkeletonObject.
    /// It holds the actual data in a RefCell because GObjects are shared references.
    #[derive(Default)]
    pub struct SkeletonObject {
        pub data: RefCell<Option<WeightedSkeleton>>,
    }

    /// The ObjectSubclass trait defines the GObject metadata.
    #[glib::object_subclass]
    impl ObjectSubclass for SkeletonObject {
        const NAME: &'static str = "SkeletonObject";
        type Type = super::SkeletonObject;
    }

    /// ObjectImpl is required for all GObjects.
    impl ObjectImpl for SkeletonObject {}
}

glib::wrapper! {
    /// A GObject wrapper around `WeightedSkeleton`.
    /// This allows us to put our skeletons into a `gio::ListStore`.
    pub struct SkeletonObject(ObjectSubclass<imp::SkeletonObject>);
}

impl SkeletonObject {
    /// Create a new SkeletonObject instance from a raw WeightedSkeleton.
    pub fn new(data: WeightedSkeleton) -> Self {
        let obj: Self = glib::Object::builder().build();
        // We inject the data into the internal state immediately after construction.
        *obj.imp().data.borrow_mut() = Some(data);
        obj
    }

    /// Retrieve a clone of the underlying WeightedSkeleton data.
    /// This is cheap because `WeightedSkeleton` holds an `Arc<String>`.
    pub fn skeleton(&self) -> WeightedSkeleton {
        self.imp().data.borrow().as_ref().expect("SkeletonObject data missing").clone()
    }
}

// ==================================================================================
// 2. THE CONTEXT VIEW (The Living HUD)
// ==================================================================================
// This struct manages the GTK widgetry for displaying the telemetry stream.
// It uses a `ListView` for high-performance recycling of widgets.

#[derive(Clone)] // Clone is cheap for GObjects/Widgets (Reference Counted)
pub struct ContextView {
    /// The root widget to be added to the UI hierarchy.
    pub container: ScrolledWindow,
    /// The backing model for the ListView.
    store: gio::ListStore,
}

impl ContextView {
    pub fn new() -> Self {
        // 1. The Data Store
        // We use a ListStore that holds objects of type `SkeletonObject`.
        let store = gio::ListStore::new::<SkeletonObject>();

        // 2. The Selection Model
        // SingleSelection wraps the store and handles selection state.
        // We enable autoselect to always highlight the top item if desired,
        // but for a HUD, maybe we don't force it. Let's keep it standard.
        let selection = SingleSelection::new(Some(store.clone()));

        // 3. The Factory (Widget Recycling)
        // This is the heart of the "Can-Am" performance.
        // Instead of creating new widgets for every data item, we recycle them.
        let factory = SignalListItemFactory::new();

        // SIGNAL: SETUP (Create new widgets)
        // Called when the ListView needs a new row widget.
        factory.connect_setup(move |_factory, item| {
            let list_item = item.downcast_ref::<ListItem>().unwrap();

            // The Row Layout: A vertical box containing Path (Title) and Score (Subtitle).
            let box_container = Box::new(Orientation::Vertical, 4);
            box_container.set_margin_top(8);
            box_container.set_margin_bottom(8);
            box_container.set_margin_start(12);
            box_container.set_margin_end(12);

            let path_label = Label::new(None);
            path_label.set_halign(Align::Start);
            path_label.add_css_class("heading"); // Libadwaita style

            let score_label = Label::new(None);
            score_label.set_halign(Align::Start);
            score_label.add_css_class("caption"); // Libadwaita style
            score_label.set_opacity(0.7);

            box_container.append(&path_label);
            box_container.append(&score_label);

            list_item.set_child(Some(&box_container));
        });

        // SIGNAL: BIND (Bind data to widgets)
        // Called when a recycled widget is assigned to a new data item.
        factory.connect_bind(move |_factory, item| {
            let list_item = item.downcast_ref::<ListItem>().unwrap();
            let widget = list_item.child().and_downcast::<Box>().expect("ListItem child is not a Box");
            let obj = list_item.item().and_downcast::<SkeletonObject>().expect("Item is not SkeletonObject");

            let data = obj.skeleton();

            // Extract labels from the container
            // (Assuming order: Path Label is first, Score Label is second)
            let path_label = widget.first_child().and_downcast::<Label>().expect("First child is not Label");
            let score_label = widget.last_child().and_downcast::<Label>().expect("Last child is not Label");

            // Update UI with Zero-Copy data
            // We only display the filename, not the full path, for brevity.
            let filename = data.path.file_name()
                .unwrap_or_default()
                .to_string_lossy();

            path_label.set_text(&filename);
            score_label.set_text(&format!("Gravity: {:.2}", data.score));

            // --- NEW: ADD TOOLTIP ---
            widget.set_tooltip_text(Some(&data.path.to_string_lossy()));
        });

        // 4. The View
        let list_view = ListView::new(Some(selection), Some(factory));

        // 6. Interaction (Activation - HUD Drilldown)
        // When a user double-clicks or presses Enter on a row, we open a transient window.
        // This allows deep inspection of the skeleton without losing context.
        list_view.connect_activate(move |list_view, position| {
            let model = list_view.model().expect("No model in ListView");
            let selection_model = model.downcast_ref::<SingleSelection>().expect("Not SingleSelection");

            // Get the item at the activated position
            if let Some(obj) = selection_model.item(position).and_downcast::<SkeletonObject>() {
                let data = obj.skeleton();

                // Spawn a transient HUD Window
                let window = Window::builder()
                    // Use deref coercion for Cow<str> to &str
                    .title(&*data.path.to_string_lossy())
                    .default_width(800)
                    .default_height(600)
                    .modal(true)
                    .build();

                if let Some(root) = list_view.root().and_then(|r| r.downcast::<Window>().ok()) {
                    window.set_transient_for(Some(&root));
                }

                let text_view = TextView::builder()
                    .monospace(true)
                    .editable(false)
                    .wrap_mode(WrapMode::WordChar)
                    .left_margin(10)
                    .right_margin(10)
                    .top_margin(10)
                    .bottom_margin(10)
                    .build();

                // Zero-Copy text set (Arc<String> -> &str)
                text_view.buffer().set_text(&data.content);

                let scroll = ScrolledWindow::builder()
                    .child(&text_view)
                    .build();

                window.set_child(Some(&scroll));
                window.present();
            }
        });

        // 5. The Container (ScrolledWindow)
        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .min_content_width(250) // HUD Sidebar width
            .child(&list_view)
            .build();

        Self {
            container: scroll,
            store,
        }
    }

    /// Update the view with a new batch of telemetry data.
    /// This uses `splice` to atomically replace the entire list in one operation.
    /// This ensures zero UI jitter and maximum efficiency.
    pub fn update(&self, skeletons: Vec<WeightedSkeleton>) {
        // Convert Rust structs to GObjects
        let new_items: Vec<SkeletonObject> = skeletons
            .into_iter()
            .map(SkeletonObject::new)
            .collect();

        // ATOMIC SWAP:
        // Remove all items (0..n_items) and insert new_items in their place.
        // This emits a single "items-changed" signal.
        self.store.splice(0, self.store.n_items(), &new_items);
    }
}

use gtk4::prelude::*;
use gtk4::{Application, Widget, Window};
#[cfg(not(feature = "gnome"))]
use gtk4::ApplicationWindow;

#[cfg(feature = "gnome")]
use libadwaita::prelude::*;
#[cfg(feature = "gnome")]
use libadwaita::ApplicationWindow as AdwaitaWindow;

pub enum WindowBackend {
    #[cfg(feature = "gnome")]
    Adwaita(AdwaitaWindow),
    #[cfg(not(feature = "gnome"))]
    Gtk(ApplicationWindow),
}

pub struct UnaWindow {
    inner: WindowBackend,
    #[cfg(feature = "gnome")]
    view: libadwaita::ToolbarView,
}

impl UnaWindow {
    pub fn new(app: &Application) -> Self {
        #[cfg(feature = "gnome")]
        {
            let window = AdwaitaWindow::builder()
                .application(app)
                .default_width(1100)
                .default_height(750)
                .title("Vein")
                .build();

            let view = libadwaita::ToolbarView::new();
            window.set_content(Some(&view));

            Self {
                inner: WindowBackend::Adwaita(window),
                view,
            }
        }
        #[cfg(not(feature = "gnome"))]
        {
            let window = ApplicationWindow::builder()
                .application(app)
                .default_width(1100)
                .default_height(750)
                .title("Vein")
                .build();
            Self {
                inner: WindowBackend::Gtk(window),
            }
        }
    }

    pub fn set_content(&self, content: &impl IsA<Widget>) {
        match &self.inner {
            #[cfg(feature = "gnome")]
            WindowBackend::Adwaita(_) => self.view.set_content(Some(content)),
            #[cfg(not(feature = "gnome"))]
            WindowBackend::Gtk(w) => w.set_child(Some(content)),
        }
    }

    pub fn set_titlebar(&self, titlebar: Option<&impl IsA<Widget>>) {
        match &self.inner {
            #[cfg(feature = "gnome")]
            WindowBackend::Adwaita(_) => {
                if let Some(t) = titlebar {
                    self.view.add_top_bar(t);
                }
            },
            #[cfg(not(feature = "gnome"))]
            WindowBackend::Gtk(w) => w.set_titlebar(titlebar),
        }
    }

    pub fn present(&self) {
        match &self.inner {
            #[cfg(feature = "gnome")]
            WindowBackend::Adwaita(w) => w.present(),
            #[cfg(not(feature = "gnome"))]
            WindowBackend::Gtk(w) => w.present(),
        }
    }

    pub fn downgrade(&self) -> glib::WeakRef<Window> {
        match &self.inner {
            #[cfg(feature = "gnome")]
            WindowBackend::Adwaita(w) => w.upcast_ref::<Window>().downgrade(),
            #[cfg(not(feature = "gnome"))]
            WindowBackend::Gtk(w) => w.upcast_ref::<Window>().downgrade(),
        }
    }
}

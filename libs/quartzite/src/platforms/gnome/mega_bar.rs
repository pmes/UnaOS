use gtk4::prelude::*;
use gtk4::{Box, CssProvider, Orientation, Paned};
use libadwaita::prelude::*;
use libadwaita as adw;

pub struct MegaBar;

impl MegaBar {
    pub fn build(
        _window: &gtk4::ApplicationWindow,
        _title: &str, // Native GNOME handles titles via the window system or we can let it be
        status_widget: &gtk4::Widget,
        left_tabs: &gtk4::Widget,
        right_tabs: &gtk4::Widget,
        left_content: &gtk4::Widget,
        right_content: &gtk4::Widget,
    ) -> gtk4::Widget {
        // 1. Inject CSS
        let provider = CssProvider::new();
        provider.load_from_string(
            "
            /* -- PANED HANDLE FIX -- */
            paned > separator { background-color: transparent; }

            /* -- LEFT: GNOME BUILDER GRAY -- */
            .builder-sidebar, .builder-sidebar:backdrop, .builder-sidebar > background {
                background-color: @headerbar_bg_color; /* Maps exactly to #ebebed / #2e2e32 */
            }
            .builder-sidebar box,
            .builder-sidebar scrolledwindow,
            .builder-sidebar listview,
            .builder-sidebar columnview,
            .builder-sidebar listbox,
            .builder-sidebar row,
            .builder-sidebar tabview,
            .builder-sidebar stack {
                background-color: transparent;
                background-image: none;
                box-shadow: none;
            }

            /* -- RIGHT: GNOME BUILDER WHITE -- */
            .builder-view {
                background-color: @view_bg_color;
                border-left: 1px solid @borders;
            }

            /* -- UNIFIED HEADERS & TABS -- */
            headerbar {
                background: transparent;
                border: none;
                box-shadow: none;
                min-height: 46px;
            }
            tabbar {
                background: transparent;
                border-bottom: 1px solid @borders;
            }

            /* -- GTK FALLBACK MEGA BAR BORDERS -- */
            .title-vbox {
                border-bottom: 1px solid @borders;
            }
            .title-vbox stackswitcher {
                margin-left: 8px; margin-right: 8px; margin-bottom: 4px;
            }
            ",
        );

        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // 2. Build Content Frames with ToolbarViews
        let main_h_paned = Paned::new(Orientation::Horizontal);
        main_h_paned.set_position(260); // Slightly wider sidebar for TeleHUD
        main_h_paned.set_hexpand(true);
        main_h_paned.set_vexpand(true);
        main_h_paned.set_wide_handle(false);
        main_h_paned.set_shrink_start_child(false);
        main_h_paned.set_resize_start_child(false);

        // --- LEFT SIDE ---
        let left_toolbar = adw::ToolbarView::new();
        left_toolbar.set_widget_name("left");
        left_toolbar.add_css_class("builder-sidebar");
        left_toolbar.set_width_request(260);

        // Strip native drop-shadows
        left_toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);

        let left_header = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .build();

        left_toolbar.add_top_bar(&left_header);
        left_toolbar.add_top_bar(left_tabs);

        // Wrap the content so we can apply the class reliably if needed,
        // though ToolbarView with the class handles the background.
        let left_vbox = Box::new(Orientation::Vertical, 0);
        left_vbox.append(left_content);
        left_toolbar.set_content(Some(&left_vbox));

        // --- RIGHT SIDE ---
        let right_toolbar = adw::ToolbarView::new();
        right_toolbar.set_widget_name("right");
        right_toolbar.add_css_class("builder-view");
        right_toolbar.set_hexpand(true);

        // Strip native drop-shadows
        right_toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);

        let right_header = adw::HeaderBar::builder()
            .show_start_title_buttons(false) // Only show the window controls on the far right
            .build();

        // Pack the status widget
        right_header.pack_start(status_widget);

        right_toolbar.add_top_bar(&right_header);
        right_toolbar.add_top_bar(right_tabs);

        let right_vbox = Box::new(Orientation::Vertical, 0);
        right_vbox.append(right_content);
        right_toolbar.set_content(Some(&right_vbox));

        // --- ALIGNMENT FIX FOR GNOME TABS ---
        let tab_size_group = gtk4::SizeGroup::new(gtk4::SizeGroupMode::Vertical);
        tab_size_group.add_widget(left_tabs);
        tab_size_group.add_widget(right_tabs);

        main_h_paned.set_start_child(Some(&left_toolbar));
        main_h_paned.set_end_child(Some(&right_toolbar));

        main_h_paned.upcast::<gtk4::Widget>()
    }
}

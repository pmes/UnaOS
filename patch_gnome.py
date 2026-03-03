import re

with open("libs/quartzite/src/platforms/gtk/spline.rs", "r") as f:
    code = f.read()

# Replace RIGHT TOOLBAR definition and pack start status group
layout_old_gnome_2 = r"""    // 4\. THE WORKSPACE \(Right Pane\)
    let right_toolbar = adw::ToolbarView::new\(\);
    right_toolbar\.set_widget_name\("right"\);

    right_toolbar\.add_css_class\("builder-view"\);

    let command_header_bar = adw::HeaderBar::builder\(\)
        \.show_start_title_buttons\(false\)
        \.build\(\);
    command_header_bar\.set_title_widget\(Some\(&Label::new\(Some\("Lumen"\)\)\)\);
    command_header_bar\.pack_start\(&status_group\);

    // CRITICAL ALIGNMENT FIX: Force left and right headers to be identical heights
    let header_size_group = gtk4::SizeGroup::new\(gtk4::SizeGroupMode::Vertical\);
    header_size_group\.add_widget\(&left_header\);
    header_size_group\.add_widget\(&command_header_bar\);

    let right_tab_view = adw::TabView::new\(\);
    let right_tab_bar = adw::TabBar::new\(\);
    // Mathematically lock the tab bars to the exact same height
    let tab_size_group = gtk4::SizeGroup::new\(gtk4::SizeGroupMode::Vertical\);
    tab_size_group\.add_widget\(&left_tab_bar\);
    tab_size_group\.add_widget\(&right_tab_bar\);

    // Strip native Adwaita toolbar shadows to prevent misalignment
    left_toolbar\.set_top_bar_style\(adw::ToolbarStyle::Flat\);
    right_toolbar\.set_top_bar_style\(adw::ToolbarStyle::Flat\);
    right_tab_bar\.set_view\(Some\(&right_tab_view\)\);"""

layout_new_gnome_2 = r"""    // 4. THE WORKSPACE (Right Pane)
    let right_tab_view = adw::TabView::new();
    let right_tab_bar = adw::TabBar::new();
    right_tab_bar.set_view(Some(&right_tab_view));"""

code = re.sub(layout_old_gnome_2, layout_new_gnome_2, code)

# Replace remaining toolbar usages
old_usages = r"""    right_toolbar\.add_top_bar\(&command_header_bar\);
    right_toolbar\.add_top_bar\(&right_tab_bar\);
    right_toolbar\.set_content\(Some\(&right_tab_view\)\);

    main_h_paned\.set_start_child\(Some\(&left_toolbar\)\);"""

new_usages = r""

code = re.sub(old_usages, new_usages, code)

old_end_child = r"""    main_h_paned\.set_end_child\(Some\(&right_toolbar\)\);

    let left_toolbar_clone = left_toolbar\.clone\(\);

    sidebar_toggle\.connect_toggled\(move \|btn\| \{
        left_toolbar_clone\.set_visible\(btn\.is_active\(\)\);
    \}\);"""

new_end_child = r"""    let left_tab_view_clone = left_tab_view.clone();

    sidebar_toggle.connect_toggled(move |btn| {
        left_tab_view_clone.set_visible(btn.is_active());
    });"""

code = re.sub(old_end_child, new_end_child, code)

old_return = r"""    main_h_paned\.upcast::<Widget>\(\)"""

new_return = r"""    crate::platforms::gnome::mega_bar::MegaBar::build(
        window.upcast_ref::<gtk4::ApplicationWindow>(),
        "Vein (Trinity)",
        &status_group,
        left_tab_bar.upcast_ref::<gtk4::Widget>(),
        right_tab_bar.upcast_ref::<gtk4::Widget>(),
        left_tab_view.upcast_ref::<gtk4::Widget>(),
        right_tab_view.upcast_ref::<gtk4::Widget>(),
    )"""

code = code.replace("    main_h_paned.upcast::<Widget>()", new_return, 1) # Only replace the first occurrence (which is GNOME's)

with open("libs/quartzite/src/platforms/gtk/spline.rs", "w") as f:
    f.write(code)

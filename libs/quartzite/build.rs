// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

fn main() {
    #[cfg(all(not(target_os = "macos"), feature = "gtk"))]
    {
        glib_build_tools::compile_resources(
            &["src/platforms/gtk/assets"],
            "src/platforms/gtk/assets/resources.gresource.xml",
            "quartzite.gresource",
        );
    }

    #[cfg(feature = "qt")]
    {
        unsafe {
            let mut builder = cxx_qt_build::CxxQtBuilder::new()
                .qt_module("Network")
                .qt_module("Quick")
                .file("src/platforms/qt/bridge.rs")
                .file("src/platforms/qt/mod.rs")
                .qrc("src/platforms/qt/assets/qml/qml.qrc");

            builder = builder.cc_builder(|cc| {
                cc.include("src/platforms/qt");

                // Dynamically find Qt Widgets and Quick Widgets include paths via pkg-config
                if let Ok(qt_widgets) = pkg_config::Config::new().probe("Qt6Widgets") {
                    for path in qt_widgets.include_paths {
                        cc.include(path);
                    }
                }

                if let Ok(qt_quick_widgets) = pkg_config::Config::new().probe("Qt6QuickWidgets") {
                    for path in qt_quick_widgets.include_paths {
                        cc.include(path);
                    }
                }

                cc.file("src/platforms/qt/main_window.cpp");
            });

            builder.build();
        }
    }
}
fn main() {
    #[cfg(all(not(target_os = "macos"), feature = "gtk"))]
    {
        glib_build_tools::compile_resources(
            &["src/platforms/gtk/assets"],
            "src/platforms/gtk/assets/resources.gresource.xml",
            "quartzite.gresource",
        );
    }
}
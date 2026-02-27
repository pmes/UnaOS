use std::path::Path;

fn main() {
    #[cfg(all(not(target_os = "macos"), feature = "gtk"))]
    {
        glib_build_tools::compile_resources(
            &["assets"],
            "assets/resources.gresource.xml",
            "quartzite.gresource",
        );
    }
}

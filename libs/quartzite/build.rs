use std::path::Path;

fn main() {
    #[cfg(all(not(target_os = "macos"), feature = "gtk"))]
    {
        glib_build_tools::compile_resources(
            &["src/resources"],
            "src/resources/quartzite.gresource.xml",
            "quartzite.gresource",
        );
    }
}

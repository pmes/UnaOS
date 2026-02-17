fn main() {
    glib_build_tools::compile_resources(
        &["assets"],                        // Source directory
        "assets/resources.gresource.xml",   // Input XML
        "quartzite.gresource",              // Output binary name
    );
}

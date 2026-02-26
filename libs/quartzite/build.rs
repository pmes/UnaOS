fn main() {
    #[cfg(target_os = "linux")]
    glib_build_tools::compile_resources(
        &["assets"],                      // Source directory
        "assets/resources.gresource.xml", // Input XML
        "quartzite.gresource",            // Output binary name
    );

    #[cfg(not(target_os = "linux"))]
    {
        // On non-Linux platforms (macOS/UEFI), we do not compile GResources.
        // We create a dummy file to satisfy any include_bytes! if necessary,
        // although quartzite/lib.rs gates the include_bytes! behind target_os="linux" too.

        // However, build.rs must produce SOMETHING if the crate expects it in OUT_DIR?
        // libs/quartzite/src/lib.rs:
        // #[cfg(not(target_os = "macos"))]
        // const EMBEDDED_RESOURCE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quartzite.gresource"));

        // If target is UEFI ("none"), target_os != macos is TRUE.
        // So UEFI build tries to include "quartzite.gresource".
        // So we MUST create it, even if empty.

        use std::env;
        use std::fs;
        use std::path::Path;

        let out_dir = env::var("OUT_DIR").unwrap();
        let dest_path = Path::new(&out_dir).join("quartzite.gresource");
        fs::write(&dest_path, b"").unwrap();
    }
}

#[cfg(feature = "qt")]
fn main() {
    cxx_qt_build::CxxQtBuilder::new()
        .file("src/shell_qt.rs")
        .build();
}
#[cfg(not(feature = "qt"))]
fn main() {}

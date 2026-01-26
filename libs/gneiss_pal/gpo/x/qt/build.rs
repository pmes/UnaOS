use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("com.template.qt").version(1, 0)
    )
    .qt_module("Network")
    .qt_module("Quick")
    .file("src/bridge.rs")
    .build();
}

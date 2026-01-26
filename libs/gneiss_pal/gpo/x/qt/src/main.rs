mod bridge;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl, QByteArray, QString};

fn main() {
    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    // Load QML
    // In a real app, this would be a resource file or relative path
    // For skeleton, we just set up the engine.

    // We need to register the QML module path if we were loading real QML that uses the bridge.
    if let Some(mut qml_engine) = engine.as_mut() {
        // qml_engine.load(&QUrl::from("qrc:/main.qml"));
        // We don't have a qrc file for the template yet, so we skip loading.
        println!("Qt Engine Initialized. (No QML loaded in skeleton)");
    }

    if let Some(app) = app.as_mut() {
        // app.exec(); // Don't block forever in skeleton test
        println!("Qt App Initialized.");
    }
}

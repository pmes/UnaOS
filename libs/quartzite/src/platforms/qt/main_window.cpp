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

#include "quartzite/src/platforms/qt/mod.cxx.h"
#include <QVBoxLayout>
#include <QWidget>
#include <QQmlEngine>
#include <qqml.h>
#include <QDirIterator>
#include <QDebug>
#include "quartzite/src/platforms/qt/bridge.cxxqt.h"

// Explicitly link the generated cxx_qt plugin block for this crate.
// This prevents the GNU static linker from garbage collecting it
// because it believes the QML modules are unreferenced static objects.
extern "C" void cxx_qt_init_crate_quartzite();

LumenMainWindow::LumenMainWindow(QWidget *parent) : QMainWindow(parent) {
    // Call the generated function to force initialization
    cxx_qt_init_crate_quartzite();

    // Manually register QML types to bypass fragile static QRC plugin loading
    qmlRegisterType<LumenApp>("com.unaos.lumen", 1, 0, "LumenApp");
    qmlRegisterType<HistoryItemQml>("com.unaos.lumen", 1, 0, "HistoryItemQml");
    qmlRegisterType<PreFlightPayloadQml>("com.unaos.lumen", 1, 0, "PreFlightPayloadQml");

    setWindowTitle("Lumen (Qt)");
    resize(1024, 768);

    QWidget* centralWidget = new QWidget(this);
    setCentralWidget(centralWidget);

    QVBoxLayout* layout = new QVBoxLayout(centralWidget);
    layout->setContentsMargins(0, 0, 0, 0);

    m_quickWidget = new QQuickWidget(this);
    m_quickWidget->setResizeMode(QQuickWidget::SizeRootObjectToView);

    // Blanket Import Paths
    m_quickWidget->engine()->addImportPath(QStringLiteral("qrc:/"));
    m_quickWidget->engine()->addImportPath(QStringLiteral("qrc:/qt/qml"));

    // TELEMETRY: Dump Resource System to verify module existence
    qInfo() << "[LUMEN QT] Scanning Resource Tree for CXX-Qt Modules...";
    QDirIterator it(":", QDirIterator::Subdirectories);
    while (it.hasNext()) {
        QString path = it.next();
        if (path.contains("unaos") || path.contains("qmldir")) {
            qInfo() << " >> FOUND RESOURCE:" << path;
        }
    }

    m_quickWidget->setSource(QUrl(QStringLiteral("qrc:/qt/qml/main.qml")));
    layout->addWidget(m_quickWidget);
}

LumenMainWindow::~LumenMainWindow() {
}

std::unique_ptr<LumenMainWindow> create_main_window() {
    return std::make_unique<LumenMainWindow>();
}

void show_main_window(LumenMainWindow& window) {
    window.show();
}

// LumenQApp Implementation
LumenQApp::LumenQApp(int& argc, char** argv) {
    app = new QApplication(argc, argv);
}

LumenQApp::~LumenQApp() {
    delete app;
}

int LumenQApp::exec() {
    return app->exec();
}

static int fake_argc = 1;
static char fake_arg0[] = "lumen";
static char* fake_argv[] = { fake_arg0, nullptr };

std::unique_ptr<LumenQApp> create_qapplication() {
    return std::make_unique<LumenQApp>(fake_argc, fake_argv);
}

int exec_qapplication(LumenQApp& app) {
    return app.exec();
}

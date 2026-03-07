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

LumenMainWindow::LumenMainWindow(QWidget *parent)
    : QMainWindow(parent)
{
    // Configure window
    setWindowTitle("Lumen (Qt)");
    resize(1024, 768);

    // Create central widget
    QWidget* centralWidget = new QWidget(this);
    setCentralWidget(centralWidget);

    // Create layout
    QVBoxLayout* layout = new QVBoxLayout(centralWidget);
    layout->setContentsMargins(0, 0, 0, 0);

    // Initialize QQuickWidget
    m_quickWidget = new QQuickWidget(this);
    m_quickWidget->setResizeMode(QQuickWidget::SizeRootObjectToView);

    // Load QML
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

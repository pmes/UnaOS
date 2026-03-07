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

#pragma once
#include <QGuiApplication>
#include <QMainWindow>
#include <QQuickWidget>
#include "rust/cxx.h"

class LumenMainWindow : public QMainWindow {
public:
    explicit LumenMainWindow(QWidget *parent = nullptr);
    ~LumenMainWindow() override;

private:
    QQuickWidget* m_quickWidget;
};

std::unique_ptr<LumenMainWindow> create_main_window();
void show_main_window(LumenMainWindow& window);

std::unique_ptr<QGuiApplication> create_qapplication();
int exec_qapplication(QGuiApplication& app);

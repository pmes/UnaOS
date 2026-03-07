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

import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import com.unaos.lumen 1.0

Item {
    id: root
    width: 800
    height: 600

    // Core Window Logic & Routing Registration
    LumenWindow {
        id: lumenWindow
        Component.onCompleted: {
            lumenWindow.registerThread();
        }
    }

    // Handler Facade: Vein
    VeinBridge {
        id: veinBridge
        Component.onCompleted: {
            veinBridge.registerThread();
            veinBridge.requestHistory();
        }
    }

    // Models are now uncreatable and injected via the context property from C++ or acquired via singleton

    SplitView {
        anchors.fill: parent
        orientation: Qt.Horizontal

        // Nodes Email List (Sidebar)
        NodesEmail {
            SplitView.preferredWidth: 250
            SplitView.fillHeight: true
            SplitView.minimumWidth: 150

            // Assuming context properties "historyModel" and "preflightPayload" will be injected
            historyModel: typeof _historyModel !== 'undefined' ? _historyModel : null
            backend: veinBridge
        }

        // Nexus Chat (Main Area)
        NexusChat {
            SplitView.fillWidth: true
            SplitView.fillHeight: true

            historyModel: typeof _historyModel !== 'undefined' ? _historyModel : null
            backend: veinBridge
        }
    }
}

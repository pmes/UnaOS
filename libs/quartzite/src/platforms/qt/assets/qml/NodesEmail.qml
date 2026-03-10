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

Rectangle {
    id: root
    color: sys.window

    property var historyModel: null
    property var backend: null

    SystemPalette { id: sys; colorGroup: SystemPalette.Active }

    ListView {
        id: emailListView
        anchors.fill: parent
        model: historyModel
        clip: true

        delegate: ItemDelegate {
            width: emailListView.width
            height: 60
            background: Rectangle {
                color: model.toolTip ? sys.highlight : sys.base
                border.color: sys.mid
                border.width: 1
            }

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 10
                spacing: 4

                Text {
                    text: model.edit !== undefined ? model.edit : ""
                    color: model.toolTip ? sys.highlightedText : sys.text
                    font.bold: true
                    Layout.fillWidth: true
                }
                Text {
                    text: model.display !== undefined ? model.display : ""
                    color: model.toolTip ? sys.highlightedText : sys.windowText
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
            }
        }
    }
}

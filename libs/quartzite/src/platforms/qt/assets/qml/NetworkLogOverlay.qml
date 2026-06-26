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
    opacity: 0.95
    visible: false


    // Prevent clicks from passing through
    MouseArea {
        anchors.fill: parent
        onClicked: {}
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 20
        spacing: 16

        RowLayout {
            Layout.fillWidth: true

            Text {
                text: "NETWORK LOG :: THE TRUTH VIEW"
                font.bold: true
                font.pixelSize: 20
                Layout.fillWidth: true
            }

            Button {
                text: "X"
                implicitWidth: 40
                contentItem: Text {
                    text: parent.text
                    font.bold: true
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                onClicked: {
                    root.visible = false;
                }
            }
        }

        ListView {
            id: networkLogView
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            spacing: 8
            model: typeof _networkLogModel !== "undefined" ? _networkLogModel : null

            delegate: Rectangle {
                width: networkLogView.width
                height: Math.max(logText.implicitHeight + 16, 30)
                border.width: 1
                radius: 4

                TextEdit {
                    id: logText
                    anchors.centerIn: parent
                    width: Math.max(parent.width - 16, 10)
                    text: display !== undefined ? display : "Awaiting transmission..."
                    font.family: "Monospace"
                    wrapMode: TextEdit.WrapAnywhere
                    readOnly: true
                    selectByMouse: true
                    cursorVisible: false
                }
            }

            onCountChanged: {
                networkLogView.positionViewAtEnd()
            }
        }
    }

}

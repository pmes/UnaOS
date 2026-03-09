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

Item {
    id: root
    anchors.fill: parent
    visible: false
    z: 1000

    property string titleText: ""
    property string bodyText: ""
    property var buttons: [] // Array of objects: [{ label: "Cancel", action: "reject", color: "#555" }]
    default property alias customContent: contentArea.data

    signal actionTriggered(string action)

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        onClicked: {}
        onWheel: {}
    }

    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0.6
    }

    Rectangle {
        id: dialogBox
        width: Math.max(380, contentLayout.implicitWidth + 32)
        height: contentLayout.implicitHeight + 32
        anchors.centerIn: parent
        color: "#1e1e1e"
        border.color: "#444444"
        border.width: 1
        radius: 8

        ColumnLayout {
            id: contentLayout
            anchors.fill: parent
            anchors.margins: 16
            spacing: 12

            Text {
                text: root.titleText
                color: "#FFFFFF"
                font.pixelSize: 16
                font.bold: true
                visible: text !== ""
                Layout.fillWidth: true
            }

            Text {
                text: root.bodyText
                color: "#CCCCCC"
                font.pixelSize: 14
                visible: text !== ""
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
            }

            Item {
                id: contentArea
                Layout.fillWidth: true
                Layout.fillHeight: true
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignRight
                spacing: 12

                Repeater {
                    model: root.buttons
                    Button {
                        text: modelData.label
                        background: Rectangle {
                            color: modelData.color ? modelData.color : "#333333"
                            radius: 4
                            implicitWidth: 100
                            implicitHeight: 32
                        }
                        contentItem: Text {
                            text: parent.text
                            color: "#FFFFFF"
                            font.bold: modelData.primary ? true : false
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                        }
                        onClicked: root.actionTriggered(modelData.action)
                    }
                }
            }
        }
    }
}

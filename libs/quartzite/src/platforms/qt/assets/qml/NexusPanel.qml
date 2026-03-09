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

Rectangle {
    id: root
    color: palette.window
    border.color: palette.mid

    property var historyModel: null
    property var backend: null

    // Custom signal to request network log overlay
    signal viewNetworkLog()

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        TabBar {
            id: nexusTabBar
            Layout.fillWidth: true
            background: Rectangle { color: palette.base }

            TabButton {
                text: "nodes"
                width: implicitWidth
                contentItem: Text {
                    text: parent.text
                    color: parent.checked ? palette.text : palette.windowText
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle { color: parent.checked ? palette.mid : "transparent" }
            }
            TabButton {
                text: "nexus"
                width: implicitWidth
                contentItem: Text {
                    text: parent.text
                    color: parent.checked ? palette.text : palette.windowText
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle { color: parent.checked ? palette.mid : "transparent" }
            }
            TabButton {
                text: "teleHUD"
                width: implicitWidth
                contentItem: Text {
                    text: parent.text
                    color: parent.checked ? palette.text : palette.windowText
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle { color: parent.checked ? palette.mid : "transparent" }
            }
        }

        StackLayout {
            id: nexusStackLayout
            currentIndex: nexusTabBar.currentIndex
            Layout.fillWidth: true
            Layout.fillHeight: true

            // Tab 0: nodes (Existing Email View)
            NodesEmail {
                historyModel: root.historyModel
                backend: root.backend
                Layout.fillWidth: true
                Layout.fillHeight: true
            }

            // Tab 1: nexus (Network Log Trigger)
            Rectangle {
                color: palette.window
                Layout.fillWidth: true
                Layout.fillHeight: true

                ColumnLayout {
                    anchors.centerIn: parent
                    spacing: 16

                    Text {
                        text: "NEXUS ROUTING"
                        color: palette.text
                        font.pixelSize: 18
                        font.bold: true
                        Layout.alignment: Qt.AlignHCenter
                    }

                    Button {
                        text: "View Network Log"
                        Layout.alignment: Qt.AlignHCenter
                        background: Rectangle {
                            color: palette.highlight
                            radius: 4
                            implicitWidth: 160
                            implicitHeight: 40
                        }
                        contentItem: Text {
                            text: parent.text
                            color: palette.highlightedText
                            font.bold: true
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                        }
                        onClicked: {
                            root.viewNetworkLog()
                        }
                    }
                }
            }

            // Tab 2: teleHUD (Placeholder)
            Rectangle {
                color: "#1e1e1e"
                Layout.fillWidth: true
                Layout.fillHeight: true

                Text {
                    anchors.centerIn: parent
                    text: "[TeleHUD Context Vector Awaiting Signal]"
                    color: "#888888"
                    font.italic: true
                }
            }
        }
    }
}

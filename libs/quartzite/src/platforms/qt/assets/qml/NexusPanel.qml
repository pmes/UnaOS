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

            TabButton {
                text: "nodes"
                width: implicitWidth
                contentItem: Text {
                    text: parent.text
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
            }
            TabButton {
                text: "nexus"
                width: implicitWidth
                contentItem: Text {
                    text: parent.text
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
            }
            TabButton {
                text: "teleHUD"
                width: implicitWidth
                contentItem: Text {
                    text: parent.text
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
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
                Layout.fillWidth: true
                Layout.fillHeight: true

                ColumnLayout {
                    anchors.centerIn: parent
                    spacing: 16

                    Text {
                        text: "NEXUS ROUTING"
                        font.pixelSize: 18
                        font.bold: true
                        Layout.alignment: Qt.AlignHCenter
                    }

                    Button {
                        text: "View Network Log"
                        Layout.alignment: Qt.AlignHCenter
                        background: Rectangle {
                            radius: 4
                            implicitWidth: 160
                            implicitHeight: 40
                        }
                        contentItem: Text {
                            text: parent.text
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

            // Tab 2: teleHUD (Matrix Tetra)
            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 12
                    spacing: 12

                    Text {
                        text: "MATRIX TETRA"
                        font.pixelSize: 14
                        font.bold: true
                        color: "#888888"
                    }

                    ListView {
                        id: matrixListView
                        Layout.fillWidth: true
                        Layout.preferredHeight: 150
                        model: _matrixModel
                        clip: true

                        delegate: Rectangle {
                            width: ListView.view.width
                            height: 30
                            color: "transparent"

                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                anchors.left: parent.left
                                anchors.leftMargin: 10
                                text: display
                                color: "white"
                            }

                            MouseArea {
                                anchors.fill: parent
                                onClicked: {
                                    _matrixModel.toggleNode(idRole)
                                }
                            }
                        }
                    }

                    Text {
                        text: "CONTEXT VECTOR"
                        font.pixelSize: 14
                        font.bold: true
                        color: "#888888"
                        Layout.topMargin: 20
                    }

                    Text {
                        text: "[TeleHUD Context Vector Awaiting Signal]"
                        font.italic: true
                        color: "#666666"
                        Layout.alignment: Qt.AlignHCenter
                    }
                }
            }
        }
    }
}

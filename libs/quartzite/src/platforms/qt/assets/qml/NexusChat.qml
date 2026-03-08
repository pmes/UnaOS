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
    color: "#121212"

    VeinBridge {
        id: veinEngine
        Component.onCompleted: {
            veinEngine.registerThread();
            veinEngine.requestHistory();
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 10

        // Chat / Message View natively scrolls via Flickable
        ListView {
            id: chatListView
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: typeof _historyModel !== "undefined" ? _historyModel : null
            spacing: 8
            delegate: Rectangle {
                width: Math.max(chatListView.width, 100)
                height: Math.max(messageText.implicitHeight + 24, 40)
                color: model.toolTip ? "#0078D7" : "#333333"
                radius: 8

                Text {
                    id: messageText
                    anchors.centerIn: parent
                    width: Math.max(parent.width - 16, 10)
                    text: display !== undefined ? display : (model.display !== undefined ? model.display : "Awaiting Telemetry...")
                    color: "#FFFFFF"
                    wrapMode: Text.WordWrap
                }
            }

            onCountChanged: {
                // Auto-scroll to bottom on new messages
                chatListView.positionViewAtEnd()
            }
        }

        // Input Area
        RowLayout {
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignBottom

            TextField {
                id: inputField
                Layout.fillWidth: true
                placeholderText: "Type a message..."
                color: "white"
                background: Rectangle {
                    color: "#333333"
                    radius: 4
                }
                onAccepted: {
                    if (inputField.text !== "") {
                        veinEngine.sendMessage(inputField.text);
                        inputField.text = "";
                    }
                }
            }

            Button {
                text: "Send"
                onClicked: {
                    if (inputField.text !== "") {
                        veinEngine.sendMessage(inputField.text);
                        inputField.text = "";
                    }
                }
            }
        }
    }
}

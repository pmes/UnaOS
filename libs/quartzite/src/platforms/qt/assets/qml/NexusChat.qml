// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import com.unaos.lumen 1.0

Rectangle {
    id: root
    color: "#121212"

    property var historyModel: null
    property var backend: null

    Component.onCompleted: {
        if (historyModel) {
            historyModel.registerModelThread();
        }
        if (backend) {
            backend.registerThread();
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
            model: historyModel ? historyModel : null
            spacing: 8
            delegate: Rectangle {
                width: chatListView.width
                height: Math.max(messageText.implicitHeight + 24, 40)
                color: model.toolTip ? "#0078D7" : "#333333"
                radius: 8

                Text {
                    id: messageText
                    anchors.centerIn: parent
                    width: parent.width - 16
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
                    if (backend && inputField.text !== "") {
                        backend.sendMessage(inputField.text);
                        inputField.text = "";
                    }
                }
            }

            Button {
                text: "Send"
                onClicked: {
                    if (backend && inputField.text !== "") {
                        backend.sendMessage(inputField.text);
                        inputField.text = "";
                    }
                }
            }
        }
    }
}

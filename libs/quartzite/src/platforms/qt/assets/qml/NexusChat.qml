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


    property var backend: null

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

            // Fluid Geometry Constraints
            property int fluidThreshold: 600
            property bool isWideMode: width > fluidThreshold

            delegate: Item {
                // Total width is full list view, height is calculated text height
                width: chatListView.width
                height: Math.max(messageText.implicitHeight + 24, 40)

                // Outer container for positioning (Staggered vs Inline)
                Rectangle {
                    id: bubble
                    // Fluid Math:
                    // If Wide Mode -> bubbles take 70% max width, align L/R.
                    // If Narrow Mode -> bubbles take full width, stacked.
                    width: chatListView.isWideMode ? Math.min(parent.width * 0.7, messageText.implicitWidth + 32) : parent.width
                    height: parent.height

                    // Anchoring logic for stagger
                    anchors.left: chatListView.isWideMode && !model.toolTip ? parent.left : undefined
                    anchors.right: chatListView.isWideMode && model.toolTip ? parent.right : undefined
                    // Fallback to center if not staggered (though full width means it covers everything anyway)

                    border.width: 1
                    radius: 8

                    Text {
                        id: messageText
                        anchors.centerIn: parent
                        width: Math.max(parent.width - 32, 10)
                        text: display !== undefined ? display : (model.display !== undefined ? model.display : "Awaiting Telemetry...")
                        wrapMode: Text.WordWrap
                    }
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
                background: Rectangle {
                    radius: 4
                }
                onAccepted: {
                    if (backend && inputField.text !== "") {
                        backend.requestPreFlightReview(inputField.text);
                    }
                }
            }

            Button {
                text: "Pre-Flight"
                onClicked: {
                    if (backend) {
                        if (inputField.text !== "") {
                            backend.requestPreFlightReview(inputField.text);
                        } else {
                            backend.requestPreFlightReview("");
                        }
                    }
                }
            }
        }
    }

    // Embed the temporary Pre-Flight Overlay here to cover the chat completely
    PreFlightOverlay {
        id: preFlightOverlay
        anchors.fill: parent
        z: 90
        backend: root.backend

        onPayloadSent: {
            inputField.text = "";
        }

        onPayloadCanceled: {
            inputField.text = "";
            if (backend) {
                backend.cancelPreFlight();
            }
        }
    }

    Connections {
        target: root.backend
        function onNetworkPayloadDispatched(payload) {
            if (typeof _networkLogModel !== "undefined" && _networkLogModel !== null) {
                _networkLogModel.appendLog(payload);
            }
        }
        function onPayloadReadyForReview(system, directives, engrams, prompt) {
            preFlightOverlay.systemTextAreaText = system.toString();
            preFlightOverlay.directivesTextAreaText = directives.toString();
            preFlightOverlay.engramsTextAreaText = engrams.toString();
            preFlightOverlay.promptTextAreaText = prompt.toString();
            preFlightOverlay.visible = true;
        }
    }
}

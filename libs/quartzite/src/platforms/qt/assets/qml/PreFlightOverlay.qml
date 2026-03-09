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
    visible: false
    opacity: 0.98


    property var backend: null
    property alias systemTextAreaText: systemTextArea.text
    property alias directivesTextAreaText: directivesTextArea.text
    property alias engramsTextAreaText: engramsTextArea.text
    property alias promptTextAreaText: promptTextArea.text

    // Custom signals to tell the parent to clear its input state
    signal payloadSent()
    signal payloadCanceled()

    // Prevent clicks from passing through
    MouseArea {
        anchors.fill: parent
        onClicked: {}
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 16
        spacing: 12

        Text {
            text: "PRE-FLIGHT REVIEW"
            font.pixelSize: 22
            font.bold: true
            Layout.alignment: Qt.AlignHCenter
        }

        TabBar {
            id: preflightTabBar
            Layout.fillWidth: true

            TabButton {
                text: "System"
                contentItem: Text {
                    text: parent.text
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                }
            }
            TabButton {
                text: "Directives"
                contentItem: Text {
                    text: parent.text
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                }
            }
            TabButton {
                text: "Engrams"
                contentItem: Text {
                    text: parent.text
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                }
            }
            TabButton {
                text: "Prompt"
                contentItem: Text {
                    text: parent.text
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                }
            }
        }

        StackLayout {
            id: preflightStack
            currentIndex: preflightTabBar.currentIndex
            Layout.fillWidth: true
            Layout.fillHeight: true

            // System Tab
            Rectangle {
                ScrollView {
                    anchors.fill: parent
                    anchors.margins: 8
                    TextArea {
                        id: systemTextArea
                        wrapMode: Text.WordWrap
                        background: Item {}
                    }
                }
            }

            // Directives Tab
            Rectangle {
                ScrollView {
                    anchors.fill: parent
                    anchors.margins: 8
                    TextArea {
                        id: directivesTextArea
                        wrapMode: Text.WordWrap
                        background: Item {}
                    }
                }
            }

            // Engrams Tab
            Rectangle {
                ScrollView {
                    anchors.fill: parent
                    anchors.margins: 8
                    TextArea {
                        id: engramsTextArea
                        wrapMode: Text.WordWrap
                        background: Item {}
                    }
                }
            }

            // Prompt Tab
            Rectangle {
                ScrollView {
                    anchors.fill: parent
                    anchors.margins: 8
                    TextArea {
                        id: promptTextArea
                        wrapMode: Text.WordWrap
                        background: Item {}
                    }
                }
            }
        } // Close StackLayout

        // Action Buttons
        RowLayout {
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignRight
            spacing: 16

            Button {
                text: "Cancel"
                background: Rectangle { color: "#D70000"; radius: 4; implicitWidth: 100; implicitHeight: 36 }
                contentItem: Text { text: parent.text; color: "#FFFFFF"; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                onClicked: {
                    customCancelAlert.open();
                }
            }

            Button {
                text: "Send"
                onClicked: {
                    if (backend && promptTextArea.text !== "") {
                        backend.dispatchPayload(
                            systemTextArea.text,
                            directivesTextArea.text,
                            engramsTextArea.text,
                            promptTextArea.text
                        );
                        root.payloadSent();
                        root.visible = false;
                    }
                }
            }
        }
    }

    UnaDialog {
        id: customCancelAlert
        parent: Overlay.overlay
        titleText: "Cancel Pre-Flight?"
        bodyText: "Are you sure you want to abort the payload?\nThis will clear your current input."
        buttons: [
            { label: "No, Return", action: "return" },
            { label: "Yes, Abort", action: "reject" }
        ]

        onActionTriggered: function(action) {
            if (action === "reject") {
                customCancelAlert.close();
                // Clear the exact TextAreas by ID
                systemTextArea.text = "";
                directivesTextArea.text = "";
                engramsTextArea.text = "";
                promptTextArea.text = "";

                root.payloadCanceled();
                root.visible = false;
            } else if (action === "return") {
                customCancelAlert.close();
            }
        }
    }
}

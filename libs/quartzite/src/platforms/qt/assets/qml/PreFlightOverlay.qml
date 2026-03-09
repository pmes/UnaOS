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
            color: "#FFFFFF"
            font.pixelSize: 22
            font.bold: true
            Layout.alignment: Qt.AlignHCenter
        }

        TabBar {
            id: preflightTabBar
            Layout.fillWidth: true
            background: Rectangle { color: "#1e1e1e" }

            TabButton {
                text: "System"
                contentItem: Text {
                    text: parent.text
                    color: parent.checked ? "#FFFFFF" : "#888888"
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: parent.checked ? "#333333" : "transparent"
                }
            }
            TabButton {
                text: "Directives"
                contentItem: Text {
                    text: parent.text
                    color: parent.checked ? "#FFFFFF" : "#888888"
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: parent.checked ? "#333333" : "transparent"
                }
            }
            TabButton {
                text: "Engrams"
                contentItem: Text {
                    text: parent.text
                    color: parent.checked ? "#FFFFFF" : "#888888"
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: parent.checked ? "#333333" : "transparent"
                }
            }
            TabButton {
                text: "Prompt"
                contentItem: Text {
                    text: parent.text
                    color: parent.checked ? "#FFFFFF" : "#888888"
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: parent.checked ? "#333333" : "transparent"
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
                color: "#1e1e1e"
                border.color: "#333"
                ScrollView {
                    anchors.fill: parent
                    anchors.margins: 8
                    TextArea {
                        id: systemTextArea
                        color: "#FFFFFF"
                        wrapMode: Text.WordWrap
                    }
                }
            }

            // Directives Tab
            Rectangle {
                color: "#1e1e1e"
                border.color: "#333"
                ScrollView {
                    anchors.fill: parent
                    anchors.margins: 8
                    TextArea {
                        id: directivesTextArea
                        color: "#FFFFFF"
                        wrapMode: Text.WordWrap
                    }
                }
            }

            // Engrams Tab
            Rectangle {
                color: "#1e1e1e"
                border.color: "#333"
                ScrollView {
                    anchors.fill: parent
                    anchors.margins: 8
                    TextArea {
                        id: engramsTextArea
                        color: "#FFFFFF"
                        wrapMode: Text.WordWrap
                    }
                }
            }

            // Prompt Tab
            Rectangle {
                color: "#1e1e1e"
                border.color: "#333"
                ScrollView {
                    anchors.fill: parent
                    anchors.margins: 8
                    TextArea {
                        id: promptTextArea
                        color: "#FFFFFF"
                        wrapMode: Text.WordWrap
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
                background: Rectangle { color: "#555555"; radius: 4; implicitWidth: 100; implicitHeight: 36 }
                contentItem: Text { text: parent.text; color: "#FFFFFF"; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                onClicked: {
                    cancelDialog.open()
                }
            }

            Button {
                text: "Send"
                background: Rectangle { color: "#0078D7"; radius: 4; implicitWidth: 100; implicitHeight: 36 }
                contentItem: Text { text: parent.text; color: "#FFFFFF"; font.bold: true; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
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

    // Use QtQuick.Controls Dialog instead of Qt.labs.platform to ensure
    // the dialog anchors within the transient application window and properly
    // blocks the parent UI (modal: true). Qt.labs.platform defaults to unparented
    // native Wayland/Windows windows which can center arbitrarily.
    Dialog {
        id: cancelDialog
        title: "Cancel Pre-Flight?"
        standardButtons: Dialog.Yes | Dialog.No
        anchors.centerIn: parent
        modal: true

        background: Rectangle { color: "#1e1e1e"; border.color: "#444"; radius: 6 }

        contentItem: Text {
            text: "Are you sure you want to abort the payload?\nThis will clear your current input."
            color: "#FFFFFF"
        }

        onAccepted: {
            root.payloadCanceled();
            root.visible = false;
        }
    }
}

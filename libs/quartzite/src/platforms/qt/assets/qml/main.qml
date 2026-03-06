import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import RustQt 1.0

Item {
    id: root
    width: 800
    height: 600

    // Access the Rust-backed LumenApp Object
    // Assume it is registered as an attached property or injected context property
    // For CXX-Qt, objects can be instantiated directly if they are registered:
    LumenApp {
        id: lumenApp

        // This simulates reacting to the history changes
        onHistoryChanged: {
             console.log("History updated.");
        }
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0

        // Nexus Sidebar Stub (Widgets usually handle this, but adding a visual placeholder)
        Rectangle {
            Layout.preferredWidth: 250
            Layout.fillHeight: true
            color: "#1e1e1e"

            ListView {
                anchors.fill: parent
                model: lumenApp.history
                delegate: ItemDelegate {
                    width: parent.width
                    text: modelData.sender + ": " + modelData.content
                    font.pixelSize: 14
                    background: Rectangle {
                        color: modelData.is_chat ? "#2d2d30" : "#1e1e1e"
                    }
                }
            }
        }

        // Main Content Area
        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            color: "#252526"

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 10

                // Chat / Message View
                ScrollView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true

                    ListView {
                        id: chatListView
                        model: lumenApp.history
                        spacing: 8
                        delegate: Rectangle {
                            width: chatListView.width
                            height: messageText.implicitHeight + 20
                            color: modelData.is_chat ? "#0078D7" : "#333333"
                            radius: 8

                            Text {
                                id: messageText
                                anchors.centerIn: parent
                                width: parent.width - 20
                                text: modelData.content
                                color: "white"
                                wrapMode: Text.WordWrap
                            }
                        }
                    }
                }

                // Input Area
                RowLayout {
                    Layout.fillWidth: true

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
                            lumenApp.send_message(inputField.text);
                            inputField.text = "";
                        }
                    }

                    Button {
                        text: "Send"
                        onClicked: {
                            lumenApp.send_message(inputField.text);
                            inputField.text = "";
                        }
                    }
                }
            }
        }
    }
}

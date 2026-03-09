import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

Popup {
    id: root
    anchors.centerIn: parent
    modal: true
    closePolicy: Popup.NoAutoClose

    // RESTORED PROPERTIES (CRITICAL FOR COMPILATION)
    property string titleText: ""
    property string bodyText: ""
    property var buttons: []
    default property alias customContent: contentArea.data

    signal actionTriggered(string action)

    SystemPalette { id: sys; colorGroup: SystemPalette.Active }

    background: Rectangle {
        color: sys.window
        border.color: sys.mid
        border.width: 1
        radius: 8
    }

    contentItem: ColumnLayout {
        id: contentLayout
        spacing: 12

        Text {
            text: root.titleText
            color: sys.text
            font.pixelSize: 16
            font.bold: true
            visible: text !== ""
            Layout.fillWidth: true
        }

        Text {
            text: root.bodyText
            color: sys.text
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
                    // Let the OS theme handle button styling natively
                    onClicked: root.actionTriggered(modelData.action)
                }
            }
        }
    }
}

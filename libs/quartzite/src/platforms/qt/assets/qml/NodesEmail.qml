// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import com.unaos.lumen 1.0

Rectangle {
    id: root
    color: "#1e1e1e"

    property var historyModel: null
    property var backend: null

    ListView {
        id: emailListView
        anchors.fill: parent
        model: historyModel
        clip: true

        delegate: ItemDelegate {
            width: emailListView.width
            height: 60
            background: Rectangle {
                color: model.toolTip ? "#2d2d30" : "#1e1e1e"
                border.color: "#333"
                border.width: 1
            }

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 10
                spacing: 4

                Text {
                    text: model.display !== undefined ? model.display : ""
                    color: "white"
                    font.bold: true
                    Layout.fillWidth: true
                }
                Text {
                    text: model.decoration !== undefined ? model.decoration : ""
                    color: "#aaa"
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
            }
        }
    }
}

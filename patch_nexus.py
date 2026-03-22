with open('libs/quartzite/src/platforms/qt/assets/qml/NexusPanel.qml', 'r') as f:
    content = f.read()

content = content.replace('text: display', 'text: matrixLabel')

with open('libs/quartzite/src/platforms/qt/assets/qml/NexusPanel.qml', 'w') as f:
    f.write(content)

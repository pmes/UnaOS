#pragma once
#include <QtQuick/QQuickPaintedItem>

class AetherViewBase : public QQuickPaintedItem {
public:
    explicit AetherViewBase(QObject* parent = nullptr) 
        : QQuickPaintedItem(qobject_cast<QQuickItem*>(parent)) {}
};

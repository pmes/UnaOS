#pragma once
#include <QMainWindow>
#include <QQuickWidget>
#include "rust/cxx.h"

class LumenMainWindow : public QMainWindow {
    Q_OBJECT
public:
    explicit LumenMainWindow(QWidget *parent = nullptr);
    ~LumenMainWindow() override;

private:
    QQuickWidget* m_quickWidget;
};

std::unique_ptr<LumenMainWindow> create_main_window();
void show_main_window(LumenMainWindow& window);

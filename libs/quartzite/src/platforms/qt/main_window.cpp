#include "main_window.h"
#include <QVBoxLayout>
#include <QWidget>

LumenMainWindow::LumenMainWindow(QWidget *parent)
    : QMainWindow(parent)
{
    // Configure window
    setWindowTitle("Lumen (Qt)");
    resize(1024, 768);

    // Create central widget
    QWidget* centralWidget = new QWidget(this);
    setCentralWidget(centralWidget);

    // Create layout
    QVBoxLayout* layout = new QVBoxLayout(centralWidget);
    layout->setContentsMargins(0, 0, 0, 0);

    // Initialize QQuickWidget
    m_quickWidget = new QQuickWidget(this);
    m_quickWidget->setResizeMode(QQuickWidget::SizeRootObjectToView);

    // Load QML
    m_quickWidget->setSource(QUrl(QStringLiteral("qrc:/qt/qml/main.qml")));

    layout->addWidget(m_quickWidget);
}

LumenMainWindow::~LumenMainWindow() {
}

std::unique_ptr<LumenMainWindow> create_main_window() {
    return std::make_unique<LumenMainWindow>();
}

void show_main_window(LumenMainWindow& window) {
    window.show();
}

use quartzite::render_ui;

pub struct HistoryItem {
    pub is_chat: bool,
    pub timestamp: String,
    pub display_name: String,
    pub content: String,
}

pub struct AppState {
    pub history: Vec<HistoryItem>,
}

fn main() {
    let app_state = AppState {
        history: vec![
            HistoryItem {
                is_chat: true,
                timestamp: "10:00 AM".to_string(),
                display_name: "Alice".to_string(),
                content: "Hello UnaOS!".to_string(),
            },
            HistoryItem {
                is_chat: false,
                timestamp: "10:01 AM".to_string(),
                display_name: "System".to_string(),
                content: "User logged in.".to_string(), // Should be filtered out
            },
            HistoryItem {
                is_chat: true,
                timestamp: "10:05 AM".to_string(),
                display_name: "Bob".to_string(),
                content: "Checking the new UI macros.".to_string(),
            },
        ],
    };

    render_ui!("layout.json");
}

use quartzite::render_ui;
use bandy::state::AppState;
use bandy::ontology::Origin;
use bandy::state::HistoryItem;
use std::collections::VecDeque;

fn main() {
    let mut history = VecDeque::new();

    // Simulate reading records from disk exactly as the legacy system does it.
    // As seen in legacy/quartzite/src/platforms/macos/spline.rs,
    // new messages are appended to the end of the history array:
    // `history.push(bandy::state::HistoryItem { ... })`
    // Therefore, the items are stored chronologically (oldest at index 0).

    history.push_back(HistoryItem {
        origin: Origin::LocalUser("Alice".to_string()),
        display_name: Some("Alice".to_string()),
        content: "Hello UnaOS!".to_string(),
        timestamp: "10:00 AM".to_string(),
        is_chat: true,
    });

    history.push_back(HistoryItem {
        origin: Origin::System("UnaOS".to_string()),
        display_name: Some("System".to_string()),
        content: "User logged in.".to_string(),
        timestamp: "10:01 AM".to_string(),
        is_chat: false, // Should be filtered out
    });

    history.push_back(HistoryItem {
        origin: Origin::LocalUser("Bob".to_string()),
        display_name: Some("Bob".to_string()),
        content: "Checking the new UI macros.".to_string(),
        timestamp: "10:05 AM".to_string(),
        is_chat: true,
    });

    let app_state = AppState {
        history,
        ..Default::default()
    };

    render_ui!("layout.json");
}

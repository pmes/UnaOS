use bandy::state::HistoryItem;
use std::collections::VecDeque;
use gneiss_pal::persistence::BrainManager;
use gneiss_pal::paths::UnaPaths;

/// A history loading service that reads ACTUAL history items from disk via BrainManager.
/// We use the official `UnaPaths` infrastructure to dynamically resolve the OS's vault.
pub fn load_history_from_disk() -> VecDeque<HistoryItem> {
    // Awaken the spatial paths first just like the real lumen application does.
    let _ = UnaPaths::awaken();

    // The actual system uses primary_vault() from UnaPaths as its storage anchor.
    // This perfectly mirrors the legacy/lumen `let vein_storage = UnaPaths::primary_vault();`
    let brain_path = UnaPaths::primary_vault();

    let manager = BrainManager::new(brain_path);
    let saved_messages = manager.load();
    let mut history = VecDeque::new();

    // Convert saved messages to UI HistoryItems
    for msg in saved_messages {
        let is_user = msg.role == "user";

        let origin = if is_user {
            bandy::ontology::Origin::LocalUser("User".to_string())
        } else {
            bandy::ontology::Origin::System("UnaOS".to_string())
        };

        history.push_back(HistoryItem {
            origin,
            display_name: Some(if is_user { "User".to_string() } else { "System".to_string() }),
            content: msg.content,
            timestamp: msg.timestamp.unwrap_or_else(|| "Unknown Time".to_string()),
            is_chat: true,
        });
    }

    // Fallback error history entry if database is entirely empty
    if history.is_empty() {
        history.push_back(HistoryItem {
            origin: bandy::ontology::Origin::System("UnaOS".to_string()),
            display_name: Some("System".to_string()),
            content: "No previous history found on disk.".to_string(),
            timestamp: "Now".to_string(),
            is_chat: false,
        });
    }

    history
}

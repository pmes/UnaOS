mod history;

use quartzite::render_ui;
use bandy::state::AppState;
use history::load_history_from_disk;

fn main() {
    let history = load_history_from_disk();

    let app_state = AppState {
        history,
        ..Default::default()
    };

    render_ui!("layout.json");
}

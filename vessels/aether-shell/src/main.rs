use std::thread;
use tokio::task::LocalSet;
use bandy::{SMessage, Synapse};
// use quartzite::browser::bootstrap_browser;
use aether::AetherEngine;

fn main() {
    let synapse = Synapse::new();
    
    let engine_tx = synapse.clone();
    let mut engine_rx = synapse.subscribe();
    
    // Spawn the Engine Thread
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let local = LocalSet::new();
        
        local.block_on(&rt, async move {
            let mut engine = AetherEngine::new();
            
            // Initial render
            let _damages = engine.render_frame();
            engine_tx.fire(SMessage::SurfaceBlit {
                url: engine.title.clone(),
                width: engine.width,
                height: engine.height,
                pixels: engine.surface().to_vec(),
            });
            
            let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(16));
            
            loop {
                tokio::select! {
                    _ = tick_interval.tick() => {
                        if engine.tick() {
                            let damages = engine.render_frame();
                            if !damages.is_empty() {
                                engine_tx.fire(SMessage::SurfaceBlit {
                                    url: engine.title.clone(),
                                    width: engine.width,
                                    height: engine.height,
                                    pixels: engine.surface().to_vec(),
                                });
                            }
                        }
                    }
                    Ok(msg) = engine_rx.recv() => {
                        match msg {
                            SMessage::OpenDocument { url } => {
                                match aether::net::fetch_page(&url).await {
                                    Ok((html, sheets)) => engine.load_html_styled(&url, &html, &sheets, true),
                                    Err(e) => engine.load_error_page(&url, &e.to_string()),
                                }
                            }
                            SMessage::BrowserNavBack => {
                                if let Some(url) = engine.get_back_url() {
                                    let (html, sheets) = aether::net::fetch_page(&url).await.unwrap_or_default();
                                    engine.load_html_styled(&url, &html, &sheets, false);
                                }
                            }
                            SMessage::BrowserNavForward => {
                                if let Some(url) = engine.get_forward_url() {
                                    let (html, sheets) = aether::net::fetch_page(&url).await.unwrap_or_default();
                                    engine.load_html_styled(&url, &html, &sheets, false);
                                }
                            }
                            SMessage::BrowserNavReload => {
                                // Reload logic not directly supported by get_forward_url, ignoring for now or using current
                            }
                            SMessage::BrowserScroll(dx, dy) => {
                                engine.handle_event(aether::api::events::Event::Scroll(dx, dy));
                            }
                            SMessage::BrowserResize(w, h) => {
                                engine.handle_event(aether::api::events::Event::Resize(w, h));
                            }
                            SMessage::BrowserKey(key) => {
                                engine.handle_event(aether::api::events::Event::KeyDown(key));
                            }
                            SMessage::BrowserText(text) => {
                                engine.handle_event(aether::api::events::Event::Text(text));
                            }
                            _ => {}
                        }
                    }
                }
            }
        });
    });

    let aether_ui = quartzite::tetra::TetraNode::VStack(vec![
        quartzite::tetra::TetraNode::HStack(vec![
            quartzite::tetra::TetraNode::Button {
                id: "back".into(),
                label: "<".into(),
                action: bandy::SMessage::BrowserNavBack,
            },
            quartzite::tetra::TetraNode::Button {
                id: "fwd".into(),
                label: ">".into(),
                action: bandy::SMessage::BrowserNavForward,
            },
            quartzite::tetra::TetraNode::Button {
                id: "reload".into(),
                label: "C".into(),
                action: bandy::SMessage::BrowserNavReload,
            },
            quartzite::tetra::TetraNode::TextField {
                id: "url".into(),
                placeholder: "Enter URL...".into(),
                action: quartzite::tetra::TextAction::OpenDocument,
            },
        ]),
        quartzite::tetra::TetraNode::Surface { id: "viewport".into() }
    ]);
    
    quartzite::Backend::new_tetra_vessel("org.unaos.aether", "Aether Browser", (800.0, 600.0), aether_ui, synapse.clone()).run();
}

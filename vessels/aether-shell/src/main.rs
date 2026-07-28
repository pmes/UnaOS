use std::thread;
use tokio::task::LocalSet;
use bandy::{SMessage, Synapse};
// use quartzite::browser::bootstrap_browser;
use aether::AetherEngine;

fn main() {
    let synapse = Synapse::new();
    
    let engine_tx = synapse.clone();
    let mut engine_rx = synapse.subscribe();
    
    // Spawn the Engine Thread. Big stack: boa's parser/interpreter is
    // recursive-descent and real-world page bundles nest deeply — the
    // 2 MB default overflows (debug frames especially) once external
    // scripts execute.
    thread::Builder::new()
        .name("aether-engine".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let local = LocalSet::new();
        
        local.block_on(&rt, async move {
            let mut engine = AetherEngine::new();

            // The engine surface is BGRA (headless swaps 0<->2 when writing
            // PNGs); quartzite's set_frame contract is RGBA. Swap on the way
            // out so the viewport isn't presented channel-swapped.
            fn surface_rgba(engine: &AetherEngine) -> Vec<u8> {
                let mut px = engine.surface().to_vec();
                for p in px.chunks_exact_mut(4) {
                    p.swap(0, 2);
                }
                px
            }

            // Initial render
            let _damages = engine.render_frame();
            engine_tx.fire(SMessage::SurfaceBlit {
                url: "viewport".into(),
                width: engine.width,
                height: engine.height,
                pixels: surface_rgba(&engine),
            });
            
            let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(16));
            let mut last_url: Option<String> = None;
            
            loop {
                tokio::select! {
                    _ = tick_interval.tick() => {
                        if engine.tick() {
                            let damages = engine.render_frame();
                            if !damages.is_empty() {
                                engine_tx.fire(SMessage::SurfaceBlit {
                                    url: "viewport".into(),
                                    width: engine.width,
                                    height: engine.height,
                                    pixels: surface_rgba(&engine),
                                });
                            }
                        }
                    }
                    Ok(msg) = engine_rx.recv() => {
                        match msg {
                            SMessage::OpenDocument { url } => {
                                match aether::net::fetch_page(&url).await {
                                    Ok(page) => {
                                        engine.load_page(page, true)
                                    }
                                    Err(e) => engine.load_error_page(&url, &e.to_string()),
                                }
                            }
                            SMessage::BrowserNavBack => {
                                if let Some(url) = engine.get_back_url() {
                                    let page = aether::net::fetch_page(&url).await.unwrap_or_default();
                                    engine.load_page(page, false);
                                }
                            }
                            SMessage::BrowserNavForward => {
                                if let Some(url) = engine.get_forward_url() {
                                    let page = aether::net::fetch_page(&url).await.unwrap_or_default();
                                    engine.load_page(page, false);
                                }
                            }
                            SMessage::BrowserNavReload => {
                                if let Some(url) = engine.history.get(engine.history_idx).cloned() {
                                    match aether::net::fetch_page(&url).await {
                                        Ok(page) => engine.load_page(page, false),
                                        Err(e) => engine.load_error_page(&url, &e.to_string()),
                                    }
                                }
                            }
                            SMessage::BrowserScroll(dx, dy) => {
                                engine.handle_event(aether::api::events::Event::Scroll(dx, dy));
                            }
                            SMessage::BrowserClick(x, y) => {
                                engine.handle_event(aether::api::events::Event::MouseDown(x, y));
                                engine.handle_event(aether::api::events::Event::MouseUp(x, y));
                                // A media-element click stages a play request:
                                // hand the page's own stream to Stria.
                                if let Some((url, title, mime)) = engine.take_pending_media() {
                                    engine_tx.fire(SMessage::PlayMedia { url, title, mime });
                                }
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
                        // A link click or form submit staged a navigation;
                        // run it here where awaiting is legal (the engine
                        // thread must never block on the runtime itself).
                        if let Some(nav) = engine.take_pending_nav() {
                            match nav.method {
                                aether::forms::HttpMethod::Get => {
                                    match aether::net::fetch_page(&nav.url).await {
                                        Ok(page) => engine.load_page(page, true),
                                        Err(e) => engine.load_error_page(&nav.url, &e.to_string()),
                                    }
                                }
                                aether::forms::HttpMethod::Post => {
                                    let body = nav.body.unwrap_or_default();
                                    match aether::net::post_document(&nav.url, &body).await {
                                        Ok(html) => engine.load_html(&nav.url, &html, true),
                                        Err(e) => engine.load_error_page(&nav.url, &e.to_string()),
                                    }
                                }
                            }
                        }
                    }
                }
                // One choke point for the address bar: whatever the last turn
                // of the loop did — link click, form submit, Back, Forward,
                // Reload, typed url, or a script-driven navigation run off the
                // tick — the engine's current history entry is the truth, and
                // any change to it is mirrored to the chrome exactly once.
                if let Some(current) = engine.history.get(engine.history_idx) {
                    if last_url.as_deref() != Some(current.as_str()) {
                        last_url = Some(current.clone());
                        engine_tx.fire(SMessage::BrowserUrlChanged(current.clone()));
                    }
                }
            }
        });
    })
    .expect("spawn engine thread");

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

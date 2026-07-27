use crate::AetherEngine;
use std::path::Path;

/// Loads a document into a fresh engine, renders one frame, and writes the
/// surface PNG plus the API-coverage ledger. Shared by the CLI and tests.
/// Returns (width, height, distinct missing-API count).
pub async fn render_headless(
    url: Option<&str>,
    html_file: Option<&Path>,
    out: &Path,
    ledger_path: &Path,
) -> anyhow::Result<(u32, u32, usize)> {
    crate::ledger::reset();
    let mut engine = AetherEngine::new();

    match (html_file, url) {
        (Some(path), _) => {
            let html = std::fs::read_to_string(path)?;
            let url = format!("file://{}", path.display());
            engine.load_html(&url, &html, true);
        }
        (None, Some(url)) => match crate::net::fetch_document(url).await {
            Ok(html) => engine.load_html(url, &html, true),
            Err(e) => engine.load_error_page(url, &e.to_string()),
        },
        (None, None) => anyhow::bail!("render needs a URL or --html <file>"),
    }

    engine.render_frame();

    let (w, h) = (engine.width, engine.height);
    // The engine surface is BGRA (see render::draw_node); PNG wants RGBA.
    let mut rgba = engine.surface().to_vec();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let img = image::RgbaImage::from_raw(w, h, rgba)
        .ok_or_else(|| anyhow::anyhow!("surface size mismatch: {}x{}", w, h))?;
    img.save(out)?;

    let snapshot = crate::ledger::snapshot();
    snapshot.dump_to_file(ledger_path)?;
    Ok((w, h, snapshot.len()))
}

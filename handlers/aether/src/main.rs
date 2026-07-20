
pub mod net;
pub mod dom;
pub mod layout;
pub mod render;
pub mod js;
pub mod images;
pub mod forms;
pub mod css;

pub mod ledger;
pub mod storage;
pub mod workers;
pub mod event_loop;
pub mod fonts;
pub mod api;

use clap::{Parser, Subcommand};
use anyhow::Result;

#[derive(Parser)]
#[command(name = "aether")]
#[command(about = "Aether Web Browser")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Open a URL and dump the render to a PNG
    Open {
        url: String,
        #[arg(short, long)]
        dump: Option<String>,
    },
}

use bandy::synapse::Synapse;
use bandy::signals::SMessage;

pub async fn ignite(synapse: Synapse) -> Result<()> {
    let mut rx = synapse.subscribe();
    println!("Aether ignited. Listening for OpenDocument messages...");

    while let Ok(msg) = rx.recv().await {
        match msg {
            SMessage::OpenDocument { url } => {
                println!("Received OpenDocument for {}", url);
                

                
                // M1 fetch->parse->layout->render pipeline
                println!("Fetching document...");
                let html = net::fetch_document(&url).await?;
                println!("Fetched {} bytes. Parsing HTML...", html.len());
                let document = dom::parse_html(&html);
                
                println!("Initializing JS Engine...");
                let mut js_engine = js::Engine::new(document.clone());
                
                if let Ok(scripts) = document.select("script") {
                    for script_node in scripts {
                        let text = script_node.text_contents();
                        if !text.trim().is_empty() {
                            println!("Executing script...");
                            if let Err(e) = js_engine.execute(&text) {
                                eprintln!("JS Execution error: {}", e);
                            }
                        }
                    }
                }
                
                println!("Parsed HTML. Computing layout...");
                let mut layout_tree = layout::compute_layout(&document);
                
                if let Ok(styles) = document.select("style") {
                    for style_node in styles {
                        let text = style_node.text_contents();
                        css::apply_css(&mut layout_tree, &text);
                    }
                }

                println!("Layout computed. Rendering...");
                
                let img = render::render_to_image(&layout_tree, 800, 600);
                println!("Render complete. Blitting...");
                let blit = render::create_surface_blit(&url, img);
                
                // Publish back to compositor
                synapse.fire(blit);
            }
            _ => {}
        }
    }
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Open { url, dump } => {
            if let Some(dump_path) = dump {
                println!("Fetching {}...", url);
                let html = net::fetch_document(url).await?;
                let document = dom::parse_html(&html);
                
                let mut js_engine = js::Engine::new(document.clone());
                if let Ok(scripts) = document.select("script") {
                    for script_node in scripts {
                        let text = script_node.text_contents();
                        if !text.trim().is_empty() {
                            let _ = js_engine.execute(&text);
                        }
                    }
                }
                
                let mut layout_tree = layout::compute_layout(&document);
                if let Ok(styles) = document.select("style") {
                    for style_node in styles {
                        let text = style_node.text_contents();
                        css::apply_css(&mut layout_tree, &text);
                    }
                }
                
                let img = render::render_to_image(&layout_tree, 800, 600);
                img.save(dump_path)?;
                println!("Saved PNG dump to {}", dump_path);
            } else {
                // Run the interaction shell
                let synapse = Synapse::new();
                
                // Mock-dispatch for M2 Oracle
                let syn_clone = synapse.clone();
                let u = url.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    syn_clone.fire(SMessage::OpenDocument { url: u });
                });
                
                ignite(synapse).await?;
            }
        }
    }

    Ok(())
}


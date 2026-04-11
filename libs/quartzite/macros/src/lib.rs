extern crate proc_macro;

mod ast;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, LitStr};
use std::env;
use std::fs;
use std::path::PathBuf;

use ast::Node;

/// Recursively generates the executable Rust code for the parsed UI Nodes.
fn generate_node_tokens(node: &Node) -> proc_macro2::TokenStream {
    match node {
        Node::WindowFrame(window) => {
            let id = &window.id;
            let children_tokens = window.children.iter().map(generate_node_tokens);

            // For this phase, we map a WindowFrame to a scope that prints its ID and executes its children
            quote! {
                println!("--- WindowFrame: {} ---", #id);
                #(#children_tokens)*
                println!("--- End WindowFrame: {} ---", #id);
            }
        }
        Node::Iterator(iter) => {
            let bind_expr: proc_macro2::TokenStream = iter.bind.parse().expect("Failed to parse bind expression");
            let item_template = generate_node_tokens(&iter.item_template);

            // Map the iterator DSL to native Rust iterators
            let loop_body = quote! {
                for item in #bind_expr.iter() {
                    #item_template
                }
            };

            if let Some(filter) = &iter.filter {
                // To keep this robust, parse the filter string conditionally.
                // In our Bite 1 constraint, we translate `is_chat == true` to `item.is_chat == true` directly.
                // We will perform a simple textual replacement for 'item.' mapping for simplicity in this phase.
                // An advanced parser would use standard expression parsing.

                // Let's create a robust string replacement to map raw properties to `item.X`
                let parsed_filter = filter.replace("is_chat", "item.is_chat");
                let filter_expr: proc_macro2::TokenStream = parsed_filter.parse().expect("Failed to parse filter expression");

                quote! {
                    for item in #bind_expr.iter().filter(|item| #filter_expr) {
                        #item_template
                    }
                }
            } else {
                loop_body
            }
        }
        Node::Label(label) => {
            // We need to parse mustache `{{var}}` variables inside `label.value`.
            // We translate them to standard `{}` and pass `item.var` as the format arguments.
            let mut template_string = label.value.clone();
            let mut variables = Vec::new();

            // Simple manual extraction of {{var}}
            while let Some(start) = template_string.find("{{") {
                if let Some(end) = template_string[start..].find("}}") {
                    let var_name = template_string[start + 2..start + end].to_string();

                    // Replace {{var}} with {}
                    template_string.replace_range(start..start + end + 2, "{}");

                    // Specific mapping for display_name, which is an Option<String> in HistoryItem
                    // We need to safely unwrap or fallback so it can be formatted natively
                    let var_access = if var_name == "display_name" {
                        format!("item.{}.as_deref().unwrap_or(\"Unknown\")", var_name)
                    } else {
                        format!("item.{}", var_name)
                    };

                    let var_expr: proc_macro2::TokenStream = var_access.parse().expect("Failed to parse label variable");
                    variables.push(var_expr);
                } else {
                    break; // Malformed template, break and let the rust macro fail gracefully if needed
                }
            }

            quote! {
                println!(#template_string, #(#variables),*);
            }
        }
    }
}

/// The core procedural macro to read a JSON DSL blueprint and construct the executable AST.
#[proc_macro]
pub fn render_ui(input: TokenStream) -> TokenStream {
    // Parse the macro input as a string literal representing the file path
    let path_lit = parse_macro_input!(input as LitStr);
    let relative_path = path_lit.value();

    // Resolve the file path relative to the crate invoking the macro (CARGO_MANIFEST_DIR)
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set");
    let mut file_path = PathBuf::from(manifest_dir);
    file_path.push(&relative_path);

    // Read the file from disk
    let json_content = fs::read_to_string(&file_path)
        .unwrap_or_else(|_| panic!("Failed to read blueprint file at {:?}", file_path));

    // Parse the JSON into our AST
    let node: Node = serde_json::from_str(&json_content)
        .unwrap_or_else(|e| panic!("Failed to parse blueprint JSON: {}", e));

    // Generate the executable Rust token stream
    let generated_code = generate_node_tokens(&node);

    // Return the generated tokens wrapped in a block
    TokenStream::from(quote! {
        {
            #generated_code
        }
    })
}

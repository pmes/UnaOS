use syn::{parse_file, Item, ImplItem, TraitItem};
use quote::quote;

/// The Skeleton Generator.
///
/// This struct is responsible for "Magic Token Savings." It takes raw Rust
/// source code and strips away the internal implementation blocks of functions,
/// leaving only the structural signatures, types, and doc-comments.
///
/// Why? Because an LLM context window is precious real estate. We do not
/// waste it on the internal logic of a function unless the AI explicitly
/// requests to modify it.
pub struct SkeletonGenerator;

impl SkeletonGenerator {
    /// Parses a raw Rust source string and returns a minified, token-efficient
    /// skeleton representation of the code.
    ///
    /// # Arguments
    /// * `source_code` - The raw string slice of the Rust file (provided via zero-copy mmap).
    pub fn generate(source_code: &str) -> Result<String, syn::Error> {
        // Parse the raw string into a pure-Rust Abstract Syntax Tree (AST).
        // This is blazingly fast and requires zero C-dependencies.
        let mut ast = parse_file(source_code)?;

        // Iterate mutably over every top-level item in the file.
        for item in &mut ast.items {
            match item {
                // If the item is a standalone function...
                Item::Fn(func) => {
                    // We replace the entire `{ ... }` block with an empty block `{}`.
                    // The signature, lifetimes, generics, and doc-comments remain intact.
                    func.block.stmts.clear();
                }
                // If the item is an `impl` block (e.g., impl MyStruct { ... })
                Item::Impl(impl_block) => {
                    for impl_item in &mut impl_block.items {
                        if let ImplItem::Fn(method) = impl_item {
                            // Strip the method body.
                            method.block.stmts.clear();
                        }
                    }
                }
                // If the item is a Trait definition...
                Item::Trait(trait_block) => {
                    for trait_item in &mut trait_block.items {
                        if let TraitItem::Fn(trait_method) = trait_item {
                            // Traits can have default implementations. We strip those too.
                            if let Some(default_block) = &mut trait_method.default {
                                default_block.stmts.clear();
                            }
                        }
                    }
                }
                // Structs, Enums, Macros, and Use statements are left untouched,
                // as they are critical for architectural context.
                _ => {}
            }
        }

        // Convert the modified AST back into a formatted Rust string.
        // quote! handles the token stream reconstruction beautifully.
        let skeleton = quote!(#ast).to_string();

        Ok(skeleton)
    }
}

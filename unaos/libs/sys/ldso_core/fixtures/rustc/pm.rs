// SPDX-License-Identifier: GPL-3.0-or-later
// SELFBUILD6 (B360) proc_macro: a one-derive proc-macro crate, built by the musl rustc under ldrun at image time and staged as
// /apps/LIB/rustc/pm/libpm.so; rustc on UnaOS dlopens it through the loader trampoline.
extern crate proc_macro;
use proc_macro::TokenStream;
#[proc_macro_derive(Hello)]
pub fn hello(input: TokenStream) -> TokenStream {
    let s = input.to_string();
    let name = s.split_whitespace().skip_while(|w| *w != "struct").nth(1).unwrap().trim_end_matches(';').to_string();
    format!("impl {0} {{ fn hello() -> &'static str {{ \"derived Hello for {0}\" }} }}", name).parse().unwrap()
}

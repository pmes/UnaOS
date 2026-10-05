// SPDX-License-Identifier: GPL-3.0-or-later
// SELFBUILD6 (B360) proc_macro: the crate that uses the derive (rustc ... --extern pm=/apps/LIB/rustc/pm/libpm.so).
#[macro_use]
extern crate pm;
#[derive(Hello)]
struct Probe;
fn main() {
    println!("proc_macro: {}", Probe::hello());
}

// SPDX-License-Identifier: GPL-3.0-or-later
// SELFBUILD6 (B360) rustc_hello: what the musl-host rustc compiles on UnaOS (and, first, under ldrun on the host).
fn main() {
    let v: Vec<u64> = (1..=10).collect();
    println!("hello from rustc on ldso_core: sum={}", v.iter().sum::<u64>());
}

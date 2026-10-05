//! `js_core-test262`: run the official ECMAScript conformance suite against js_core.
//!
//! cargo run --release -p js_core --example js_core-test262 -- <test262-dir> [--parse] [-v] [--json out] [filters…]
//! `--parse` checks only the syntactic grammar (M1): negative parse tests must fail, everything else must parse.

#[path = "../tests/t262/meta.rs"]
mod meta;
#[path = "../tests/t262/runner.rs"]
mod runner;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(runner::main(args));
}

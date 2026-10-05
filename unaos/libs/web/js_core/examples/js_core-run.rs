//! `js_core-run`: run a script file (or `-e <source>`) on js_core and print its output. Debugging aid and the
//! driver for the microbenchmarks.
//!
//! cargo run --release -p js_core --example js_core-run -- <file.js | -e source>

#[path = "../tests/t262/host.rs"]
mod host;

use js_core::vm::*;
use std::cell::RefCell;
use std::rc::Rc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (src, base) = if args.first().map(|s| s.as_str()) == Some("-e") {
        (args.get(1).cloned().unwrap_or_default(), std::path::PathBuf::from("."))
    } else {
        let p = std::path::PathBuf::from(args.first().expect("usage: js_core-run <file.js | -e source>"));
        (std::fs::read_to_string(&p).expect("read"), p.parent().map(|x| x.to_path_buf()).unwrap_or_default())
    };
    let out = Rc::new(RefCell::new(String::new()));
    let mut vm = Vm::new(Box::new(host::TestHost { out: out.clone(), base }));
    host::install(&mut vm);
    let units: Vec<u16> = src.encode_utf16().collect();
    let r = vm.run_script(&units).and_then(|_| vm.run_jobs());
    print!("{}", out.borrow());
    if let Err(e) = r {
        let msg = vm.error_string(&e);
        println!("Uncaught {}", msg);
        std::process::exit(1);
    }
}

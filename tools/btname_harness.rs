//! Host-side driver for the BT-L2 Local Name walk — THE SAME SOURCE THE KERNEL RUNS.
//!
//! `bt_name.rs` is `include!`d, not copied. A harness that tests a transcription of the walk
//! proves nothing about the walk the radio path actually uses, and the whole reason the walk moved
//! into its own file was to make that sharing possible. If this harness passes and the kernel
//! disagrees, the difference is in the CALLER, never in the decode.
//!
//! It exists because the in-kernel fixture (`bt_name_fixture`) can only speak on a boot, and
//! "the fixture would catch it" is a claim that has to be demonstrated, not asserted. Break the
//! length handling in `bt_decode_local_name` and this prints FAIL for `megaboom-complete` in under
//! a second, with no bench time and no metal.
//!
//! Not a workspace member on purpose — it is one file with no dependencies, so it needs no
//! manifest and cannot perturb any kernel or userspace build:
//!
//!     rustc -O tools/btname_harness.rs -o ~/unaos-bench/scratch/gr23/btname && ~/unaos-bench/scratch/gr23/btname
//!
//! Exit status is 0 only when every leg passes.

include!("../unaos/crates/kernel/src/drivers/ehci/bt_name.rs");

fn render(b: &[u8]) -> String {
    let mut s = String::new();
    for &c in b {
        if (0x20..0x7F).contains(&c) {
            s.push(c as char);
        } else {
            s.push('.');
        }
    }
    s
}

fn main() {
    let mut pass = 0usize;
    let mut fail = 0usize;
    for c in BT_NAME_CASES {
        let got = bt_decode_local_name(c.data);
        let ok = bt_name_case_passes(c);
        if ok {
            pass += 1;
            println!(
                "PASS {:<40} name=\"{}\" complete={} cut={}",
                c.what,
                render(got.as_bytes()),
                got.ncomplete,
                got.ncut
            );
        } else {
            fail += 1;
            println!(
                "FAIL {:<40} want name=\"{}\" complete={} cut={} | got name=\"{}\" complete={} cut={}\n     why: {}",
                c.what,
                render(c.want_name),
                c.want_complete,
                c.want_cut,
                render(got.as_bytes()),
                got.ncomplete,
                got.ncut,
                c.why
            );
        }
    }

    // The match rules the name filter is built out of, exercised on the name the bench cares about.
    let cases: &[(&[u8], &[u8], bool, &str)] = &[
        (b"MEGABOOM", b"MEGABOOM", true, "exact"),
        (b"megaboom", b"MEGABOOM", true, "case-folded"),
        (b"UE MEGABOOM 3", b"MEGABOOM", true, "substring"),
        (b"MEGA", b"MEGABOOM", false, "shortened is NOT a match"),
        (b".", b"MEGABOOM", false, "the Boot AR name must NOT match"),
    ];
    for (hay, needle, want, what) in cases {
        let got = bt_name_contains_ci(hay, needle);
        if got == *want {
            pass += 1;
            println!("PASS {:<40} contains_ci(\"{}\") = {}", what, render(hay), got);
        } else {
            fail += 1;
            println!(
                "FAIL {:<40} contains_ci(\"{}\") = {} want {}",
                what,
                render(hay),
                got,
                want
            );
        }
    }
    // A shortened name the target could still straddle is a MAYBE, and must be distinguishable
    // from both a hit and a miss.
    for (hay, want, what) in [
        (&b"MEGA"[..], true, "MEGA straddles MEGABOOM"),
        (&b"."[..], false, "the Boot AR name straddles nothing"),
    ] {
        let got = bt_name_maybe_ci(hay, b"MEGABOOM");
        if got == want {
            pass += 1;
            println!("PASS {:<40} maybe_ci(\"{}\") = {}", what, render(hay), got);
        } else {
            fail += 1;
            println!("FAIL {:<40} maybe_ci(\"{}\") = {} want {}", what, render(hay), got, want);
        }
    }

    // ---- BT-L3: the ADDRESS rule, same shared source ------------------------------------------
    for c in BT_ADDR_CASES {
        let got = bt_addr_matches(&c.addr, c.atype, &c.want, c.want_type);
        if bt_addr_case_passes(c) {
            pass += 1;
            println!(
                "PASS {:<40} addr={} type={:#04x} match={}",
                c.what,
                render(&bt_addr_render_msb(&c.addr)),
                c.atype,
                got
            );
        } else {
            fail += 1;
            println!(
                "FAIL {:<40} addr={} type={:#04x} match={} want {}\n     why: {}",
                c.what,
                render(&bt_addr_render_msb(&c.addr)),
                c.atype,
                got,
                c.expect,
                c.why
            );
        }
    }
    // THE BYTE-ORDER RELATIONSHIP — the constant, rendered the way the witness renders a stored
    // address, against the text the host's own pairing record prints. This is the leg that fails
    // if `BT_L3_PEER_ADDR_BYTES` is ever rewritten MSB-first; the match legs above would not,
    // because they compare the constant against itself.
    if bt_addr_order_holds() {
        pass += 1;
        println!(
            "PASS {:<40} wire {:02x?} renders as {} == host's {}",
            "byte-order-relationship",
            BT_L3_PEER_ADDR_BYTES,
            render(&bt_addr_render_msb(&BT_L3_PEER_ADDR_BYTES)),
            render(BT_L3_PEER_ADDR_TEXT)
        );
    } else {
        fail += 1;
        println!(
            "FAIL {:<40} wire {:02x?} renders as {} but the host reports {}\n     why: the target constant is in the WRONG BYTE ORDER — the address filter cannot match anything",
            "byte-order-relationship",
            BT_L3_PEER_ADDR_BYTES,
            render(&bt_addr_render_msb(&BT_L3_PEER_ADDR_BYTES)),
            render(BT_L3_PEER_ADDR_TEXT)
        );
    }

    println!("\nbtname harness — pass={} fail={}", pass, fail);
    if fail != 0 {
        std::process::exit(1);
    }
}

//! CHARTER: Vein — shared-core
//!
//! DROPLOOPS (LEDGER SR179; BP-ring3-19) — THE PROGRAM'S BUS CLIENT. Every ring-3 program that links this crate (LUMEN,
//! MIDDEN, UNA, AETHER, NET, DIAG, STRIA) has exactly ONE mailbox keep, [`keep`], and asks a plain bus REQUEST through ONE fn,
//! [`ask`], which waits through `una_abi::bus_ask` (MAILBOXKEEP SR177): a frame met before the reply is KEPT for the
//! program's reader (`theme::due`, which receives through `una_abi::mbox_recv` with the same keep), never dropped. The
//! keep lives here, not in una-abi, because una-abi stays static-free (BP-ring3-1) and this crate is each program's
//! ring-3 runtime. Single-threaded program state behind `static mut`, as every static of this crate.
//! BUSKEEPCRATE (LEDGER SR186): this module needs no feature — a program that links the crate with
//! `default-features = false` (STRIA.ELF) gets its keep without the `tls` half's crates.

use crate::sys::sys;
use una_abi::{MboxKeep, BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REQUEST, BUS_MAGIC, BUS_VERSION};

/// The largest request body [`ask`] sends (a PrefDeclare stanza is the largest caller).
const BODY_MAX: usize = 512;

static mut KEEP: MboxKeep = MboxKeep::new();
static mut TX: [u8; BUS_HDR_LEN + BODY_MAX] = [0; BUS_HDR_LEN + BODY_MAX];
static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
static mut CORR: u32 = 0x5642_0000;

/// THIS program's mailbox keep: every reply-waiting helper keeps into it, the program's reader drains it first.
pub fn keep() -> &'static mut MboxKeep {
    unsafe { &mut *core::ptr::addr_of_mut!(KEEP) }
}

/// A fresh correlation id for one [`ask`] (one counter for every plain request of this program).
fn next_corr() -> u32 {
    unsafe {
        CORR = CORR.wrapping_add(1);
        CORR
    }
}

/// One REQUEST of `verb` with `body`; its reply's body is copied into `out` (at most `out.len()` bytes): `Ok(copied)`.
/// `Err(status)` when the reply refuses (`-ENOENT` = unset for the PREFS verbs), `Err(-errno)` when the send or a
/// receive failed, `Err(-EIO)` after `una_abi::BUS_ASK_TRIES` frames, `Err(-EINVAL)` for a body over 512 bytes.
pub fn ask(verb: u8, body: &[u8], out: &mut [u8]) -> Result<usize, i64> {
    let (tx, rx) = unsafe { (&mut *core::ptr::addr_of_mut!(TX), &mut *core::ptr::addr_of_mut!(RX)) };
    if body.len() > BODY_MAX {
        return Err(una_abi::EINVAL);
    }
    let corr = next_corr();
    tx[..BUS_HDR_LEN].fill(0);
    tx[0..4].copy_from_slice(&BUS_MAGIC);
    tx[4] = BUS_VERSION;
    tx[5] = BUS_KIND_REQUEST;
    tx[6] = verb;
    tx[8..12].copy_from_slice(&corr.to_le_bytes());
    tx[48..52].copy_from_slice(&(body.len() as u32).to_le_bytes());
    tx[BUS_HDR_LEN..BUS_HDR_LEN + body.len()].copy_from_slice(body);
    let (status, blen) = una_abi::bus_ask(&tx[..BUS_HDR_LEN + body.len()], verb, corr, rx, keep(), sys)?;
    if status != 0 {
        return Err(status);
    }
    let m = blen.min(out.len());
    out[..m].copy_from_slice(&rx[BUS_HDR_LEN..BUS_HDR_LEN + m]);
    Ok(m)
}

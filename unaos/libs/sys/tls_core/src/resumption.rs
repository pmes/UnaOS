//! TLS 1.3 session resumption (RFC 8446 §2.2, §4.2.11, §4.6.1): tickets the server sent, kept by the CALLER.
//!
//! tls_core owns the protocol — turning a NewSessionTicket into a resumption PSK, offering it with a binder in
//! the next ClientHello (psk_dhe_ke only: every resumed handshake still runs (EC)DHE, so it keeps forward
//! secrecy), and checking the server's choice — while WHERE tickets live is the caller's: [`TicketStore`] is the
//! seam (the host: a file, see `http_core::host`; the metal: Holocron later). [`MemoryTicketStore`] is the
//! in-process store.
//!
//! Rules this module and the client enforce:
//! * a ticket is used at most once (`take` removes it — RFC 8446 §C.4 discourages reuse: it links connections);
//! * only for the exact server name it was issued under (§4.6.1: the new SNI must be valid for the original
//!   certificate — an exact match is the safe reading);
//! * only within its lifetime, which may not exceed 7 days (§4.6.1), and never "from the future";
//! * only with a cipher suite whose hash is the ticket's (§4.2.11);
//! * 0-RTT is REFUSED: no early_data is ever offered. Early data is replayable by a network attacker (RFC 8446
//!   §8, §E.5) and is not forward-secret against the ticket key; the callers' requests (POSTs to the Messages
//!   API, form submissions) are not idempotent, and the round trip it saves is not worth a replayed request.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::msgs::CipherSuite;
use crate::x509::Clock;

/// RFC 8446 §4.6.1: ticket_lifetime MUST NOT exceed seven days.
pub const MAX_TICKET_LIFETIME: u32 = 604_800;

/// One resumable session: what the client needs to offer the PSK and to describe the resumed connection.
#[derive(Clone, PartialEq, Eq)]
pub struct Ticket {
    pub server_name: String,
    /// The cipher suite of the connection that received it (its hash is the PSK's hash).
    pub suite: CipherSuite,
    /// HKDF-Expand-Label(resumption_master_secret, "resumption", ticket_nonce, Hash.length).
    pub psk: Vec<u8>,
    /// The opaque ticket (the PSK identity).
    pub ticket: Vec<u8>,
    pub age_add: u32,
    pub lifetime: u32,
    /// When it arrived, in milliseconds since the epoch (the caller's clock).
    pub received_ms: u64,
    pub alpn: Option<Vec<u8>>,
    /// The scheme that authenticated the original handshake (reported for the resumed one).
    pub signature_scheme: u16,
    /// max_early_data_size the server advertised — recorded, never used (0-RTT is refused).
    pub max_early_data: Option<u32>,
}

impl core::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never print the PSK.
        f.debug_struct("Ticket")
            .field("server_name", &self.server_name)
            .field("suite", &self.suite)
            .field("ticket_len", &self.ticket.len())
            .field("lifetime", &self.lifetime)
            .field("received_ms", &self.received_ms)
            .finish()
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        for b in self.psk.iter_mut() {
            *b = 0;
        }
        core::hint::black_box(&self.psk);
    }
}

const MAGIC: &[u8; 4] = b"UTK1";

impl Ticket {
    /// Is it usable at `now_ms`? (Not expired, not from the future, lifetime within the RFC's bound.)
    pub fn usable_at(&self, now_ms: u64) -> bool {
        self.lifetime > 0
            && self.lifetime <= MAX_TICKET_LIFETIME
            && now_ms >= self.received_ms
            && now_ms - self.received_ms < self.lifetime as u64 * 1000
    }

    /// obfuscated_ticket_age = (age in ms + ticket_age_add) mod 2^32 (RFC 8446 §4.2.11.1).
    pub fn obfuscated_age(&self, now_ms: u64) -> u32 {
        (now_ms.saturating_sub(self.received_ms) as u32).wrapping_add(self.age_add)
    }

    /// A stable binary form for a persistent store (it contains the PSK: store it like a key).
    pub fn to_bytes(&self) -> Vec<u8> {
        fn put(o: &mut Vec<u8>, b: &[u8]) {
            o.extend_from_slice(&(b.len() as u32).to_be_bytes());
            o.extend_from_slice(b);
        }
        let mut o = Vec::new();
        o.extend_from_slice(MAGIC);
        put(&mut o, self.server_name.as_bytes());
        o.extend_from_slice(&self.suite.code().to_be_bytes());
        put(&mut o, &self.psk);
        put(&mut o, &self.ticket);
        o.extend_from_slice(&self.age_add.to_be_bytes());
        o.extend_from_slice(&self.lifetime.to_be_bytes());
        o.extend_from_slice(&self.received_ms.to_be_bytes());
        match &self.alpn {
            Some(a) => {
                o.push(1);
                put(&mut o, a);
            }
            None => o.push(0),
        }
        o.extend_from_slice(&self.signature_scheme.to_be_bytes());
        match self.max_early_data {
            Some(v) => {
                o.push(1);
                o.extend_from_slice(&v.to_be_bytes());
            }
            None => o.push(0),
        }
        o
    }

    pub fn from_bytes(b: &[u8]) -> Option<Ticket> {
        struct R<'a>(&'a [u8]);
        impl<'a> R<'a> {
            fn take(&mut self, n: usize) -> Option<&'a [u8]> {
                if self.0.len() < n {
                    return None;
                }
                let (a, b) = self.0.split_at(n);
                self.0 = b;
                Some(a)
            }
            fn u8(&mut self) -> Option<u8> {
                Some(self.take(1)?[0])
            }
            fn u16(&mut self) -> Option<u16> {
                Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
            }
            fn u32(&mut self) -> Option<u32> {
                Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
            }
            fn u64(&mut self) -> Option<u64> {
                Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
            }
            fn vec(&mut self) -> Option<Vec<u8>> {
                let n = self.u32()? as usize;
                Some(self.take(n)?.to_vec())
            }
        }
        let mut r = R(b);
        if r.take(4)? != MAGIC {
            return None;
        }
        let server_name = String::from_utf8(r.vec()?).ok()?;
        let suite = CipherSuite::from_code(r.u16()?).filter(|s| s.is_tls13())?;
        let psk = r.vec()?;
        let ticket = r.vec()?;
        let age_add = r.u32()?;
        let lifetime = r.u32()?;
        let received_ms = r.u64()?;
        let alpn = match r.u8()? {
            0 => None,
            1 => Some(r.vec()?),
            _ => return None,
        };
        let signature_scheme = r.u16()?;
        let max_early_data = match r.u8()? {
            0 => None,
            1 => Some(r.u32()?),
            _ => return None,
        };
        if !r.0.is_empty() || psk.len() != suite.hash().output_len() || ticket.is_empty() {
            return None;
        }
        Some(Ticket { server_name, suite, psk, ticket, age_add, lifetime, received_ms, alpn, signature_scheme, max_early_data })
    }
}

/// Where tickets live. Implementations decide persistence and eviction; the client only `put`s what servers
/// send and `take`s one ticket for the next connection to the same name.
pub trait TicketStore {
    fn put(&self, ticket: Ticket);
    /// Removes and returns a ticket for `server_name` (the newest usable one is the natural choice).
    fn take(&self, server_name: &str) -> Option<Ticket>;
}

/// The client's resumption settings: a store and the clock ticket ages are measured with.
#[derive(Clone, Copy)]
pub struct Resumption<'a> {
    pub store: &'a dyn TicketStore,
    pub clock: &'a dyn Clock,
}

/// An in-process store: at most `per_name` tickets per server name, newest first.
pub struct MemoryTicketStore {
    tickets: RefCell<Vec<Ticket>>,
    pub per_name: usize,
}

impl MemoryTicketStore {
    pub fn new() -> Self {
        MemoryTicketStore { tickets: RefCell::new(Vec::new()), per_name: 4 }
    }
    pub fn len(&self) -> usize {
        self.tickets.borrow().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemoryTicketStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TicketStore for MemoryTicketStore {
    fn put(&self, ticket: Ticket) {
        let mut v = self.tickets.borrow_mut();
        v.insert(0, ticket);
        let name = v[0].server_name.clone();
        let mut seen = 0;
        let per = self.per_name;
        v.retain(|t| {
            if t.server_name != name {
                return true;
            }
            seen += 1;
            seen <= per
        });
    }
    fn take(&self, server_name: &str) -> Option<Ticket> {
        let mut v = self.tickets.borrow_mut();
        let i = v.iter().position(|t| t.server_name == server_name)?;
        Some(v.remove(i))
    }
}

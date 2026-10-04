// SPDX-License-Identifier: GPL-3.0-or-later
// NETRING3 (B306): compile-only spike — TlsConnection::open over a socket adapter with SYS_GETRANDOM (56) as the RNG and verify=NONE (UnsecureProvider). `sys` stands in for the user-net syscall stubs. See Cargo.toml for the measurement.
#![no_std]
#![no_main]
use embedded_tls::blocking::*;
use embedded_io::{ErrorType, Read, Write};
#[panic_handler] fn p(_: &core::panic::PanicInfo) -> ! { loop {} }
struct Sock;
impl ErrorType for Sock { type Error = embedded_io::ErrorKind; }
extern "C" { fn sys(n: u64, a: u64, b: u64) -> i64; }
impl Read for Sock { fn read(&mut self, b: &mut [u8]) -> Result<usize, Self::Error> { let r = unsafe { sys(46, b.as_mut_ptr() as u64, b.len() as u64) }; if r < 0 { Err(embedded_io::ErrorKind::Other) } else { Ok(r as usize) } } }
impl Write for Sock { fn write(&mut self, b: &[u8]) -> Result<usize, Self::Error> { let r = unsafe { sys(45, b.as_ptr() as u64, b.len() as u64) }; if r < 0 { Err(embedded_io::ErrorKind::Other) } else { Ok(r as usize) } } fn flush(&mut self) -> Result<(), Self::Error> { Ok(()) } }
struct Rng;
impl rand_core::RngCore for Rng {
  fn next_u32(&mut self) -> u32 { let mut b=[0u8;4]; self.fill_bytes(&mut b); u32::from_le_bytes(b) }
  fn next_u64(&mut self) -> u64 { let mut b=[0u8;8]; self.fill_bytes(&mut b); u64::from_le_bytes(b) }
  fn fill_bytes(&mut self, d: &mut [u8]) { unsafe { sys(56, d.as_mut_ptr() as u64, d.len() as u64); } }
  fn try_fill_bytes(&mut self, d: &mut [u8]) -> Result<(), rand_core::Error> { self.fill_bytes(d); Ok(()) }
}
impl rand_core::CryptoRng for Rng {}
static mut RB: [u8; 16640] = [0; 16640];
static mut WB: [u8; 4096] = [0; 4096];
#[no_mangle] pub extern "C" fn _start() -> ! {
  let cfg = TlsConfig::new().with_server_name("api.anthropic.com");
  let mut c: TlsConnection<Sock, Aes128GcmSha256> = TlsConnection::new(Sock, unsafe { &mut *core::ptr::addr_of_mut!(RB) }, unsafe { &mut *core::ptr::addr_of_mut!(WB) });
  let r = c.open(TlsContext::new(&cfg, UnsecureProvider::new::<Aes128GcmSha256>(Rng)));
  unsafe { sys(1, r.is_ok() as u64, 0); }
  loop {}
}

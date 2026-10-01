// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Writes HELLO.LNX: ET_EXEC, one PT_LOAD (R+X) at 0x400000 covering the whole file, entry right after
//! the headers. Code: write(1,msg,len); exit_group(0). 64 B ehdr + 56 B phdr + 33 B code + 21 B message.

fn main() {
    let out = std::env::args().nth(1).expect("usage: user-linux-hello <out-path>");
    let msg = b"hello from linux abi\n";
    let code_off = 120u64;
    let base = 0x40_0000u64;
    let mut code: Vec<u8> = Vec::new();
    code.extend_from_slice(&[0xb8, 1, 0, 0, 0]); // mov eax, 1        (SYS_write)
    code.extend_from_slice(&[0xbf, 1, 0, 0, 0]); // mov edi, 1        (stdout)
    // lea rsi, [rip + rel32]: rip after the lea = code[17]; the message follows the 33-byte code
    code.extend_from_slice(&[0x48, 0x8d, 0x35]);
    code.extend_from_slice(&(33i32 - 17).to_le_bytes());
    code.push(0xba); // mov edx, len
    code.extend_from_slice(&(msg.len() as u32).to_le_bytes());
    code.extend_from_slice(&[0x0f, 0x05]); // syscall
    code.extend_from_slice(&[0xb8, 0xe7, 0, 0, 0]); // mov eax, 231     (SYS_exit_group)
    code.extend_from_slice(&[0x31, 0xff]); // xor edi, edi
    code.extend_from_slice(&[0x0f, 0x05]); // syscall
    assert_eq!(code.len(), 33);
    let total = code_off + code.len() as u64 + msg.len() as u64;

    let mut f: Vec<u8> = Vec::new();
    f.extend_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0]);
    f.extend_from_slice(&[0; 8]);
    f.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    f.extend_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    f.extend_from_slice(&1u32.to_le_bytes()); // EV_CURRENT
    f.extend_from_slice(&(base + code_off).to_le_bytes()); // e_entry
    f.extend_from_slice(&64u64.to_le_bytes()); // e_phoff
    f.extend_from_slice(&0u64.to_le_bytes()); // e_shoff
    f.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    f.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    f.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    f.extend_from_slice(&1u16.to_le_bytes()); // e_phnum
    f.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    f.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    f.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx
    // PT_LOAD
    f.extend_from_slice(&1u32.to_le_bytes());
    f.extend_from_slice(&5u32.to_le_bytes()); // R+X
    f.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    f.extend_from_slice(&base.to_le_bytes()); // p_vaddr
    f.extend_from_slice(&base.to_le_bytes()); // p_paddr
    f.extend_from_slice(&total.to_le_bytes()); // p_filesz
    f.extend_from_slice(&total.to_le_bytes()); // p_memsz
    f.extend_from_slice(&0x1000u64.to_le_bytes()); // p_align
    assert_eq!(f.len() as u64, code_off);
    f.extend_from_slice(&code);
    f.extend_from_slice(msg);
    assert_eq!(f.len() as u64, total);
    std::fs::write(&out, &f).expect("write fixture");
    println!("wrote {} ({} bytes)", out, f.len());
}

# LINUXABI3 fixture SSE.LNX: SSE/SSE2 in ring 3 + live XMM state across fork and context switch.
# Exit codes: 0 = all good ("sse ok\n" on stdout, copied there by a 64-byte SSE memcpy); 11.. = which check failed.
# Stack frame (16-aligned): 0x00 paddd result, 0x10..0x50 memcpy dest, 0x50 mxcsr scratch, 0x54 mxcsr after the arithmetic, 0x60 wait status,
# 0x80..0x100 the eight xmm8..xmm15 patterns, 0x100 child timespec (30 ms), 0x110 parent timespec (10 ms).
.intel_syntax noprefix
.text
.globl _start
_start:
  and rsp, -16
  sub rsp, 0x200
  # --- 0. MXCSR is the Linux initial value 0x1f80 (all exceptions masked, round-to-nearest, no flags) ---
  mov r15d, 14
  stmxcsr [rsp+0x50]
  cmp dword ptr [rsp+0x50], 0x1f80
  jne fail
  # --- 1. movaps (aligned store/load on the stack) + paddd ---
  movups xmm0, [rip+v_a]
  movups xmm1, [rip+v_b]
  movaps [rsp], xmm0
  movaps xmm2, [rsp]
  paddd xmm2, xmm1
  movaps [rsp], xmm2
  mov r15d, 11
  cmp dword ptr [rsp], 11
  jne fail
  cmp dword ptr [rsp+4], 22
  jne fail
  cmp dword ptr [rsp+8], 33
  jne fail
  cmp dword ptr [rsp+12], 44
  jne fail
  # --- 2. scalar double: cvtsi2sd / mulsd / addsd / cvttsd2si / cvtsd2si (round-to-nearest-even) ---
  mov r15d, 12
  mov eax, 7
  cvtsi2sd xmm3, eax
  mulsd xmm3, xmm3
  addsd xmm3, [rip+c_half]
  cvttsd2si eax, xmm3
  cmp eax, 49
  jne fail
  mov r15d, 13
  cvtsd2si eax, xmm3
  cmp eax, 50
  jne fail
  # --- 3. remember MXCSR after the arithmetic (the PE sticky flag is now set): it must survive the switches below ---
  stmxcsr [rsp+0x54]
  # --- 4. a 64-byte memcpy through xmm4..xmm7 (unaligned src in the image, aligned dst on the stack) ---
  movdqu xmm4, [rip+msg]
  movdqu xmm5, [rip+msg+16]
  movdqu xmm6, [rip+msg+32]
  movdqu xmm7, [rip+msg+48]
  movdqa [rsp+0x10], xmm4
  movdqa [rsp+0x20], xmm5
  movdqa [rsp+0x30], xmm6
  movdqa [rsp+0x40], xmm7
  # --- 5. live XMM across fork + context switches ---
  movups xmm0, [rip+v_p]
  mov eax, 1
  movd xmm1, eax
  pshufd xmm1, xmm1, 0
.irp r,8,9,10,11,12,13,14,15
  movdqa xmm\r, xmm0
  movaps [rsp+0x80+16*(\r-8)], xmm\r
  paddd xmm0, xmm1
.endr
  mov qword ptr [rsp+0x100], 0
  mov qword ptr [rsp+0x108], 30000000
  mov qword ptr [rsp+0x110], 0
  mov qword ptr [rsp+0x118], 10000000
  mov r12d, 0x0101a5a5
  call do_fork
  mov r12d, 0x5a5a0202
  call do_fork
  # parent: sleep while both children run with their own (mutated) XMM, then reap both
  mov eax, 35
  lea rdi, [rsp+0x110]
  xor esi, esi
  syscall
  call reap
  call reap
  mov r15d, 15
  call check
  mov r15d, 16
  stmxcsr [rsp+0x50]
  mov eax, [rsp+0x54]
  cmp [rsp+0x50], eax
  jne fail
  # all good: write the 7 bytes the SSE memcpy put on the stack
  mov eax, 1
  mov edi, 1
  lea rsi, [rsp+0x10]
  mov edx, 7
  syscall
  mov eax, 231
  xor edi, edi
  syscall

# fork; the child never returns (it checks, mutates, sleeps, re-checks, exits). The parent returns.
do_fork:
  mov eax, 57
  syscall
  test eax, eax
  js fork_bad
  jnz 1f
  # child (rsp = parent's rsp at the call, so the frame is at rsp+8)
  add rsp, 8
  mov r15d, 21
  call check
  movd xmm7, r12d
  pshufd xmm7, xmm7, 0
.irp r,8,9,10,11,12,13,14,15
  pxor xmm\r, xmm7
.endr
  mov eax, 35
  lea rdi, [rsp+0x100]
  xor esi, esi
  syscall
.irp r,8,9,10,11,12,13,14,15
  pxor xmm\r, xmm7
.endr
  mov r15d, 22
  call check
  mov eax, 231
  xor edi, edi
  syscall
1:
  ret
fork_bad:
  mov r15d, 17
  jmp fail

# wait4(-1, &status, 0, 0); status must be 0
reap:
  mov eax, 61
  mov rdi, -1
  lea rsi, [rsp+8+0x60]
  xor edx, edx
  xor r10d, r10d
  syscall
  mov r15d, 18
  test rax, rax
  jle fail
  mov r15d, 19
  cmp dword ptr [rsp+8+0x60], 0
  jne fail
  ret

# xmm8..xmm15 == the eight saved patterns (caller's frame at rsp+8), else exit r15d
check:
.irp r,8,9,10,11,12,13,14,15
  movdqa xmm0, xmm\r
  pcmpeqb xmm0, [rsp+8+0x80+16*(\r-8)]
  pmovmskb eax, xmm0
  cmp eax, 0xffff
  jne fail
.endr
  ret

fail:
  mov eax, 231
  mov edi, r15d
  syscall

v_a: .long 1, 2, 3, 4
v_b: .long 10, 20, 30, 40
v_p: .long 0x13579bdf, 0x2468ace0, 0x0badf00d, 0x7e57c0de
c_half: .double 0.5
msg: .ascii "sse ok\n"
     .fill 57, 1, 0

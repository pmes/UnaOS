.intel_syntax noprefix
.text
.globl _start
_start:
  sub rsp, 0x2000
  # LINUXABI3 M5: read stdin only when a line is already waiting (poll fd 0, timeout 0). Before, the first syscall was a
  # blocking read(0): run interactively with nothing typed, `linux /apps/LS.LNX` sat in it until Ctrl-C (boot 19, 50 s).
  mov dword ptr [rsp+0x1f00], 0
  mov dword ptr [rsp+0x1f04], 1
  mov eax, 7
  lea rdi, [rsp+0x1f00]
  mov esi, 1
  xor edx, edx
  syscall
  test rax, rax
  jle skipw
  test word ptr [rsp+0x1f06], 1
  jz skipw
  xor eax, eax
  xor edi, edi
  mov rsi, rsp
  mov edx, 64
  syscall
  test rax, rax
  jle skipw
  mov rdx, rax
  mov eax, 1
  mov edi, 1
  mov rsi, rsp
  syscall
skipw:
  mov eax, 2
  lea rdi, [rip+path]
  mov esi, 0x10000
  xor edx, edx
  syscall
  test rax, rax
  js bad
  mov ebx, eax
again:
  mov eax, 217
  mov edi, ebx
  lea rsi, [rsp+0x100]
  mov edx, 0x1000
  syscall
  test rax, rax
  jle done
  mov r12, rax
  xor r13d, r13d
inner:
  cmp r13, r12
  jae again
  lea rsi, [rsp+r13+0x113]
  movzx r14d, word ptr [rsp+r13+0x110]
  mov rdx, rsi
slen:
  cmp byte ptr [rdx], 0
  je sdone
  inc rdx
  jmp slen
sdone:
  mov byte ptr [rdx], 10
  inc rdx
  sub rdx, rsi
  mov eax, 1
  mov edi, 1
  syscall
  add r13, r14
  jmp inner
done:
  mov eax, 231
  xor edi, edi
  syscall
bad:
  mov eax, 231
  mov edi, 2
  syscall
path:
  .asciz "/"

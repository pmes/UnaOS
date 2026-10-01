.intel_syntax noprefix
.text
.globl _start
_start:
  sub rsp, 0x200
  mov ebx, 0x1234
  mov r12d, 0x5678
  mov eax, 22
  mov rdi, rsp
  syscall
  test rax, rax
  jnz fail1
  mov eax, 57
  syscall
  test eax, eax
  js fail2
  jnz parent
  cmp ebx, 0x1234
  jne cbad
  cmp r12d, 0x5678
  jne cbad
  mov eax, 1
  mov edi, [rsp+4]
  lea rsi, [rip+msg]
  mov edx, 5
  syscall
  mov eax, 231
  xor edi, edi
  syscall
cbad:
  mov eax, 231
  mov edi, 3
  syscall
parent:
  mov r13d, eax
  mov eax, 3
  mov edi, [rsp+4]
  syscall
  xor eax, eax
  mov edi, [rsp]
  lea rsi, [rsp+16]
  mov edx, 64
  syscall
  mov r14, rax
  mov eax, 61
  mov edi, r13d
  lea rsi, [rsp+8]
  xor edx, edx
  xor r10d, r10d
  syscall
  mov eax, 1
  mov edi, 1
  lea rsi, [rsp+16]
  mov rdx, r14
  syscall
  cmp r14, 5
  jne fail3
  cmp dword ptr [rsp+8], 0
  jne fail4
  mov eax, 231
  xor edi, edi
  syscall
fail1:
  mov edi, 11
  jmp die
fail2:
  mov edi, 12
  jmp die
fail3:
  mov edi, 13
  jmp die
fail4:
  mov edi, 14
die:
  mov eax, 231
  syscall
msg:
  .ascii "ping\n"

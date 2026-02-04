# Kernel Memory Model: The Air-Gap Strategy

## 1. The Virtual Address Space
unaOS utilizes the full 64-bit canonical address space (48-bit physical addressing on current x86/ARM).
* **User Space (Lower Half):** `0x0000_0000_0000_0000` to `0x0000_7FFF_FFFF_FFFF`
* **Kernel Space (Higher Half):** `0xFFFF_8000_0000_0000` to `0xFFFF_FFFF_FFFF_FFFF`
* **The Air Gap:** A non-canonical "Guard Zone" separates the two. Any attempt to jump across this void triggers an immediate General Protection Fault (#GP).

## 2. Allocation Strategy: "Ownership" in Silicon
We mirror Rust’s ownership model in the physical page tables.
* **RAII Paging:** Every memory page is an object. When a process dies, its pages are deterministically dropped. No "memory leaks" in the kernel.
* **No-Execute (NX) by Default:** All memory is marked non-executable (NX) upon allocation. Only the code loader can flip this bit, and only after signature verification.

## 3. Swap Strategy: Compression First
To protect SSD lifespan (Ecology) and performance (Speed):
* **Z-Pages:** When memory pressure rises, we do not swap to disk immediately. We compress old pages into a reserved RAM buffer (ZRAM).
* **Rationale:** Decompressing RAM is 100x faster than reading from NVMe, and it saves write cycles on the user's hardware.

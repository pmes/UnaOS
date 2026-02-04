# IPC Design: Typed Message Passing

## 1. The Problem with Unix IPC
In Unix, everything is a stream of bytes. If you send "delete file" to a pipe, you rely on the receiver parsing that text correctly. This is fragile.

## 2. The unaOS Solution: "Ports & Packets"
Inspired by Mach (macOS) and the NT Object Manager.
* **Ports:** A "Port" is a kernel-protected mailbox. You cannot fake a message from another process; the kernel stamps the sender's ID on every envelope.
* **Typed Packets:** Messages are not raw bytes; they are serialized Rust Structs (using `Serde` principles).

## 3. Zero-Copy Handoff
To solve the "Microkernel Speed Limit":
* When Process A sends a large image to Process B (e.g., a Video Player sending a frame to the Window Server), we do not copy the bits.
* **Memory Handoff:** The Kernel literally unmaps the memory page from A and maps it into B.
* **Speed:** This makes passing a 4GB movie file as fast as passing a boolean.

## 4. Interface Definition Language (JIDL)
* Services define their API in `.jidl` files (similar to Protobuf).
* The build system auto-generates the Rust structs for both the Client and the Server, ensuring type safety across process boundaries.

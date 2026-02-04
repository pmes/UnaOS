# Black Box Theory: The "Truman Show" Containment

## 1. The Isolation Principle
Proprietary applications are untrusted by default.
* **The Lie:** When a Windows app asks, "Am I on Windows 11?" we answer "Yes."
* **The Truth:** The app is running inside a lightweight, capability-restricted container. It sees a fake Registry, a fake C:\ drive, and a fake Admin account.

## 2. No Root Access, Ever
A foreign application can NEVER escalate privileges in unaOS.
* **Virtualized UID:** Even if the app thinks it is "Administrator" (UID 0) inside the box, the unaOS kernel sees it as a restricted user (UID 1000+).
* **Damage Control:** If a virus infects the "C:\" drive inside the box, we simply delete the container and respawn it. The actual host OS is untouched.

## 3. The Clean Room Wall
* **Implementation Rule:** The Black Box is built strictly via the **Two-Team Rule** (see `CLEAN_ROOM_POLICY.md`).
* **Source:** We do not copy Windows/macOS code. We observe the inputs/outputs of their syscalls and reimplement the logic in safe Rust.

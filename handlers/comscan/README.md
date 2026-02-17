# 📡 Comscan: The Radar

> *"The air is not empty. It is screaming with data, if you have the ears to hear it."*

**Comscan** is the signal analysis and hardware I/O suite of **UnaOS**. It rejects the passive "Settings Menu" approach to connectivity. Instead, it visualizes the electromagnetic spectrum and the raw hardware interfaces around you.

We do not just "pair" devices. We interrogate them.

## 🌌 The Philosophy: Active Scanning

Modern OSs hide the complexity of wireless protocols behind friendly names and loading spinners. **Comscan** exposes the raw signal strength (RSSI), the MAC addresses, and the handshake protocols.

### 1. The Spectrum (Wireless)
Comscan treats Bluetooth and WiFi as hostile environments first, and utilities second.
*   **The Waterfall:** A real-time visualizer of the 2.4GHz and 5GHz spectrum. See interference before you blame the router.
*   **The Interceptor:** View raw advertisement packets from BLE (Bluetooth Low Energy) devices. Debug your IoT hardware without a phone app.

### 2. The Hardline (Wired)
For the hardware hacker, Comscan is the ultimate serial terminal.
*   **UART / Serial:** A high-speed, low-latency terminal for talking to microcontrollers (Arduino, ESP32, STM32).
*   **Baud Rate Auto-Detect:** It guesses the speed so you don't see garbage text.
*   **Hex Dump Mode:** See the raw bytes coming over the wire alongside the ASCII interpretation.

## 🎛️ The Mechanics

### The Tuner
Comscan integrates Software Defined Radio (SDR) capabilities directly into the OS.
*   **FM/AM Demodulation:** Listen to radio.
*   **Protocol Sniffing:** Analyze sub-GHz remotes and sensors.

### The Handshake
When connecting to a device (keyboard, mouse, headphones), Comscan handles the cryptographic keys and stores them securely in **Holocron**. It ensures no Man-in-the-Middle attacks are occurring during pairing.

## 🛑 The Kill List
Comscan replaces:
*   **System Settings > Bluetooth / WiFi**
*   **PuTTY / Screen / Minicom** (Serial Terminals)
*   **Wireshark** (for Bluetooth/WiFi packet capture)
*   **GQRX / SDR#** (Software Defined Radio tools)

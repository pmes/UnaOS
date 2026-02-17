# 🗝️ Holocron: The Key

> *"A secret is only as safe as the one who guards it."*

**Holocron** is the cryptographic heart and secrets manager of **UnaOS**. It rejects the insecure, password-manager-as-an-app model. Instead, it integrates identity and encryption directly into the kernel.

**Holocron** is the only place where trust exists.

## 🛡️ The Philosophy: Zero-Knowledge

Modern systems leak secrets through memory dumps, swap files, and insecure clipboards. **Holocron** is designed to keep secrets in a dedicated, protected memory space that never touches the disk.

### 1. The Enclave (Secure Storage)
Holocron uses hardware-backed security (TPM / Secure Enclave) when available, and pure Rust cryptography (Ring / Sodium) as the bedrock.
*   **The Vault:** A single, encrypted database for all your passwords, SSH keys, API tokens, and GPG identities.
*   **Memory Hygiene:** Secrets are wiped from RAM the instant they are no longer needed. No lingering strings.

### 2. The Agent (Authentication)
Holocron replaces the fragmented mess of `ssh-agent`, `gpg-agent`, and browser autofill.
*   **Unified Identity:** One master key unlocks your SSH, GPG, and Web credentials for the session.
*   **Context-Aware Auth:** When **Midden** asks for `sudo` or **Vairë** pushes to GitHub, Holocron prompts for confirmation via a secure, out-of-band UI. It never blindly hands over keys.

## ⚙️ The Mechanics

### The Keyring
Holocron manages the lifecycle of your keys.
*   **Key Generation:** Create strong, modern keys (Ed25519) with a single command. No more confusing OpenSSL flags.
*   **Key Rotation:** Automatically remind you to rotate keys based on policy.

### The Bridge
Holocron securely injects credentials into applications without exposing the raw secret.
*   **Clipboard Protection:** If you must copy a password, Holocron clears the clipboard after 10 seconds.
*   **Browser Integration:** Safely fills forms in **Aether** without the browser ever seeing the database.

## 🛑 The Kill List
Holocron replaces:
*   **1Password / LastPass / Bitwarden**
*   **GPG Keychain / GPG Suite**
*   **ssh-agent**
*   **macOS Keychain Access / Windows Credential Manager**

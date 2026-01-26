# Legal Stance & Licensing

## The Hybrid Licensing Model
unaOS employs a strategic hybrid licensing model to balance user freedom with ecosystem compatibility.

### 1. The Core (GPLv3)
The kernel, bootloader, and core system utilities are licensed under the **GNU General Public License v3 (GPLv3)**.
* **Purpose:** To ensure the operating system itself remains permanently free and open.
* **Requirement:** Any modifications to the core OS must be open-sourced.

### 2. The Compatibility Layer (LGPLv3)
The system libraries, APIs, and the "Black Box" compatibility layer are licensed under the **GNU Lesser General Public License v3 (LGPLv3)**.
* **Purpose:** To allow non-GPL software (proprietary applications, games, legacy binaries) to run on unaOS without legal ambiguity.
* **Effect:** Proprietary applications may dynamically link to unaOS libraries without being forced to open their source code, provided they do not modify the libraries themselves.

## Patent Safety
We rely on **Prior Art Defense**. All architectural designs, emulation techniques, and API translations are published openly in this repository. By establishing a public timeline of invention, we prevent third parties from patenting these techniques subsequently.

## DMCA & Interoperability
unaOS exists for the purpose of **Interoperability**.
Under the Digital Millennium Copyright Act (DMCA) Section 1201(f), reverse engineering for the sole purpose of achieving interoperability between computer programs is a protected activity. unaOS does not circumvent access controls to facilitate infringement; it circumvents incompatibility to facilitate use.

# Building KazuOS

## Prerequisites

### Rust toolchains

Both stable and nightly are required.

```powershell
rustup toolchain install stable
rustup toolchain install nightly
```

The bootloader targets `x86_64-unknown-uefi` (stable) and the kernel uses a custom target `x86_64-kazuos` built with nightly's `-Zbuild-std`.

### QEMU

Install [QEMU for Windows](https://www.qemu.org/download/#windows) with OVMF firmware included.  
Common install path: `C:\Program Files\qemu\`

If OVMF is not found automatically, set the environment variable:

```powershell
$env:OVMF_PATH = "C:\path\to\edk2-x86_64-code.fd"
```

### xorriso (ISO mode only)

`xorriso.exe` is bundled under `tools\xorriso\`. No separate install needed.

---

## Building and running

Use `launch.ps1`. It interactively asks for boot mode, build options, and debug settings.

```powershell
.\launch.ps1
```

On first run, select:
1. **Boot mode** — `ESP (fat:rw)` is faster for development; `ISO` creates a bootable `.iso`
2. **Build** — `Build` to compile, or `Skip build` to reuse existing binaries
3. **Debug options** — `None` for normal boot

### Boot modes

| Mode | How it works |
| --- | --- |
| ESP (fat:rw) | QEMU serves `esp/` directly as a FAT drive. No ISO needed. |
| ISO | Builds `kazuos.iso` via `make_iso.ps1` then boots from it. |

### Skip build

Pass `-NoBuild` to skip the build prompt entirely:

```powershell
.\launch.ps1 -NoBuild
```

---

## Manual build commands

### Bootloader

```powershell
cargo build -p kazuos-bootloader --target x86_64-unknown-uefi --release
```

Output: `target\x86_64-unknown-uefi\release\kazuos-bootloader.efi`

### Kernel

```powershell
cargo +nightly build -p kazuos-kernel `
    --target crates/kernel/x86_64-kazuos.json `
    -Zbuild-std=core,alloc `
    -Zbuild-std-features=compiler-builtins-mem `
    -Zjson-target-spec `
    --release
```

Output: `target\x86_64-kazuos\release\kazuos-kernel`

> The kernel build script (`crates/kernel/build.rs`) also compiles all `.rs` files under
> `crates/user_programs/` into `.kxe` binaries and embeds them into the kernel as an initramfs.
> These are compiled with `rustc` directly against `x86_64-unknown-none`.

### ISO

```powershell
.\make_iso.ps1
```

Output: `kazuos.iso`

---

## ESP layout

After a build, the `esp/` directory looks like:

```
esp/
  EFI/
    BOOT/
      BOOTX64.EFI     ← bootloader
  KazuOS/
    kernel.elf         ← kernel
    font.ttf           ← optional TrueType font
```

Any `.wav` files in the project root are also copied to `esp/KazuOS/` for the audio driver.

---

## User programs

User programs live in `crates/user_programs/*.rs`. Each `.rs` file is compiled independently into a flat binary (`.kxe`) by the kernel's build script and bundled into the initramfs at `/bin/<name>.kxe`.

They are `#![no_std] #![no_main]` binaries that call the kernel via `int 0x80`. See `docs/USER_ABI.md` for the syscall interface.

---

## Troubleshooting

**`OVMF firmware not found`**  
Set `$env:OVMF_PATH` to the full path of `edk2-x86_64-code.fd`.

**`qemu-system-x86_64.exe not found`**  
Set `$env:QEMU_PATH` to the full path of `qemu-system-x86_64.exe`.

**`can't find crate for core` during kernel build**  
Make sure you are using `cargo +nightly` and have the nightly toolchain installed (`rustup toolchain install nightly`).

**`xorriso not found` during ISO build**  
`tools\xorriso\xorriso.exe` should be present. If missing, extract `tools\xorriso-win.zip`.

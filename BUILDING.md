# Building KazuOS

## Prerequisites

### Rust toolchain

The toolchain is pinned by `rust-toolchain.toml` (nightly), which also declares the
required targets (`x86_64-unknown-uefi`, `x86_64-unknown-none`) and components
(`rust-src`, `rustfmt`, `clippy`, `llvm-tools`). rustup installs all of this
automatically the first time you build — no manual `rustup` steps are needed.

The bootloader targets `x86_64-unknown-uefi`; the kernel uses a custom target
`x86_64-kazuos` built with nightly's `-Zbuild-std`.

### QEMU

Install [QEMU for Windows](https://www.qemu.org/download/#windows) with OVMF firmware included.
Common install path: `C:\Program Files\qemu\`

If QEMU or OVMF is not found automatically, set:

```powershell
$env:QEMU_PATH = "C:\path\to\qemu-system-x86_64.exe"
$env:OVMF_PATH = "C:\path\to\edk2-x86_64-code.fd"
```

### xorriso (ISO mode only)

ISO authoring needs `xorriso` (or `mkisofs` / `genisoimage`). It is **not** shipped
in the repo. `scripts/make_iso.rs` looks for it in this order:

1. `xorriso` (or `mkisofs`/`genisoimage`) on `PATH`
2. `tools\xorriso\xorriso.exe` (a local, untracked convenience location on Windows)
3. an explicit `--xorriso PATH` argument or the `XORRISO` environment variable

So either install xorriso and put it on `PATH`, or drop a Windows build at
`tools\xorriso\xorriso.exe` (this path is git-ignored).

---

## Building and running

The dev scripts are Rust [cargo-scripts](https://doc.rust-lang.org/cargo/), run via
`-Zscript`. The repo ships thin `.sh` wrappers; on Windows run them under Git Bash,
or invoke the cargo command directly in any shell.

### Run in QEMU

```bash
./run_in_qemu.sh                 # wrapper
# or, directly:
cargo +nightly -Zscript scripts/launch.rs
```

It boots KazuOS directly via its own UEFI bootloader from the `esp/` directory
(served by QEMU as a FAT drive — no ISO needed) and interactively prompts for:

1. **Build** — `Build` to compile, or `Skip build` to reuse existing binaries
2. **Debug options** — `None` for normal boot; others enable `no-reboot` /
   `no-shutdown` / an exception log (`qemu-debug.log`)
3. **Audio device** — `Intel HDA` or `None`

Pass `--no-build` to skip the build prompt:

```bash
cargo +nightly -Zscript scripts/launch.rs --no-build
```

### Build an ISO

```bash
./make_iso.sh
# or:
cargo +nightly -Zscript scripts/make_iso.rs [--output kazuos.iso]
```

Output: `kazuos.iso`

---

## Manual build commands

### Bootloader

```bash
cargo build -p kazuos-bootloader --target x86_64-unknown-uefi --release
```

Output: `target/x86_64-unknown-uefi/release/kazuos-bootloader.efi`

### Kernel

```bash
cargo +nightly build -p kazuos-kernel \
    --target crates/kernel/x86_64-kazuos.json \
    -Zbuild-std=core,alloc \
    -Zbuild-std-features=compiler-builtins-mem \
    -Zjson-target-spec \
    --release
```

Output: `target/x86_64-kazuos/release/kazuos-kernel`

> The kernel build script (`crates/kernel/build.rs`) also compiles every `.rs` file
> under `crates/user_programs/` into a flat `.kxe` binary and every `.rs` under
> `crates/user_modules/` into a `.kkm`, then bundles them into `target/initrd.kfs`
> (the initramfs). These are compiled with `rustc` directly against
> `x86_64-unknown-none`.

---

## ESP layout

`scripts/build_esp.rs` (invoked automatically by `launch.rs`) assembles the `esp/`
directory:

```
esp/
  EFI/
    BOOT/
      BOOTX64.EFI      ← KazuOS bootloader (firmware boot entry)
  KazuOS/
    kernel.elf         ← kernel
    initrd.kfs         ← initramfs (user programs + modules)
    font.ttf           ← optional TrueType font (copied if font.ttf exists in repo root)
```

---

## User programs and modules

User programs live in `crates/user_programs/*.rs`; each is compiled independently
into a flat `.kxe` binary and bundled into the initramfs at `/bin/<name>.kxe`.
Kernel modules live in `crates/user_modules/*.rs`, compiled into `.kkm` and placed
under `/modules/`.

Both are `#![no_std] #![no_main]` binaries that run in ring3 and call the kernel via
`int 0x80`. See `docs/USER_ABI.md` for the syscall interface and
`docs/ARCHITECTURE.md` for the driver policy.

---

## Troubleshooting

**`qemu-system-x86_64 not found`**
Set `$env:QEMU_PATH` to the full path of `qemu-system-x86_64.exe`.

**`OVMF firmware not found`**
Set `$env:OVMF_PATH` to the full path of the OVMF code firmware (e.g. `edk2-x86_64-code.fd`).

**`can't find crate for core` during kernel build**
Make sure the nightly toolchain and `rust-src` component are present. They are
declared in `rust-toolchain.toml`, so running any `cargo` command in the repo root
should install them automatically.

**`No ISO authoring tool found` during ISO build**
Install `xorriso` (or `mkisofs`/`genisoimage`) and put it on `PATH`, set `XORRISO`
to its full path, or place a Windows build at `tools\xorriso\xorriso.exe`.

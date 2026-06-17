# KXE User Programs and Drivers

KXE is KazuOS's native executable format. All user programs are compiled as KXE binaries and embedded into the initramfs at build time.

## Anatomy of a KXE Binary

```
offset  size  field
 0       4    magic      "KXE\0"
 8       8    entry       virtual entry point (0 = use raw binary address)
16       8    code_offset offset to code from file start
24       8    code_size   size of code in bytes
32       4    flags       0=user, 1=driver
36       4    reserved
36+     ...   code        raw binary (starts at offset 36)
```

The kernel loads the KXE at `USER_BASE` (0x8000000000), applies `R_X86_64_RELATIVE` relocations (all fixed up to `USER_BASE`), and jumps to `_start`.

## Writing a User Program

All user programs are `.rs` files in `crates/user_programs/`. They are compiled automatically by `build.rs` during kernel build.

### Minimal User Program

```rust
#![no_std]
#![no_main]

include!("../../crates/kazuos_abi/src/syscall_numbers.rs");

#[no_mangle]
pub extern "C" fn _start(_argc: u64, _argv: u64) -> ! {
    syscall(SYS_CONSOLE_WRITE, "Hello\n".as_ptr() as u64, 6, 0);
    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
}

fn syscall(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
        );
    }
    r
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
```

### Requirements

- `#![no_std]` and `#![no_main]` — no standard library or runtime
- `_start(argc: u64, argv: u64)` as the entry point (set by `link.ld`)
- `include!("../../crates/kazuos_abi/src/syscall_numbers.rs")` for syscall constants
- `#[panic_handler]` — must be present

### Build Integration

`build.rs` automatically picks up any `.rs` file in `crates/user_programs/` (except `syscall_numbers.rs`). It:

1. Compiles each `.rs` with `rustc` targeting `x86_64-unknown-none`
2. Links with `crates/user_programs/link.ld` (code at virtual address 0)
3. Extracts the raw binary via `objcopy`
4. Patches `R_X86_64_RELATIVE` relocations to `USER_BASE`
5. Wraps it in a KXE header
6. Embeds the KXE as a `pub const PROGRAM_KXE: &[u8]` in `user_programs_generated.rs`
7. Builds `initrd.kfs` containing all KXE binaries

### syscall_numbers.rs

All syscall constants live in `crates/kazuos_abi/src/syscall_numbers.rs`. This file is the single source of truth, included via `include!()` by both the kernel and all user programs. Never hardcode syscall numbers in user programs.

### Multiple syscall arguments

Different user programs define different convenience wrappers. Common patterns:

```rust
fn syscall0(n: u64) -> u64 { /* rax=n, rdi=rsi=rdx=0 */ }
fn syscall1(n: u64, a0: u64) -> u64 { /* rdi=a0 */ }
fn syscall2(n: u64, a0: u64, a1: u64) -> u64 { /* rdi=a0, rsi=a1 */ }
fn syscall3(n: u64, a0: u64, a1: u64) -> u64 { /* rdi=a0, rsi=a1, rdx=0 */ }
fn syscall4(n: u64, a0: u64, a1: u64, a2: u64) -> u64 { /* rdi=a0, rsi=a1, rdx=a2 */ }
```

## Writing a Driver

Drivers follow the same pattern as user programs but with additional privileges.

### Driver Flag

Name the file `drv_*.rs` (e.g. `drv_ac97.rs`). `build.rs` detects the `drv_` prefix and sets `flags=1` in the KXE header, marking it as a driver.

### Driver Privileges

Driver processes can call:

- `SYS_IOPORT_REQUEST` — allow port I/O access
- `SYS_IRQ_WAIT` — block and wait for hardware IRQs
- `SYS_DMA_ALLOC` / `SYS_DMA_FREE` — allocate physically-contiguous DMA memory

These syscalls return `u64::MAX` for non-driver processes.

### Driver Lifecycle

- Drivers are spawned automatically at boot by `exec::spawn_driver()`
- **Cannot be killed** — `SYS_KILL` and Ctrl+C both refuse to terminate a driver
- Drivers typically run an infinite loop waiting for IPC requests or IRQs

### Minimal Driver

```rust
#![no_std]
#![no_main]

include!("../../crates/kazuos_abi/src/syscall_numbers.rs");

#[no_mangle]
pub extern "C" fn _start(_argc: u64, _argv: u64) -> ! {
    // Register an IPC channel for service requests
    let ch = syscall(SYS_IPC_OPEN, "myservice\0".as_ptr() as u64, 9, 0);
    if ch == u64::MAX { loop {} }

    loop {
        let mut buf = [0u8; 4096];
        let n = syscall4(SYS_IPC_RECV, ch, buf.as_mut_ptr() as u64, 4096);
        if n != u64::MAX {
            // Handle request in buf[..n]
        }
    }
}

fn syscall(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
        );
    }
    r
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
```

## Framebuffer Access

Programs can draw pixels directly using these syscalls:

1. `SYS_FB_ACQUIRE` — gain exclusive access (auto-clears to black, maps FB into process address space)
2. `SYS_FB_RELEASE` — release ownership (auto-restores the saved shell screen on exit)

See `USER_ABI.md` for details on `FbInfo` layout.

### Example: Drawing a Pixel

```rust
// Acquire the framebuffer
let mut info = FbInfo { base: 0, width: 0, height: 0, stride: 0, format: 0 };
syscall(SYS_FB_ACQUIRE, &mut info as *mut _ as u64, 0, 0);

let fb = info.base as *mut u32;

// Draw a red pixel at (100, 100)
let stride = info.stride as usize;
let pixel = if info.format == 0 { 0x0000FF } else { 0xFF0000 }; // RGB vs BGR
unsafe { *fb.add(100 * stride + 100) = pixel; }
```

## Shell Integration

The shell automatically routes unknown commands to `/bin/<name>.kxe`. So a file named `myapp.rs` becomes `/bin/myapp.kxe` and can be launched by typing `myapp` in the shell.

Pipes work: `prog1 | prog2`

Background execution: `prog &`

## Common Pitfalls

- **No heap by default**: User programs run in ring 3 with no `alloc` crate. Use `SYS_HEAP_ALLOC`/`SYS_HEAP_FREE` for dynamic memory, or use statics.
- **No floating point**: The kernel does not save/restore FPU state across context switches.
- **No panic unwind**: Panics enter an infinite loop (`loop {}`). All panics are fatal to the process.
- **Syscall argument limits**: User programs define their own inline `syscall` helpers. There is no libc.
- **Relocation**: The binary is position-independent and relocated at load time. Static addresses in code are patched to `USER_BASE` (0x8000000000) — the linker map uses a zero base.

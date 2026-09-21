# GUI Client Protocol

KazuOS GUI clients are independent ring3 processes. The `gui` compositor exclusively owns the physical framebuffer, window chrome, focus, stacking, dragging, and taskbar. A client draws only into two shared-memory pixel buffers created and mapped by the compositor.

## Transport

The compositor opens the named `gui-control` channel. Any ordinary process can discover the live compositor PID with `SYS_PROCESS_NEXT`/`SYS_PROCESS_INFO`, open that channel, and send Connect with `SYS_IPC_TRY_SEND_TO`. The built-in launcher only executes the selected client KXE; it does not create private pipes or provision buffers.

GUI traffic uses directed named IPC. Send buffers have an 8-byte target-PID prefix and receive buffers get an 8-byte kernel-stamped sender-PID prefix. The compositor always uses that authenticated sender PID as the client identity; no PID from protocol payload is trusted. Kernel target filtering prevents a process from receiving another client's replies. Directed queues are bounded and never evict messages: send returns `0` when full, and GUI peers retry where appropriate or disconnect rather than silently dropping protocol state.

Protocol payloads are one fixed 64-byte IPC message each. Integers are little-endian.

Every message starts with:

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 4 | magic `KGUI` (`u32::from_le_bytes(*b"KGUI")`) |
| 4 | 2 | protocol version, currently `2` |
| 6 | 2 | message type |
| 8 | 56 | type-specific payload, zero-filled when unused |

The shared definitions and decoder are in `userspace/runtime/gui_protocol.rs`.

## Messages

| Type | Direction | Payload |
| ---: | --- | --- |
| 1 Connect | client to compositor | requested `u32 width, height` at offsets 8 and 12; `u8 title_len` at 16 and printable ASCII title at 17 |
| 2 Init | compositor to client | `u32 width, height, stride, format` at offsets 8..23; two `u64` SHM IDs at offsets 24 and 32 |
| 3 Commit | client to compositor | `u8 buffer` at 8; `u32 x, y, width, height` at 12..27 |
| 4 BufferRelease | compositor to client | `u8 buffer` at 8 |
| 5 Mouse | compositor to client | `i32 x, y`, `u32 buttons, changed` at 8..23, in body-local coordinates |
| 6 Key | compositor to client | `u8 code`, `u8 released` at 8 and 9 |
| 7 Focus | compositor to client | `u8 focused` at 8 |
| 8 CloseRequest | compositor to client | no payload |
| 9 Destroy | client to compositor | no payload |
| 10 StatsRequest | client to compositor | no payload |
| 11 StatsResponse | compositor to client | seven compositor counters as `u64` values at offsets 8..63 |

Formats `0` and `1` are packed RGBX8888 and BGRX8888 respectively. The current compositor uses a tightly packed stride equal to width.

## Buffer ownership

After Init, both buffers belong to the client. The client may render into an available buffer and Commit it. A successful Commit transfers that buffer to the compositor; the client must not modify it. When a different buffer is committed, the compositor switches the displayed front buffer and sends BufferRelease for the old one. That release transfers the old buffer back to the client.

The compositor validates the protocol header, message direction, buffer index, ownership state, and overflow-safe damage bounds. A malformed client is disconnected and killed. Every front-buffer switch currently repaints the full surface because the two buffers are not required to preserve identical pixels outside the submitted damage rectangle. The validated damage fields are reserved for a future copy-forward or retained-surface optimization.

The compositor reads SHM through non-owning raw views and never constructs an owning `Vec` from a shared address. It owns the SHM object references and mappings for the window lifetime.

## Input and lifecycle

Mouse button, captured motion, key press/release, and focus transitions are directed only to the authenticated client PID. The compositor owns title-bar close handling: it sends CloseRequest and allows the client to answer with Destroy. An unresponsive client is killed after a bounded grace period.

Destroy, malformed input, directed-transport failure, compositor shutdown, and close timeout all release both compositor SHM references. The compositor accepts at most 32 directed requests per frame, at most 8 external clients, and Connect sizes from 64x64 through 1024x768. Client exit cleanup removes its grants, mappings, channel references, and queued messages targeting it. Because `gui` is single-threaded, explicit SHM close satisfies the current no-cross-CPU-TLB-shootdown restriction.

`guidemo.kxe` is the reference client. `taskmgr.kxe`, `terminal.kxe`, and `profiler.kxe` use the same shared client helper. They can be run from the built-in launcher, the console shell, or Terminal. A client discovers `/bin/gui.kxe`, retries Connect and Init only for bounded intervals, and exits nonzero when the compositor is absent or unreachable; GUI Demo also prints `guidemo: gui is not running`. `gui --test-client`, `--test-taskmgr`, `--test-terminal`, and `--test-profiler` exercise their ordinary lifecycle paths. The Terminal test also validates ANSI clear/home behavior, exercises Ctrl+C at a nested prompt and in a nested foreground command, exits the nested shell with Ctrl+D, force-kills the root shell, and verifies that the Terminal client window is removed cleanly. `gui --test-many` starts twelve GUI Demo clients concurrently to cover window-count and cleanup regressions.

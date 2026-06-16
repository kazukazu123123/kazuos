#![no_std]

//! KazuOS user/kernel ABI: the stable contract between the kernel and
//! user-space programs (syscall numbers and their associated constants).
//!
//! This is the single source of truth for the ABI. The kernel depends on this
//! crate via Cargo (`use kazuos_abi::*`). Programs and runtimes that are
//! compiled standalone (not as workspace crates) instead pull the same
//! definitions in with `include!(".../kazuos_abi/src/syscall_numbers.rs")`,
//! so there is exactly one place to edit when adding or renumbering a syscall.

include!("syscall_numbers.rs");

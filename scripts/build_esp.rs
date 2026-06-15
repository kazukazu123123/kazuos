---
[package]
name = "build_esp"
edition = "2024"
[dependencies]
---
//! Build a UEFI ESP directory that boots KazuOS directly via its own bootloader.
//!
//! Run from the repository root:
//!   cargo +nightly -Zscript scripts/build_esp.rs \
//!       --esp-dir esp \
//!       --bootloader target/x86_64-unknown-uefi/release/kazuos-bootloader.efi \
//!       --kernel    target/x86_64-kazuos/release/kazuos-kernel \
//!       --initrd    target/initrd.kfs \
//!       [--font font.ttf]
//!
//! Layout produced under <esp-dir>:
//!   EFI/BOOT/BOOTX64.EFI               the KazuOS bootloader (firmware boot entry)
//!   KazuOS/kernel.elf, initrd.kfs, font.ttf   loaded by the bootloader
//!
//! The bootloader reads \KazuOS\* from the volume it was loaded from (this ESP).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("build_esp: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    repo_root()?;

    require(&args.bootloader, "Bootloader")?;
    require(&args.kernel, "Kernel")?;
    require(&args.initrd, "initrd.kfs")?;

    let esp = &args.esp_dir;
    if esp.exists() {
        fs::remove_dir_all(esp).map_err(|e| format!("clean {}: {e}", esp.display()))?;
    }
    for d in ["EFI/BOOT", "KazuOS"] {
        fs::create_dir_all(esp.join(d)).map_err(|e| format!("mkdir {d}: {e}"))?;
    }

    // The KazuOS bootloader is the firmware boot entry.
    copy(&args.bootloader, &esp.join("EFI/BOOT/BOOTX64.EFI"))?;

    // Payload the bootloader loads from the ESP root.
    copy(&args.kernel, &esp.join("KazuOS/kernel.elf"))?;
    copy(&args.initrd, &esp.join("KazuOS/initrd.kfs"))?;
    if let Some(font) = &args.font {
        if font.exists() {
            copy(font, &esp.join("KazuOS/font.ttf"))?;
        }
    }

    println!("  ESP ready at {} (boot: EFI/BOOT/BOOTX64.EFI = KazuOS bootloader)", esp.display());
    Ok(())
}

struct Args {
    esp_dir: PathBuf,
    bootloader: PathBuf,
    kernel: PathBuf,
    initrd: PathBuf,
    font: Option<PathBuf>,
}

impl Args {
    fn parse() -> Result<Args, String> {
        let mut esp_dir = None;
        let mut bootloader = None;
        let mut kernel = None;
        let mut initrd = None;
        let mut font = None;
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            let val = |it: &mut dyn Iterator<Item = String>| {
                it.next().ok_or_else(|| format!("{a} requires a value"))
            };
            match a.as_str() {
                "--esp-dir" => esp_dir = Some(PathBuf::from(val(&mut it)?)),
                "--bootloader" => bootloader = Some(PathBuf::from(val(&mut it)?)),
                "--kernel" => kernel = Some(PathBuf::from(val(&mut it)?)),
                "--initrd" => initrd = Some(PathBuf::from(val(&mut it)?)),
                "--font" => font = Some(PathBuf::from(val(&mut it)?)),
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(Args {
            esp_dir: esp_dir.ok_or("--esp-dir is required")?,
            bootloader: bootloader.ok_or("--bootloader is required")?,
            kernel: kernel.ok_or("--kernel is required")?,
            initrd: initrd.ok_or("--initrd is required")?,
            font,
        })
    }
}

fn repo_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    if !cwd.join("crates").is_dir() {
        return Err(format!(
            "expected to run from the repository root (no 'crates' dir in {}).",
            cwd.display()
        ));
    }
    Ok(cwd)
}

fn require(p: &Path, what: &str) -> Result<(), String> {
    if p.exists() {
        Ok(())
    } else {
        Err(format!("{what} not found: {}", p.display()))
    }
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))
}

---
[package]
name = "launch"
edition = "2024"
[dependencies]
---
//! Interactive QEMU launcher for KazuOS (boots directly via its own bootloader).
//!
//! Run from the repository root:
//!   cargo +nightly -Zscript scripts/launch.rs [--no-build]
//!
//! Prompts for build / debug / audio options, then starts QEMU in the
//! foreground with the serial console on stdio.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("launch: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let root = repo_root()?;
    let no_build_flag = std::env::args().any(|a| a == "--no-build");

    let do_build = if no_build_flag {
        false
    } else {
        menu("Build", &["Build", "Skip build (use existing binaries)"])? == 0
    };

    let debug_choice = menu(
        "Debug options",
        &[
            "None          -- normal boot",
            "no-reboot     -- halt on triple fault instead of rebooting",
            "no-reboot + no-shutdown -- keep QEMU paused after fault",
            "Full          -- no-reboot + no-shutdown + exception log",
        ],
    )?;

    let audio_choice = menu(
        "Audio device",
        &[
            "Intel HDA   -- Intel High Definition Audio",
            "None        -- no audio device",
        ],
    )?;

    let ovmf = find_ovmf()?;
    let qemu = find_qemu()?;
    let ovmf_dir = ovmf.parent().ok_or("bad OVMF path")?;
    let ovmf_vars = find_first(&[
        ovmf_dir.join("edk2-i386-vars.fd"),
        ovmf_dir.join("edk2-x86_64-vars.fd"),
        ovmf_dir.join("OVMF_VARS.fd"),
        ovmf_dir.join("OVMF_VARS.4m.fd"),
    ]);

    let esp_dir = root.join("esp");
    if do_build {
        println!("Building bootloader...");
        cargo_build(&["build", "-p", "kazuos-bootloader", "--target", "x86_64-unknown-uefi", "--release"])?;
        println!("Building kernel...");
        cargo_build(&[
            "build", "-p", "kazuos-kernel",
            "--target", "crates/kernel/x86_64-kazuos.json",
            "-Zbuild-std=core,alloc", "-Zbuild-std-features=compiler-builtins-mem", "-Zjson-target-spec",
            "--release",
        ])?;
        println!("Preparing ESP...");
        build_esp(&root, &esp_dir)?;
    } else if !esp_dir.exists() {
        return Err("ESP directory not found: esp (build first)".into());
    }

    let temp_vars = root.join("ovmf_vars.tmp.fd");
    if let Some(v) = &ovmf_vars {
        copy(v, &temp_vars)?;
    }

    let audiodev_backend = if cfg!(unix) { "pa" } else { "dsound" };

    let mut args: Vec<String> = vec![
        "-machine".into(), "q35,pcspk-audiodev=snd0".into(),
        "-drive".into(), format!("if=pflash,format=raw,readonly=on,file={}", ovmf.display()),
    ];
    if ovmf_vars.is_some() {
        args.push("-drive".into());
        args.push(format!("if=pflash,format=raw,file={}", temp_vars.display()));
    }
    args.extend([
        "-drive".into(), format!("format=raw,file=fat:rw:{}", esp_dir.display()),
        "-boot".into(), "order=a,menu=on".into(),
        "-m".into(), "1G".into(),
        "-net".into(), "none".into(),
        "-device".into(), "VGA".into(),
        "-audiodev".into(), format!("{audiodev_backend},id=snd0"),
        "-serial".into(), "stdio".into(),
        "-smp".into(), "4".into(),
    ]);
    match audio_choice {
        0 => args.extend(["-device".into(), "intel-hda".into(), "-device".into(), "hda-duplex,audiodev=snd0".into()]),
        _ => {}
    }
    if debug_choice >= 1 {
        args.push("-no-reboot".into());
    }
    if debug_choice >= 2 {
        args.push("-no-shutdown".into());
    }
    if debug_choice >= 3 {
        let qemu_log = root.join("qemu-debug.log");
        args.extend(["-d".into(), "int,guest_errors".into(), "-D".into(), qemu_log.display().to_string()]);
        println!("Exception log: {}", qemu_log.display());
    }

    println!("Starting QEMU...");
    println!("  {} {}", qemu.display(), args.join(" "));
    println!();
    let status = Command::new(&qemu).args(&args).status().map_err(|e| format!("run QEMU: {e}"))?;
    if !status.success() {
        eprintln!("QEMU exited with status {status}");
    }
    Ok(())
}

/// Print a numbered menu and return the chosen 0-based index.
fn menu(title: &str, options: &[&str]) -> Result<usize, String> {
    println!();
    println!("  {title}");
    for (i, opt) in options.iter().enumerate() {
        println!("    [{}] {opt}", i + 1);
    }
    let stdin = std::io::stdin();
    loop {
        print!("  Select: ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("no input (EOF)".into());
        }
        if let Ok(n) = line.trim().parse::<usize>() {
            if n >= 1 && n <= options.len() {
                return Ok(n - 1);
            }
        }
    }
}

fn build_esp(root: &Path, esp_dir: &Path) -> Result<(), String> {
    let status = Command::new("cargo")
        .args(["+nightly", "-Zscript", "scripts/build_esp.rs"])
        .arg("--esp-dir").arg(esp_dir)
        .arg("--bootloader").arg(root.join("target/x86_64-unknown-uefi/release/kazuos-bootloader.efi"))
        .arg("--kernel").arg(root.join("target/x86_64-kazuos/release/kazuos-kernel"))
        .arg("--initrd").arg(root.join("target/initrd.kfs"))
        .arg("--font").arg(root.join("font.ttf"))
        .status()
        .map_err(|e| format!("run build_esp.rs: {e}"))?;
    if !status.success() {
        return Err("build_esp.rs failed".into());
    }
    Ok(())
}

fn cargo_build(args: &[&str]) -> Result<(), String> {
    let status = Command::new("cargo").arg("+nightly").args(args).status().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("cargo build failed".into());
    }
    Ok(())
}

fn find_ovmf() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("OVMF_PATH") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }
    find_first(&[
        // Linux (Arch / Debian / Fedora)
        PathBuf::from("/usr/share/edk2/x64/OVMF_CODE.4m.fd"),
        PathBuf::from("/usr/share/OVMF/x64/OVMF_CODE.4m.fd"),
        PathBuf::from("/usr/share/ovmf/x64/OVMF_CODE.4m.fd"),
        PathBuf::from("/usr/share/edk2-ovmf/x64/OVMF_CODE.4m.fd"),
        PathBuf::from("/usr/share/edk2/x64/OVMF_CODE.fd"),
        PathBuf::from("/usr/share/OVMF/OVMF_CODE.fd"),
        PathBuf::from("/usr/share/ovmf/OVMF.fd"),
        // Windows
        PathBuf::from(r"C:\Program Files\qemu\share\edk2-x86_64-code.fd"),
        PathBuf::from(r"C:\Program Files\qemu\share\qemu\edk2-x86_64-code.fd"),
        PathBuf::from(r"C:\Program Files\qemu\share\ovmf-x64\OVMF_CODE.fd"),
        PathBuf::from(r"C:\msys64\usr\share\qemu\edk2-x86_64-code.fd"),
    ])
    .ok_or_else(|| "OVMF firmware not found (set OVMF_PATH)".into())
}

fn find_qemu() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("QEMU_PATH") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }
    // Try PATH lookup on Unix
    if cfg!(unix) {
        if let Ok(out) = Command::new("which").arg("qemu-system-x86_64").output() {
            if out.status.success() {
                let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
                if path.exists() {
                    return Ok(path);
                }
            }
        }
    }
    find_first(&[
        // Linux common paths
        PathBuf::from("/usr/bin/qemu-system-x86_64"),
        PathBuf::from("/usr/local/bin/qemu-system-x86_64"),
        // Windows
        PathBuf::from(r"C:\Program Files\qemu\qemu-system-x86_64.exe"),
        PathBuf::from(r"C:\msys64\mingw64\bin\qemu-system-x86_64.exe"),
    ])
    .ok_or_else(|| "qemu-system-x86_64 not found (set QEMU_PATH)".into())
}

fn find_first(cands: &[PathBuf]) -> Option<PathBuf> {
    cands.iter().find(|p| p.exists()).cloned()
}

fn repo_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    if !cwd.join("crates").is_dir() {
        return Err(format!("expected to run from the repository root (no 'crates' dir in {}).", cwd.display()));
    }
    Ok(cwd)
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to).map(|_| ()).map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))
}

use std::fs;

---
[package]
name = "make_iso"
edition = "2024"
[dependencies]
fatfs = "0.3"
---
//! Build a UEFI-bootable ISO that boots KazuOS directly via its own bootloader.
//!
//! Run from the repository root:
//!   cargo +nightly -Zscript scripts/make_iso.rs [--output kazuos.iso] [--xorriso PATH]
//!
//! Boot flow (no Limine):
//!   firmware -> El Torito UEFI boot image (a FAT ESP)
//!            -> EFI/BOOT/BOOTX64.EFI = the KazuOS bootloader
//!            -> reads \KazuOS\kernel.elf / initrd.kfs / font.ttf from the SAME
//!               FAT volume (OVMF exposes FAT as a UEFI SimpleFileSystem; it does
//!               NOT do so for plain ISO9660, which is why everything lives on the
//!               embedded FAT image rather than the ISO9660 tree).
//!
//! The FAT image is built in-process with the `fatfs` crate (no mtools / no
//! hand-rolled FAT). Requires xorriso (preferred) or mkisofs/genisoimage for the
//! final ISO9660 wrapper; a bundled Windows xorriso under tools/xorriso is used
//! automatically if present.

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("make_iso: error: {e}");
            ExitCode::FAILURE
        }
    }
}

enum IsoTool {
    Xorriso(PathBuf),
    Mkisofs(PathBuf),
}

fn run() -> Result<(), String> {
    let root = repo_root()?;
    let mut output = root.join("kazuos.iso");
    let mut xorriso_override: Option<String> = std::env::var("XORRISO").ok().filter(|s| !s.is_empty());
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--output" => output = PathBuf::from(it.next().ok_or("--output requires a value")?),
            "--xorriso" => xorriso_override = Some(it.next().ok_or("--xorriso requires a value")?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let tool = find_iso_tool(xorriso_override)?;
    match &tool {
        IsoTool::Xorriso(p) => println!("ISO tool: {} (xorriso)", p.display()),
        IsoTool::Mkisofs(p) => println!("ISO tool: {} (mkisofs)", p.display()),
    }

    println!("Building bootloader...");
    cargo_build(&["build", "-p", "kazuos-bootloader", "--target", "x86_64-unknown-uefi", "--release"])?;
    println!("Building kernel...");
    cargo_build(&[
        "build", "-p", "kazuos-kernel",
        "--target", "crates/kernel/x86_64-kazuos.json",
        "-Zbuild-std=core,alloc", "-Zbuild-std-features=compiler-builtins-mem", "-Zjson-target-spec",
        "--release",
    ])?;

    let bootloader = root.join("target/x86_64-unknown-uefi/release/kazuos-bootloader.efi");
    let kernel = root.join("target/x86_64-kazuos/release/kazuos-kernel");
    let initrd = root.join("target/initrd.kfs");
    let font = root.join("font.ttf");
    require(&bootloader, "bootloader")?;
    require(&kernel, "kernel")?;
    require(&initrd, "initrd.kfs")?;

    // Files to place on the FAT ESP image: (path-in-fat, host-file).
    let mut files: Vec<(&str, PathBuf)> = vec![
        ("EFI/BOOT/BOOTX64.EFI", bootloader),
        ("KazuOS/kernel.elf", kernel),
        ("KazuOS/initrd.kfs", initrd),
    ];
    if font.exists() {
        files.push(("KazuOS/font.ttf", font));
    }

    let iso_root = root.join("iso_root");
    if iso_root.exists() {
        fs::remove_dir_all(&iso_root).map_err(|e| format!("clean iso_root: {e}"))?;
    }
    fs::create_dir_all(&iso_root).map_err(|e| format!("mkdir iso_root: {e}"))?;

    println!("Building FAT ESP image...");
    let efi_img = iso_root.join("efi.img");
    build_fat_image(&efi_img, &files)?;

    if output.exists() {
        fs::remove_file(&output).map_err(|e| format!("remove old iso: {e}"))?;
    }
    println!("Creating ISO...");
    author_iso(&tool, &iso_root, &output)?;
    println!("Created {}", output.display());
    Ok(())
}

/// Create a FAT filesystem image containing `files` (each at its given path).
fn build_fat_image(img_path: &Path, files: &[(&str, PathBuf)]) -> Result<(), String> {
    // Size = payload + generous slack, so the FAT metadata always fits.
    let payload: u64 = files
        .iter()
        .map(|(_, p)| fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum();
    let size = (payload + 8 * 1024 * 1024).next_multiple_of(512);

    let mut img = fs::OpenOptions::new()
        .read(true).write(true).create(true).truncate(true)
        .open(img_path)
        .map_err(|e| format!("create {}: {e}", img_path.display()))?;
    img.set_len(size).map_err(|e| e.to_string())?;
    img.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;

    fatfs::format_volume(&mut img, fatfs::FormatVolumeOptions::new().volume_label(*b"KAZUOS     "))
        .map_err(|e| format!("format FAT: {e}"))?;
    let fs = fatfs::FileSystem::new(&mut img, fatfs::FsOptions::new())
        .map_err(|e| format!("open FAT: {e}"))?;

    {
        let root = fs.root_dir();
        for (fat_path, host) in files {
            let bytes = fs::read(host).map_err(|e| format!("read {}: {e}", host.display()))?;
            // Create any parent directories, then write the file.
            let mut dir = root.clone();
            let parts: Vec<&str> = fat_path.split('/').collect();
            for comp in &parts[..parts.len() - 1] {
                dir = dir.create_dir(comp).map_err(|e| format!("mkdir {comp}: {e}"))?;
            }
            let mut f = dir
                .create_file(parts[parts.len() - 1])
                .map_err(|e| format!("create {fat_path}: {e}"))?;
            f.truncate().map_err(|e| e.to_string())?;
            f.write_all(&bytes).map_err(|e| format!("write {fat_path}: {e}"))?;
        }
    }
    fs.unmount().map_err(|e| format!("unmount FAT: {e}"))?;
    Ok(())
}

fn find_iso_tool(override_path: Option<String>) -> Result<IsoTool, String> {
    if let Some(p) = override_path {
        return Ok(IsoTool::Xorriso(PathBuf::from(p)));
    }
    if let Some(p) = which("xorriso") {
        return Ok(IsoTool::Xorriso(p));
    }
    if let Some(p) = find_under(&PathBuf::from("tools").join("xorriso"), "xorriso.exe") {
        return Ok(IsoTool::Xorriso(p));
    }

    if let Some(p) = which("mkisofs").or_else(|| which("genisoimage")) {
        return Ok(IsoTool::Mkisofs(p));
    }
    Err("No ISO authoring tool found. Install xorriso (set XORRISO) or mkisofs/genisoimage.".into())
}

fn author_iso(tool: &IsoTool, iso_root: &Path, output: &Path) -> Result<(), String> {
    let status = match tool {
        IsoTool::Xorriso(p) => {
            let iso_root_c = to_cygwin(iso_root);
            let output_c = to_cygwin(output);
            let efi_img_c = to_cygwin(&iso_root.join("efi.img"));
            // Hybrid ISO: besides the El Torito UEFI entry, expose the FAT boot
            // image as a real GPT EFI System Partition with a protective MBR.
            // This makes the medium bootable by firmwares that ignore El Torito
            // EFI and instead look for a GPT ESP (e.g. VirtualBox).
            Command::new(p)
                .args([
                    "-as", "mkisofs", "-R", "-r", "-J", "-V", "KAZUOS",
                    "-eltorito-platform", "efi", "-b", "efi.img", "-no-emul-boot",
                    "-appended_part_as_gpt",
                    "-append_partition", "2",
                    "c12a7328-f81f-11d2-ba4b-00a0c93ec93b", &efi_img_c,
                    "-o", &output_c, &iso_root_c,
                ])
                .status()
        }
        IsoTool::Mkisofs(p) => Command::new(p)
            .args([
                "-R", "-J", "-V", "KAZUOS",
                "-eltorito-platform", "efi", "-eltorito-boot", "efi.img", "-no-emul-boot",
                "-o", &output.to_string_lossy(), &iso_root.to_string_lossy(),
            ])
            .status(),
    }
    .map_err(|e| format!("run ISO tool: {e}"))?;
    if !status.success() {
        return Err("ISO creation failed.".into());
    }
    Ok(())
}

fn find_under(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.file_name().and_then(|s| s.to_str()) == Some(name) {
            return Some(p);
        }
    }
    for d in subdirs {
        if let Some(found) = find_under(&d, name) {
            return Some(found);
        }
    }
    None
}

fn to_cygwin(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if let Some((drive, rest)) = s.split_once(":/") {
        if drive.len() == 1 {
            return format!("/cygdrive/{}/{}", drive.to_ascii_lowercase(), rest);
        }
    }
    s
}

fn cargo_build(args: &[&str]) -> Result<(), String> {
    let status = Command::new("cargo").arg("+nightly").args(args).status().map_err(|e| format!("run cargo: {e}"))?;
    if !status.success() {
        return Err("cargo build failed".into());
    }
    Ok(())
}

fn which(name: &str) -> Option<PathBuf> {
    let exts = if cfg!(windows) { vec![".exe", ""] } else { vec![""] };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn repo_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    if !cwd.join("crates").is_dir() {
        return Err(format!("expected to run from the repository root (no 'crates' dir in {}).", cwd.display()));
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

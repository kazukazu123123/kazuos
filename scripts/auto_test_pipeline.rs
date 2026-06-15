---
[package]
name = "auto_test_pipeline"
edition = "2024"
[dependencies]
---
//! Automated boot/test pipeline: build KazuOS, boot it in QEMU, drive the shell
//! over the QEMU monitor, and check the serial log. Headless.
//!
//! Run from the repository root, e.g.:
//!   cargo +nightly -Zscript scripts/auto_test_pipeline.rs -- \
//!       --cpu-count 4 --send "ls /bin" --expect "file /bin/shell.kxe"
//!
//! Flags (all optional):
//!   --boot-timeout N     seconds to wait for boot patterns (default 30)
//!   --after-wait N       seconds to keep running after sending input (default 6)
//!   --no-build           reuse the existing esp/ instead of rebuilding
//!   --keep-alive         do not quit QEMU at the end
//!   --verbose            pick the verbose boot menu entry
//!   --send LINE          input line to type (repeatable); "ret" presses Enter
//!   --expect PATTERN     substring required in serial.log for success
//!   --wait-pattern S     serial substring marking shell readiness
//!                        (default "KazuOS kernel started")
//!   --early-pattern S    serial substring marking the bootloader
//!                        (default "KazuOS Bootloader")
//!   --audio ac97|hda|none   audio device (default ac97)
//!   --cpu-count N        number of CPUs (default 2)

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const MONITOR_PORT: u16 = 55555;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(e) => {
            eprintln!("auto_shell_qemu: error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

struct Cfg {
    boot_timeout: u64,
    after_wait: u64,
    no_build: bool,
    keep_alive: bool,
    verbose: bool,
    send_lines: Vec<String>,
    expect: Option<String>,
    wait_pattern: String,
    early_pattern: String,
    audio: String,
    cpu_count: u32,
}

fn parse_cfg() -> Result<Cfg, String> {
    let mut c = Cfg {
        boot_timeout: 30,
        after_wait: 6,
        no_build: false,
        keep_alive: false,
        verbose: false,
        send_lines: Vec::new(),
        expect: None,
        wait_pattern: "KazuOS kernel started".into(),
        early_pattern: "KazuOS Bootloader".into(),
        audio: "ac97".into(),
        cpu_count: 2,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{a} requires a value"));
        match a.as_str() {
            "--boot-timeout" => c.boot_timeout = val()?.parse().map_err(|_| "bad --boot-timeout")?,
            "--after-wait" => c.after_wait = val()?.parse().map_err(|_| "bad --after-wait")?,
            "--no-build" => c.no_build = true,
            "--keep-alive" => c.keep_alive = true,
            "--verbose" => c.verbose = true,
            "--send" => c.send_lines.push(val()?),
            "--expect" => c.expect = Some(val()?),
            "--wait-pattern" => c.wait_pattern = val()?,
            "--early-pattern" => c.early_pattern = val()?,
            "--audio" => c.audio = val()?,
            "--cpu-count" => c.cpu_count = val()?.parse().map_err(|_| "bad --cpu-count")?,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if c.send_lines.is_empty() {
        c.send_lines.push("ret".into());
    }
    Ok(c)
}

fn run() -> Result<bool, String> {
    let cfg = parse_cfg()?;
    let root = repo_root()?;
    let serial_log = root.join("serial.log");
    let qemu_log = root.join("qemu-debug.log");
    let qemu_stdout = root.join("qemu-stdout.log");
    let qemu_stderr = root.join("qemu-stderr.log");
    let esp_dir = root.join("esp");

    stop_old_qemu();
    for f in [&serial_log, &qemu_log, &qemu_stdout, &qemu_stderr] {
        let _ = fs::remove_file(f);
    }

    if !cfg.no_build {
        println!("[1/4] Building ESP");
        println!("  Building bootloader...");
        cargo_build(&["build", "-p", "kazuos-bootloader", "--target", "x86_64-unknown-uefi", "--release"])?;
        println!("  Building kernel...");
        cargo_build(&[
            "build", "-p", "kazuos-kernel",
            "--target", "crates/kernel/x86_64-kazuos.json",
            "-Zbuild-std=core,alloc", "-Zbuild-std-features=compiler-builtins-mem", "-Zjson-target-spec",
            "--release",
        ])?;
        println!("  Setting up ESP...");
        build_esp(&root, &esp_dir)?;
    } else if !esp_dir.exists() {
        return Err("ESP directory not found: esp".into());
    }

    let ovmf = find_ovmf()?;
    let qemu = find_qemu()?;
    let ovmf_dir = ovmf.parent().ok_or("bad OVMF path")?;
    let ovmf_vars = find_first(&[
        ovmf_dir.join("edk2-i386-vars.fd"),
        ovmf_dir.join("edk2-x86_64-vars.fd"),
        ovmf_dir.join("OVMF_VARS.fd"),
    ]);
    let temp_vars = root.join("ovmf_vars_pipeline.tmp.fd");
    if let Some(v) = &ovmf_vars {
        copy(v, &temp_vars)?;
    }

    let mut args: Vec<String> = vec![
        "-machine".into(), "q35,pcspk-audiodev=snd0,i8042=on".into(),
        "-smp".into(), cfg.cpu_count.to_string(),
        "-drive".into(), format!("if=pflash,format=raw,readonly=on,file={}", ovmf.display()),
    ];
    if ovmf_vars.is_some() {
        args.push("-drive".into());
        args.push(format!("if=pflash,format=raw,file={}", temp_vars.display()));
    }
    args.extend([
        "-drive".into(), format!("format=raw,file=fat:rw:{}", esp_dir.display()),
        "-boot".into(), "order=a,menu=on".into(),
        "-m".into(), "4G".into(),
        "-netdev".into(), "user,id=net0".into(),
        "-device".into(), "e1000,netdev=net0".into(),
        "-display".into(), "none".into(),
        "-vnc".into(), "127.0.0.1:1".into(),
        "-serial".into(), format!("file:{}", serial_log.display()),
        "-monitor".into(), format!("tcp:127.0.0.1:{MONITOR_PORT},server,nowait"),
        "-audiodev".into(), "none,id=snd0".into(),
    ]);
    match cfg.audio.as_str() {
        "ac97" => args.extend(["-device".into(), "ac97,audiodev=snd0".into()]),
        "hda" => args.extend(["-device".into(), "intel-hda".into(), "-device".into(), "hda-duplex,audiodev=snd0".into()]),
        "none" => {}
        other => return Err(format!("bad --audio: {other}")),
    }
    args.extend([
        "-no-reboot".into(),
        "-d".into(), "int,guest_errors".into(),
        "-D".into(), qemu_log.display().to_string(),
    ]);

    println!("[2/4] Starting QEMU");
    let mut child = Command::new(&qemu)
        .args(&args)
        .stdout(Stdio::from(fs::File::create(&qemu_stdout).map_err(|e| e.to_string())?))
        .stderr(Stdio::from(fs::File::create(&qemu_stderr).map_err(|e| e.to_string())?))
        .spawn()
        .map_err(|e| format!("spawn QEMU: {e}"))?;

    let result = drive(&cfg, &serial_log);

    if !cfg.keep_alive {
        let _ = monitor_send(&["quit".into()]);
        sleep(Duration::from_millis(800));
        let _ = child.kill();
    }
    let _ = child.wait();

    println!("[4/4] Results");
    println!("=== serial.log tail ===");
    print_tail(&serial_log, 60);
    println!("=== qemu-debug.log faults ===");
    print_faults(&qemu_log);

    let ok = match (&cfg.expect, result) {
        (Some(pat), _) => {
            if log_contains(&serial_log, pat) {
                println!("[PASS] Expected pattern found: '{pat}'");
                true
            } else {
                println!("[FAIL] Expected pattern not found: '{pat}'");
                false
            }
        }
        (None, r) => r,
    };
    Ok(ok)
}

/// Boot interaction: wait for patterns, pick the menu entry, send input lines.
fn drive(cfg: &Cfg, serial_log: &Path) -> bool {
    println!("[3/4] Waiting for monitor");
    if !wait_port(MONITOR_PORT, 20) {
        eprintln!("QEMU monitor did not open");
        return false;
    }
    println!("[4/4] Waiting for bootloader (pattern: '{}')", cfg.early_pattern);
    if !wait_pattern(serial_log, &cfg.early_pattern, cfg.boot_timeout) {
        println!("Warning: early pattern not found, trying anyway");
    }
    sleep(Duration::from_secs(1));
    if cfg.verbose {
        let _ = monitor_send(&["sendkey down".into(), "sendkey ret".into()]);
    } else {
        let _ = monitor_send(&["sendkey ret".into()]);
    }
    sleep(Duration::from_millis(500));

    println!("[4/4] Waiting for shell prompt (pattern: '{}')", cfg.wait_pattern);
    if !wait_pattern(serial_log, &cfg.wait_pattern, cfg.boot_timeout) {
        println!("Warning: wait pattern not found, sending commands anyway");
    }
    sleep(Duration::from_secs(5));

    for line in &cfg.send_lines {
        let line = line.trim();
        if line == "ret" {
            let _ = monitor_send(&["sendkey 0x1c".into()]);
            sleep(Duration::from_millis(500));
        } else if line == "^C" || line == "ctrl-c" {
            let _ = monitor_send(&["sendkey ctrl-c".into()]);
            sleep(Duration::from_millis(500));
        } else {
            let mut s = line.to_string();
            s.push('\n');
            let _ = monitor_send(&text_to_sendkeys(&s));
            sleep(Duration::from_millis(500));
        }
    }
    sleep(Duration::from_secs(cfg.after_wait));
    true
}

/// Translate a string into QEMU `sendkey` monitor commands (one per char).
fn text_to_sendkeys(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for ch in text.chars() {
        let key = match ch {
            '\n' => "0x1c".to_string(),
            ' ' => "spc".to_string(),
            '/' => "slash".to_string(),
            '.' => "dot".to_string(),
            '-' => "minus".to_string(),
            '_' => "shift-minus".to_string(),
            '&' => "shift-7".to_string(),
            c => c.to_string(),
        };
        out.push(format!("sendkey {key}"));
    }
    out
}

fn monitor_send(lines: &[String]) -> Result<(), String> {
    let mut stream = connect_monitor(10)?;
    sleep(Duration::from_millis(200));
    // Drain any banner.
    stream.set_read_timeout(Some(Duration::from_millis(150))).ok();
    let mut scratch = [0u8; 1024];
    let _ = stream.read(&mut scratch);
    for line in lines {
        stream
            .write_all(format!("{line}\n").as_bytes())
            .map_err(|e| format!("monitor write: {e}"))?;
        stream.flush().ok();
        sleep(Duration::from_millis(500));
    }
    Ok(())
}

fn connect_monitor(timeout_secs: u64) -> Result<TcpStream, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match TcpStream::connect(("127.0.0.1", MONITOR_PORT)) {
            Ok(s) => return Ok(s),
            Err(_) if Instant::now() < deadline => sleep(Duration::from_millis(250)),
            Err(e) => return Err(format!("monitor connect failed: {e}")),
        }
    }
}

fn wait_port(port: u16, timeout_secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        sleep(Duration::from_millis(250));
    }
    false
}

fn wait_pattern(file: &Path, pattern: &str, timeout_secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        if log_contains(file, pattern) {
            return true;
        }
        sleep(Duration::from_millis(300));
    }
    false
}

fn log_contains(file: &Path, pattern: &str) -> bool {
    fs::read_to_string(file)
        .map(|s| s.contains(pattern))
        .unwrap_or(false)
}

fn print_tail(file: &Path, lines: usize) {
    match fs::read_to_string(file) {
        Ok(s) => {
            let all: Vec<&str> = s.lines().collect();
            let start = all.len().saturating_sub(lines);
            for l in &all[start..] {
                println!("{l}");
            }
        }
        Err(_) => println!("(missing)"),
    }
}

fn print_faults(file: &Path) {
    let Ok(s) = fs::read_to_string(file) else {
        println!("(missing)");
        return;
    };
    let markers = ["check_exception", "Triple", "v=0d", "v=0e", "v=06", "v=08"];
    let hits: Vec<&str> = s.lines().filter(|l| markers.iter().any(|m| l.contains(m))).collect();
    for l in hits.iter().rev().take(40).rev() {
        println!("{l}");
    }
}

fn stop_old_qemu() {
    let _ = Command::new("taskkill")
        .args(["/IM", "qemu-system-x86_64.exe", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    sleep(Duration::from_millis(300));
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
    find_first(&[
        PathBuf::from(r"C:\Program Files\qemu\qemu-system-x86_64.exe"),
        PathBuf::from(r"C:\msys64\mingw64\bin\qemu-system-x86_64.exe"),
    ])
    .ok_or_else(|| "qemu-system-x86_64.exe not found (set QEMU_PATH)".into())
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

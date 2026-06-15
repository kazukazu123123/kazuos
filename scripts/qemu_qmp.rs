---
[package]
name = "qemu_qmp"
edition = "2024"
[dependencies]
---
//! Headless QEMU controller using QMP (QEMU Machine Protocol).
//!
//! Starts QEMU headlessly (from the KazuOS ISO) with QMP + VNC, and can send
//! keys, type text, take screenshots, and read serial output.
//!
//! Run from the repository root:
//!   cargo +nightly -Zscript scripts/qemu_qmp.rs -- <action> [flags]
//!
//! Actions: start | stop | key | text | screenshot | serial | run
//!
//! Examples:
//!   ... qemu_qmp.rs -- start --keep-alive
//!   ... qemu_qmp.rs -- text --text "help"
//!   ... qemu_qmp.rs -- key  --key ret
//!   ... qemu_qmp.rs -- screenshot --out screen.png
//!   ... qemu_qmp.rs -- run  --text "ps" --out screen.png
//!   ... qemu_qmp.rs -- stop
//!
//! Flags: --iso PATH (default kazuos.iso) --key NAME --text STR --out PATH
//!        --qmp-port N (4444) --vnc-display N (2) --boot-wait N (30)
//!        --after-wait N (2) --no-build --keep-alive --no-wait

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

struct Cfg {
    action: String,
    iso: String,
    key: String,
    text: String,
    out: String,
    qmp_port: u16,
    vnc_display: u16,
    boot_wait: u64,
    after_wait: u64,
    no_build: bool,
    keep_alive: bool,
    no_wait: bool,
    root: PathBuf,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("qemu_qmp: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let root = repo_root()?;
    let mut cfg = Cfg {
        action: "run".into(),
        iso: "kazuos.iso".into(),
        key: "ret".into(),
        text: String::new(),
        out: std::env::temp_dir().join("kazuos_screen.ppm").to_string_lossy().into_owned(),
        qmp_port: 4444,
        vnc_display: 2,
        boot_wait: 30,
        after_wait: 2,
        no_build: false,
        keep_alive: false,
        no_wait: false,
        root: root.clone(),
    };

    let mut it = std::env::args().skip(1).peekable();
    // First non-flag arg is the action.
    if let Some(first) = it.peek() {
        if !first.starts_with("--") {
            cfg.action = it.next().unwrap();
        }
    }
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{a} requires a value"));
        match a.as_str() {
            "--action" => cfg.action = val()?,
            "--iso" => cfg.iso = val()?,
            "--key" => cfg.key = val()?,
            "--text" | "--send-text" => cfg.text = val()?,
            "--out" => cfg.out = val()?,
            "--qmp-port" => cfg.qmp_port = val()?.parse().map_err(|_| "bad --qmp-port")?,
            "--vnc-display" => cfg.vnc_display = val()?.parse().map_err(|_| "bad --vnc-display")?,
            "--boot-wait" => cfg.boot_wait = val()?.parse().map_err(|_| "bad --boot-wait")?,
            "--after-wait" => cfg.after_wait = val()?.parse().map_err(|_| "bad --after-wait")?,
            "--no-build" => cfg.no_build = true,
            "--keep-alive" => cfg.keep_alive = true,
            "--no-wait" => cfg.no_wait = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    match cfg.action.as_str() {
        "start" => action_start(&cfg),
        "stop" => { stop_qemu(&cfg); println!("QEMU stopped."); Ok(()) }
        "key" => { qmp_send_key(&cfg, &cfg.key)?; println!("Sent key: {}", cfg.key); Ok(()) }
        "text" => { qmp_send_text(&cfg, &cfg.text)?; qmp_send_key(&cfg, "ret")?; println!("Sent text: {}", cfg.text); Ok(()) }
        "screenshot" => { let s = screenshot(&cfg, &cfg.out)?; println!("Screenshot saved: {s}"); Ok(()) }
        "serial" => { print!("{}", fs::read_to_string(cfg.root.join("serial.log")).unwrap_or_else(|_| "(no serial.log)".into())); Ok(()) }
        "run" => action_run(&cfg),
        other => Err(format!("Unknown action: {other}. Use: start|stop|key|text|screenshot|serial|run")),
    }
}

fn action_start(cfg: &Cfg) -> Result<(), String> {
    stop_qemu(cfg);
    let _ = fs::remove_file(cfg.root.join("serial.log"));
    if !cfg.no_build {
        println!("Building ISO...");
        make_iso(cfg)?;
    }
    start_qemu(cfg)?;
    if !wait_port(cfg.qmp_port, 20) {
        return Err("QMP did not open".into());
    }
    println!("QMP ready.");
    if !wait_for_serial(cfg, "KazuOS Bootloader", 20) {
        eprintln!("Warning: bootloader prompt not seen");
    }
    sleep(Duration::from_millis(500));
    qmp_send_key(cfg, "ret")?;
    if !cfg.no_wait {
        println!("Waiting for kernel start...");
        if !wait_for_serial(cfg, "KazuOS kernel started", cfg.boot_wait) {
            eprintln!("Warning: kernel start not seen in {}s", cfg.boot_wait);
        }
        sleep(Duration::from_secs(2));
    }
    if !cfg.keep_alive {
        println!("QEMU started. Use the 'stop' action to shut it down.");
        println!("(PID saved to .qemu_qmp.pid)");
    }
    Ok(())
}

fn action_run(cfg: &Cfg) -> Result<(), String> {
    stop_qemu(cfg);
    let _ = fs::remove_file(cfg.root.join("serial.log"));
    if !cfg.no_build {
        println!("Building ISO...");
        make_iso(cfg)?;
    }
    start_qemu(cfg)?;
    if !wait_port(cfg.qmp_port, 20) {
        return Err("QMP did not open".into());
    }
    if !wait_for_serial(cfg, "KazuOS Bootloader", 20) {
        eprintln!("Warning: bootloader prompt not seen");
    }
    sleep(Duration::from_millis(500));
    qmp_send_key(cfg, "ret")?;
    println!("Waiting for kernel start...");
    if !wait_for_serial(cfg, "KazuOS kernel started", cfg.boot_wait) {
        eprintln!("Warning: kernel start not seen");
    }
    sleep(Duration::from_secs(2));
    if !cfg.text.is_empty() {
        println!("Sending: {}", cfg.text);
        qmp_send_text(cfg, &cfg.text)?;
        qmp_send_key(cfg, "ret")?;
        sleep(Duration::from_secs(cfg.after_wait));
    }
    let saved = screenshot(cfg, &cfg.out)?;
    println!("Screenshot: {saved}");
    println!("=== Serial output ===");
    print!("{}", fs::read_to_string(cfg.root.join("serial.log")).unwrap_or_else(|_| "(none)".into()));
    if !cfg.keep_alive {
        stop_qemu(cfg);
    }
    Ok(())
}

// ---- QMP protocol ----

fn qmp_exec(cfg: &Cfg, json: &str) -> Result<(), String> {
    let stream = TcpStream::connect(("127.0.0.1", cfg.qmp_port)).map_err(|e| format!("QMP connect: {e}"))?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?; // greeting
    writer.write_all(b"{\"execute\":\"qmp_capabilities\"}\n").map_err(|e| e.to_string())?;
    line.clear();
    reader.read_line(&mut line).map_err(|e| e.to_string())?; // cap response
    writer.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    sleep(Duration::from_millis(120));
    // Drain whatever came back (not parsed).
    reader.get_ref().set_read_timeout(Some(Duration::from_millis(150))).ok();
    let mut buf = [0u8; 2048];
    let _ = reader.get_mut().read(&mut buf);
    Ok(())
}

fn qmp_send_key(cfg: &Cfg, qcode: &str) -> Result<(), String> {
    let json = format!(
        "{{\"execute\":\"send-key\",\"arguments\":{{\"keys\":[{{\"type\":\"qcode\",\"data\":\"{qcode}\"}}]}}}}"
    );
    qmp_exec(cfg, &json)
}

fn qmp_send_text(cfg: &Cfg, text: &str) -> Result<(), String> {
    for ch in text.chars() {
        let qcode = match ch {
            ' ' => "spc".to_string(),
            '/' => "slash".to_string(),
            '.' => "dot".to_string(),
            '-' => "minus".to_string(),
            '_' => "shift-minus".to_string(),
            '=' => "equal".to_string(),
            '+' => "shift-equal".to_string(),
            c => c.to_string(),
        };
        qmp_send_key(cfg, &qcode)?;
        sleep(Duration::from_millis(30));
    }
    Ok(())
}

fn screenshot(cfg: &Cfg, path: &str) -> Result<String, String> {
    let ppm = if let Some(stripped) = path.strip_suffix(".png") {
        format!("{stripped}.ppm")
    } else {
        path.to_string()
    };
    let qemu_path = ppm.replace('\\', "/");
    let json = format!("{{\"execute\":\"screendump\",\"arguments\":{{\"filename\":\"{qemu_path}\"}}}}");
    qmp_exec(cfg, &json)?;
    sleep(Duration::from_millis(300));
    if ppm != path && Path::new(&ppm).exists() {
        if which("magick").is_some() {
            let _ = Command::new("magick").arg(&ppm).arg(path).status();
            let _ = fs::remove_file(&ppm);
        } else {
            let _ = fs::copy(&ppm, path);
        }
    }
    Ok(ppm)
}

// ---- QEMU lifecycle ----

fn start_qemu(cfg: &Cfg) -> Result<(), String> {
    let ovmf = find_ovmf()?;
    let qemu = find_qemu()?;
    let ovmf_dir = ovmf.parent().ok_or("bad OVMF path")?;
    let ovmf_vars = find_first(&[
        ovmf_dir.join("edk2-i386-vars.fd"),
        ovmf_dir.join("edk2-x86_64-vars.fd"),
        ovmf_dir.join("OVMF_VARS.fd"),
    ]);
    let temp_vars = cfg.root.join("ovmf_vars_qmp.tmp.fd");
    if let Some(v) = &ovmf_vars {
        fs::copy(v, &temp_vars).map_err(|e| e.to_string())?;
    }
    let iso_path = if Path::new(&cfg.iso).is_absolute() {
        PathBuf::from(&cfg.iso)
    } else {
        cfg.root.join(&cfg.iso)
    };

    let mut args: Vec<String> = vec![
        "-machine".into(), "q35".into(),
        "-drive".into(), format!("if=pflash,format=raw,readonly=on,file={}", ovmf.display()),
    ];
    if ovmf_vars.is_some() {
        args.push("-drive".into());
        args.push(format!("if=pflash,format=raw,file={}", temp_vars.display()));
    }
    args.extend([
        "-cdrom".into(), iso_path.display().to_string(),
        "-boot".into(), "order=d,menu=on".into(),
        "-m".into(), "4G".into(),
        "-net".into(), "none".into(),
        "-no-reboot".into(),
        "-d".into(), "int,guest_errors".into(),
        "-D".into(), cfg.root.join("qemu-debug.log").display().to_string(),
        "-display".into(), "none".into(),
        "-vnc".into(), format!("127.0.0.1:{}", cfg.vnc_display),
        "-serial".into(), format!("file:{}", cfg.root.join("serial.log").display()),
        "-qmp".into(), format!("tcp:127.0.0.1:{},server,nowait", cfg.qmp_port),
    ]);

    println!("Starting QEMU headlessly...");
    let child = Command::new(&qemu)
        .args(&args)
        .current_dir(&cfg.root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn QEMU: {e}"))?;
    fs::write(cfg.root.join(".qemu_qmp.pid"), child.id().to_string()).map_err(|e| e.to_string())?;
    println!("QEMU PID: {}  QMP port: {}  VNC: 127.0.0.1:{}", child.id(), cfg.qmp_port, 5900 + cfg.vnc_display);
    Ok(())
}

fn stop_qemu(cfg: &Cfg) {
    let pid_file = cfg.root.join(".qemu_qmp.pid");
    if let Ok(pid) = fs::read_to_string(&pid_file) {
        let pid = pid.trim();
        if !pid.is_empty() {
            let _ = Command::new("taskkill").args(["/PID", pid, "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        let _ = fs::remove_file(&pid_file);
    }
    let _ = Command::new("taskkill").args(["/IM", "qemu-system-x86_64.exe", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    sleep(Duration::from_millis(400));
}

fn make_iso(cfg: &Cfg) -> Result<(), String> {
    let status = Command::new("cargo")
        .args(["+nightly", "-Zscript", "scripts/make_iso.rs", "--output"])
        .arg(&cfg.iso)
        .status()
        .map_err(|e| format!("run make_iso.rs: {e}"))?;
    if !status.success() {
        return Err("make_iso.rs failed".into());
    }
    Ok(())
}

fn wait_for_serial(cfg: &Cfg, pattern: &str, secs: u64) -> bool {
    let f = cfg.root.join("serial.log");
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if fs::read_to_string(&f).map(|s| s.contains(pattern)).unwrap_or(false) {
            return true;
        }
        sleep(Duration::from_millis(300));
    }
    false
}

fn wait_port(port: u16, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        sleep(Duration::from_millis(300));
    }
    false
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

fn find_ovmf() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("OVMF_PATH") {
        let p = PathBuf::from(p);
        if p.exists() { return Ok(p); }
    }
    find_first(&[
        PathBuf::from(r"C:\Program Files\qemu\share\edk2-x86_64-code.fd"),
        PathBuf::from(r"C:\Program Files\qemu\share\qemu\edk2-x86_64-code.fd"),
        PathBuf::from(r"C:\Program Files\qemu\share\ovmf-x64\OVMF_CODE.fd"),
    ]).ok_or_else(|| "OVMF not found (set OVMF_PATH)".into())
}

fn find_qemu() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("QEMU_PATH") {
        let p = PathBuf::from(p);
        if p.exists() { return Ok(p); }
    }
    find_first(&[
        PathBuf::from(r"C:\Program Files\qemu\qemu-system-x86_64.exe"),
        PathBuf::from(r"C:\msys64\mingw64\bin\qemu-system-x86_64.exe"),
    ]).ok_or_else(|| "qemu-system-x86_64.exe not found (set QEMU_PATH)".into())
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

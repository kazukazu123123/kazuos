---
[package]
name = "auto_test_pipeline"
edition = "2024"
[dependencies]
---
//! Automated boot/test pipeline: build KazuOS, boot it in QEMU, drive the shell
//! over the QEMU monitor, and check the serial log. Headless.
//!
//! Two ways to describe what to do once the shell is up:
//!
//! 1. Ordered inline step flags (repeatable, order preserved):
//!      --send LINE     type LINE then press Enter
//!      --key  NAME     press one key (ret, ctrl-c, esc, 0x1c, ...)
//!      --wait SECS     sleep (accepts fractions, e.g. 0.3)
//!      --expect PAT    block until PAT appears in serial.log (fails on timeout)
//!    Wrap the whole inline sequence in a loop with `--repeat N`.
//!
//! 2. A scenario file (`--scenario FILE`) with a tiny line DSL:
//!      # comment
//!      send hdatest
//!      wait 0.3
//!      key ctrl-c
//!      repeat 30 {
//!        send hdatest
//!        wait 0.2
//!        key ctrl-c
//!      }
//!      expect "shell"
//!
//! Throughout the whole run a background watcher tails serial.log and aborts
//! (FAIL) the moment a `--fail-on` pattern appears, or — if `--idle-timeout` is
//! set — if the log stops growing for that many seconds (freeze/hang detection).
//! Sensible kernel-panic patterns are watched by default.
//!
//! Run from the repository root, e.g. drive the HDA "連打" repro:
//!   cargo +nightly -Zscript scripts/auto_test_pipeline.rs -- \
//!       --audio hda --cpu-count 4 --idle-timeout 10 \
//!       --repeat 40 --send hdatest --wait 0.25 --key ctrl-c --wait 0.15
//!
//! Flags (all optional):
//!   --boot-timeout N     seconds to wait for boot patterns (default 30)
//!   --after-wait N       seconds to keep running after the scenario (default 6)
//!   --no-build           reuse the existing esp/ instead of rebuilding
//!   --keep-alive         do not quit QEMU at the end
//!   --verbose            pick the "Verbose" boot menu entry
//!   --heartbeat          pick the "Verbose + Heartbeat" boot menu entry (enables
//!                        the periodic HEARTBEAT liveness line; implies verbose)
//!   --scenario FILE      run steps from a scenario file (see DSL above)
//!   --send LINE          inline step: type LINE + Enter (repeatable, ordered)
//!   --key NAME           inline step: press one key (repeatable, ordered)
//!   --wait SECS          inline step: sleep (repeatable, ordered)
//!   --expect PAT         inline step: wait for PAT in serial.log
//!   --repeat N           repeat the inline step sequence N times
//!   --send-delay MS      per-key delay when typing/pressing (default 120)
//!   --expect-timeout N   seconds an `expect` step waits before failing (default 15)
//!   --fail-on PAT        abort (FAIL) if PAT appears in serial.log (repeatable)
//!   --no-default-fail-on do not watch the built-in panic patterns
//!   --idle-timeout N     freeze window in seconds (0=off, default 0). Without
//!                        --liveness-pattern: FAIL if serial.log stops growing
//!                        for N s. With it: FAIL if no liveness line for N s.
//!   --liveness-pattern P treat freeze as "P stopped appearing" rather than raw
//!                        serial silence. Use with the kernel's HEARTBEAT line
//!                        (enabled by --heartbeat) so idle gaps don't false-positive:
//!                        --heartbeat --liveness-pattern HEARTBEAT --idle-timeout 8
//!   --wait-pattern S     serial substring marking shell readiness
//!                        (default "KazuOS kernel started")
//!   --early-pattern S    serial substring marking the bootloader
//!                        (default "KazuOS Bootloader")
//!   --audio hda|none     audio device (default hda)
//!   --cpu-count N        number of CPUs (default 2)

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

const MONITOR_PORT: u16 = 55555;

const DEFAULT_FAIL_ON: &[&str] = &[
    "KERNEL PANIC",
    "PAGE FAULT",
    "DOUBLE FAULT",
    "Triple",
];

fn main() -> std::process::ExitCode {
    match run() {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(e) => {
            eprintln!("auto_test_pipeline: error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// One scenario action. Executed in order; `Repeat` runs its body `count` times.
#[derive(Clone)]
enum Step {
    Send(String),
    Key(String),
    Wait(f64),
    Expect(String),
    Repeat { count: u32, body: Vec<Step> },
}

struct Cfg {
    boot_timeout: u64,
    after_wait: u64,
    no_build: bool,
    keep_alive: bool,
    verbose: bool,
    heartbeat: bool,
    steps: Vec<Step>,
    repeat: u32,
    scenario: Option<PathBuf>,
    send_delay_ms: u64,
    expect_timeout: u64,
    fail_on: Vec<String>,
    idle_timeout: u64,
    liveness_pattern: Option<String>,
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
        heartbeat: false,
        steps: Vec::new(),
        repeat: 1,
        scenario: None,
        send_delay_ms: 120,
        expect_timeout: 15,
        fail_on: DEFAULT_FAIL_ON.iter().map(|s| s.to_string()).collect(),
        idle_timeout: 0,
        liveness_pattern: None,
        wait_pattern: "KazuOS kernel started".into(),
        early_pattern: "KazuOS Bootloader".into(),
        audio: "hda".into(),
        cpu_count: 2,
    };
    let mut default_fail_on = true;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{a} requires a value"));
        match a.as_str() {
            "--boot-timeout" => c.boot_timeout = val()?.parse().map_err(|_| "bad --boot-timeout")?,
            "--after-wait" => c.after_wait = val()?.parse().map_err(|_| "bad --after-wait")?,
            "--no-build" => c.no_build = true,
            "--keep-alive" => c.keep_alive = true,
            "--verbose" => c.verbose = true,
            "--heartbeat" => c.heartbeat = true,
            "--scenario" => c.scenario = Some(PathBuf::from(val()?)),
            "--send" => c.steps.push(Step::Send(val()?)),
            "--key" => c.steps.push(Step::Key(val()?)),
            "--wait" => c.steps.push(Step::Wait(val()?.parse().map_err(|_| "bad --wait")?)),
            "--expect" => c.steps.push(Step::Expect(val()?)),
            "--repeat" => c.repeat = val()?.parse().map_err(|_| "bad --repeat")?,
            "--send-delay" => c.send_delay_ms = val()?.parse().map_err(|_| "bad --send-delay")?,
            "--expect-timeout" => c.expect_timeout = val()?.parse().map_err(|_| "bad --expect-timeout")?,
            "--fail-on" => c.fail_on.push(val()?),
            "--no-default-fail-on" => default_fail_on = false,
            "--idle-timeout" => c.idle_timeout = val()?.parse().map_err(|_| "bad --idle-timeout")?,
            "--liveness-pattern" => c.liveness_pattern = Some(val()?),
            "--wait-pattern" => c.wait_pattern = val()?,
            "--early-pattern" => c.early_pattern = val()?,
            "--audio" => c.audio = val()?,
            "--cpu-count" => c.cpu_count = val()?.parse().map_err(|_| "bad --cpu-count")?,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if !default_fail_on {
        let extra = c.fail_on.split_off(DEFAULT_FAIL_ON.len());
        c.fail_on = extra;
    }
    // A scenario file overrides inline steps entirely.
    if let Some(path) = &c.scenario {
        let text = fs::read_to_string(path).map_err(|e| format!("read scenario {}: {e}", path.display()))?;
        c.steps = parse_scenario(&text)?;
        c.repeat = 1;
    } else if c.repeat != 1 {
        let body = std::mem::take(&mut c.steps);
        c.steps = vec![Step::Repeat { count: c.repeat, body }];
    }
    if c.steps.is_empty() {
        c.steps.push(Step::Key("ret".into()));
    }
    Ok(c)
}

/// Parse the scenario-file DSL into a step list. Supports nested `repeat N { ... }`.
fn parse_scenario(text: &str) -> Result<Vec<Step>, String> {
    let mut lines = text.lines().map(|l| l.to_string());
    let steps = parse_block(&mut lines, true)?;
    Ok(steps)
}

fn parse_block(lines: &mut dyn Iterator<Item = String>, top: bool) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "}" {
            if top {
                return Err("unexpected '}' at top level".into());
            }
            return Ok(steps);
        }
        let (kw, rest) = match line.split_once(char::is_whitespace) {
            Some((k, r)) => (k, r.trim()),
            None => (line, ""),
        };
        match kw {
            "send" => steps.push(Step::Send(rest.to_string())),
            "key" => steps.push(Step::Key(rest.to_string())),
            "wait" => steps.push(Step::Wait(rest.parse().map_err(|_| format!("bad wait: {rest}"))?)),
            "expect" => steps.push(Step::Expect(strip_quotes(rest))),
            "repeat" => {
                let body_open = rest.trim_end();
                let count_str = body_open.trim_end_matches('{').trim();
                let count: u32 = count_str.parse().map_err(|_| format!("bad repeat count: {count_str}"))?;
                if !body_open.ends_with('{') {
                    return Err("repeat must end with '{'".into());
                }
                let body = parse_block(lines, false)?;
                steps.push(Step::Repeat { count, body });
            }
            other => return Err(format!("unknown scenario keyword: {other}")),
        }
    }
    if !top {
        return Err("missing closing '}'".into());
    }
    Ok(steps)
}

fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && (s.starts_with('"') && s.ends_with('"')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Shared watcher state: set when the run must abort (panic pattern / freeze).
struct Watch {
    stop: AtomicBool,
    abort: Mutex<Option<String>>,
}

impl Watch {
    fn aborted(&self) -> Option<String> {
        self.abort.lock().unwrap().clone()
    }
    fn set_abort(&self, reason: String) {
        let mut g = self.abort.lock().unwrap();
        if g.is_none() {
            *g = Some(reason);
        }
    }
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
        "-net".into(), "none".into(),
        "-display".into(), "none".into(),
        "-vnc".into(), "127.0.0.1:1".into(),
        "-serial".into(), format!("file:{}", serial_log.display()),
        "-monitor".into(), format!("tcp:127.0.0.1:{MONITOR_PORT},server,nowait"),
        "-audiodev".into(), "none,id=snd0".into(),
    ]);
    match cfg.audio.as_str() {
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
        let _ = monitor_send(&["quit".into()], 200);
        sleep(Duration::from_millis(800));
        let _ = child.kill();
    }
    let _ = child.wait();

    println!("[4/4] Results");
    println!("=== serial.log tail ===");
    print_tail(&serial_log, 60);
    println!("=== qemu-debug.log faults ===");
    print_faults(&qemu_log);

    let ok = match result {
        DriveOutcome::Aborted(reason) => {
            println!("[FAIL] aborted: {reason}");
            false
        }
        DriveOutcome::Ok => {
            // Optional final gate: any `expect` step already validated inline,
            // so a clean run with no abort is a pass.
            println!("[PASS] scenario completed with no abort");
            true
        }
    };
    Ok(ok)
}

enum DriveOutcome {
    Ok,
    Aborted(String),
}

/// Boot interaction: wait for patterns, pick the menu entry, run the scenario
/// under a background panic/freeze watcher.
fn drive(cfg: &Cfg, serial_log: &Path) -> DriveOutcome {
    println!("[3/4] Waiting for monitor");
    if !wait_port(MONITOR_PORT, 20) {
        return DriveOutcome::Aborted("QEMU monitor did not open".into());
    }
    println!("[4/4] Waiting for bootloader (pattern: '{}')", cfg.early_pattern);
    if !wait_pattern(serial_log, &cfg.early_pattern, cfg.boot_timeout) {
        println!("Warning: early pattern not found, trying anyway");
    }
    sleep(Duration::from_secs(1));
    // Boot menu entries: 0=KazuOS, 1=Verbose, 2=Verbose + Heartbeat.
    if cfg.heartbeat {
        let _ = monitor_send(&["sendkey down".into(), "sendkey down".into(), "sendkey ret".into()], 200);
    } else if cfg.verbose {
        let _ = monitor_send(&["sendkey down".into(), "sendkey ret".into()], 200);
    } else {
        let _ = monitor_send(&["sendkey ret".into()], 200);
    }
    sleep(Duration::from_millis(500));

    println!("[4/4] Waiting for shell prompt (pattern: '{}')", cfg.wait_pattern);
    if !wait_pattern(serial_log, &cfg.wait_pattern, cfg.boot_timeout) {
        println!("Warning: wait pattern not found, sending commands anyway");
    }
    sleep(Duration::from_secs(5));

    // Start the background watcher only now that boot is done, so boot-time
    // quiet periods don't trip the freeze detector.
    let watch = Arc::new(Watch { stop: AtomicBool::new(false), abort: Mutex::new(None) });
    let watcher = spawn_watcher(
        watch.clone(),
        serial_log.to_path_buf(),
        cfg.fail_on.clone(),
        cfg.idle_timeout,
        cfg.liveness_pattern.clone(),
    );

    let outcome = match exec_steps(&cfg.steps, cfg, serial_log, &watch) {
        Ok(()) => {
            sleep_checked(Duration::from_secs(cfg.after_wait), &watch);
            match watch.aborted() {
                Some(r) => DriveOutcome::Aborted(r),
                None => DriveOutcome::Ok,
            }
        }
        Err(reason) => DriveOutcome::Aborted(reason),
    };

    watch.stop.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    outcome
}

/// Background tail of serial.log: trips abort on a fail pattern or on freeze.
///
/// Freeze detection has two modes. With a `liveness_pattern`, a freeze means
/// "that liveness line (e.g. the kernel HEARTBEAT) appeared at least once and
/// then stopped for `idle_timeout` seconds" — so legitimate workload-quiet
/// periods don't false-positive; only a real total hang stops the beat. Without
/// it, freeze falls back to "serial.log stopped growing for `idle_timeout`s".
fn spawn_watcher(
    watch: Arc<Watch>,
    serial_log: PathBuf,
    fail_on: Vec<String>,
    idle_timeout: u64,
    liveness_pattern: Option<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut last_size: u64 = fs::metadata(&serial_log).map(|m| m.len()).unwrap_or(0);
        let mut last_change = Instant::now();
        let mut last_beats: usize = 0;
        let mut last_beat_time = Instant::now();
        let mut seen_beat = false;
        let mut reported_fail = false;
        while !watch.stop.load(Ordering::SeqCst) {
            if let Ok(s) = fs::read_to_string(&serial_log) {
                if !reported_fail {
                    if let Some(pat) = fail_on.iter().find(|p| s.contains(p.as_str())) {
                        watch.set_abort(format!("fail-on pattern seen: '{pat}'"));
                        reported_fail = true;
                    }
                }
                let size = s.len() as u64;
                if size != last_size {
                    last_size = size;
                    last_change = Instant::now();
                }
                if let Some(pat) = &liveness_pattern {
                    let beats = s.matches(pat.as_str()).count();
                    if beats > last_beats {
                        last_beats = beats;
                        last_beat_time = Instant::now();
                        seen_beat = true;
                    }
                }
            }
            if idle_timeout > 0 {
                match &liveness_pattern {
                    Some(pat) => {
                        if seen_beat && last_beat_time.elapsed() >= Duration::from_secs(idle_timeout) {
                            watch.set_abort(format!(
                                "freeze: liveness '{pat}' stopped for >= {idle_timeout}s (total hang)"
                            ));
                            break;
                        }
                    }
                    None => {
                        if last_size > 0 && last_change.elapsed() >= Duration::from_secs(idle_timeout) {
                            watch.set_abort(format!(
                                "freeze: serial.log idle for >= {idle_timeout}s (last size {last_size} bytes)"
                            ));
                            break;
                        }
                    }
                }
            }
            sleep(Duration::from_millis(300));
        }
    })
}

/// Execute steps in order; returns Err(reason) if the watcher aborts mid-run.
fn exec_steps(steps: &[Step], cfg: &Cfg, serial_log: &Path, watch: &Watch) -> Result<(), String> {
    for step in steps {
        if let Some(r) = watch.aborted() {
            return Err(r);
        }
        match step {
            Step::Send(line) => {
                let mut s = line.clone();
                s.push('\n');
                let _ = monitor_send(&text_to_sendkeys(&s), cfg.send_delay_ms);
            }
            Step::Key(name) => {
                let _ = monitor_send(&[key_to_sendkey(name)], cfg.send_delay_ms);
            }
            Step::Wait(secs) => {
                sleep_checked(Duration::from_secs_f64(*secs), watch);
            }
            Step::Expect(pat) => {
                if !wait_pattern_watched(serial_log, pat, cfg.expect_timeout, watch) {
                    if let Some(r) = watch.aborted() {
                        return Err(r);
                    }
                    return Err(format!("expect '{pat}' not found within {}s", cfg.expect_timeout));
                }
            }
            Step::Repeat { count, body } => {
                for i in 0..*count {
                    if let Some(r) = watch.aborted() {
                        return Err(format!("{r} (during repeat iter {}/{count})", i + 1));
                    }
                    exec_steps(body, cfg, serial_log, watch)?;
                }
            }
        }
    }
    Ok(())
}

/// Map a friendly key name to a QEMU `sendkey` monitor command.
fn key_to_sendkey(name: &str) -> String {
    let key = match name.trim() {
        "ret" | "enter" => "0x1c".to_string(),
        "^C" | "ctrl-c" => "ctrl-c".to_string(),
        other => other.to_string(),
    };
    format!("sendkey {key}")
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
            '|' => "shift-backslash".to_string(),
            c => c.to_string(),
        };
        out.push(format!("sendkey {key}"));
    }
    out
}

fn monitor_send(lines: &[String], per_line_ms: u64) -> Result<(), String> {
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
        sleep(Duration::from_millis(per_line_ms));
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

/// Like `wait_pattern` but bails early if the watcher trips an abort.
fn wait_pattern_watched(file: &Path, pattern: &str, timeout_secs: u64, watch: &Watch) -> bool {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        if watch.aborted().is_some() {
            return false;
        }
        if log_contains(file, pattern) {
            return true;
        }
        sleep(Duration::from_millis(300));
    }
    false
}

/// Sleep in small slices so an abort is noticed promptly.
fn sleep_checked(dur: Duration, watch: &Watch) {
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        if watch.aborted().is_some() {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        sleep(remaining.min(Duration::from_millis(100)));
    }
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

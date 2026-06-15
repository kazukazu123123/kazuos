param(
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path

function Find-FirstExisting($paths) {
    foreach ($p in $paths) {
        if ($p -and (Test-Path $p)) { return $p }
    }
    return $null
}

function Show-Menu([string]$Title, [string[]]$Options) {
    Write-Host ""
    Write-Host "  $Title"
    for ($i = 0; $i -lt $Options.Length; $i++) {
        Write-Host "    [$($i + 1)] $($Options[$i])"
    }
    do {
        $input = Read-Host "  Select"
        $n = 0
        $ok = [int]::TryParse($input, [ref]$n) -and $n -ge 1 -and $n -le $Options.Length
    } while (-not $ok)
    return $n - 1
}

# --- Build ---
$doBuild = $true
if ($NoBuild) {
    $doBuild = $false
} else {
    $buildChoice = Show-Menu "Build" @(
        "Build",
        "Skip build (use existing binaries)"
    )
    $doBuild = ($buildChoice -eq 0)
}

# --- Debug options ---
$debugChoice = Show-Menu "Debug options" @(
    "None          -- normal boot",
    "no-reboot     -- halt on triple fault instead of rebooting",
    "no-reboot + no-shutdown -- keep QEMU paused after fault/exception",
    "Full          -- no-reboot + no-shutdown + exception log (-d int,guest_errors)"
)

# --- Audio device ---
$audioChoice = Show-Menu "Audio device" @(
    "AC97        -- legacy AC97 audio",
    "Intel HDA   -- Intel High Definition Audio (for hda driver)",
    "None        -- no audio device"
)

Write-Host ""

# ===== Find OVMF =====
$OvmfPath = if ($env:OVMF_PATH) { $env:OVMF_PATH } else {
    Find-FirstExisting @(
        "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
        "C:\Program Files\qemu\share\qemu\edk2-x86_64-code.fd",
        "C:\Program Files\qemu\share\ovmf-x64\OVMF_CODE.fd",
        "C:\ProgramData\chocolatey\lib\qemu\tools\qemu\share\edk2-x86_64-code.fd",
        "C:\msys64\usr\share\qemu\edk2-x86_64-code.fd"
    )
}
if (-not $OvmfPath) { throw "OVMF firmware not found. Set OVMF_PATH." }

$OvmfDir = Split-Path -Parent $OvmfPath
$OvmfVars = Find-FirstExisting @(
    (Join-Path $OvmfDir "edk2-i386-vars.fd"),
    (Join-Path $OvmfDir "edk2-x86_64-vars.fd"),
    (Join-Path $OvmfDir "OVMF_VARS.fd")
)

# ===== Find QEMU =====
$QemuPath = if ($env:QEMU_PATH) { $env:QEMU_PATH } else {
    Find-FirstExisting @(
        "C:\Program Files\qemu\qemu-system-x86_64.exe",
        "C:\ProgramData\chocolatey\bin\qemu-system-x86_64.exe",
        "C:\msys64\mingw64\bin\qemu-system-x86_64.exe",
        "C:\msys64\usr\bin\qemu-system-x86_64.exe"
    )
}
if (-not $QemuPath) { throw "qemu-system-x86_64.exe not found. Set QEMU_PATH." }

# ===== Build =====
if ($doBuild) {
    Write-Host "Building bootloader..."
    cargo build -p kazuos-bootloader --target x86_64-unknown-uefi --release
    if ($LASTEXITCODE -ne 0) { throw "Bootloader build failed." }

    Write-Host "Building kernel..."
    cargo build -p kazuos-kernel --target crates/kernel/x86_64-kazuos.json '-Zbuild-std=core,alloc' '-Zbuild-std-features=compiler-builtins-mem' -Zjson-target-spec --release
    if ($LASTEXITCODE -ne 0) { throw "Kernel build failed." }

    Write-Host "Preparing ESP (Limine chain-load)..."
    & (Join-Path $Root "setup_limine_esp.ps1") `
        -EspDir        (Join-Path $Root "esp") `
        -BootloaderEfi (Join-Path $Root "target\x86_64-unknown-uefi\release\kazuos-bootloader.efi") `
        -KernelElf     (Join-Path $Root "target\x86_64-kazuos\release\kazuos-kernel") `
        -InitrdKfs     (Join-Path $Root "target\initrd.kfs") `
        -FontTtf       (Join-Path $Root "font.ttf")
} elseif (!(Test-Path (Join-Path $Root "esp"))) {
    throw "ESP directory not found: esp\`nBuild first or use an existing ESP."
}

# ===== Copy VARS =====
$TempVars = Join-Path $Root "ovmf_vars.tmp.fd"
if ($OvmfVars) { Copy-Item $OvmfVars $TempVars -Force }

# ===== Build QEMU args =====
$qemuArgs = @(
    "-machine", "q35,pcspk-audiodev=snd0",
    "-drive",   "if=pflash,format=raw,readonly=on,file=$OvmfPath"
)
if ($OvmfVars) {
    $qemuArgs += @("-drive", "if=pflash,format=raw,file=$TempVars")
}

$qemuArgs += @(
    "-drive", "format=raw,file=fat:rw:esp",
    "-boot",  "order=a,menu=on"
)

$qemuArgs += @(
    "-m",       "1G",
    "-net",     "none",
    "-device",  "VGA",
    "-audiodev","dsound,id=snd0",
    "-serial",  "stdio"
)

$qemuArgs += @(
    "-smp",     "4"
)

switch ($audioChoice) {
    0 { $qemuArgs += @("-device", "AC97,audiodev=snd0") }
    1 { $qemuArgs += @("-device", "intel-hda", "-device", "hda-duplex,audiodev=snd0") }
    2 { }
}

if ($debugChoice -ge 1) { $qemuArgs += "-no-reboot" }
if ($debugChoice -ge 2) { $qemuArgs += "-no-shutdown" }
if ($debugChoice -ge 3) {
    $QemuLog = Join-Path $Root "qemu-debug.log"
    $qemuArgs += @("-d", "int,guest_errors", "-D", $QemuLog)
    Write-Host "Exception log: $QemuLog"
}

# ===== Launch =====
Write-Host "Starting QEMU..."
Write-Host "  $QemuPath $($qemuArgs -join ' ')"
Write-Host ""

& $QemuPath @qemuArgs

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "QEMU exited with code $LASTEXITCODE"
    pause
}

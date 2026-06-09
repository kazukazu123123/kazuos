param(
    [int]$BootTimeoutSeconds = 30,
    [int]$AfterWaitSeconds = 6,
    [switch]$NoBuild,
    [switch]$KeepAlive,
    [string[]]$SendLines = @("ret"),
    [string]$ExpectPattern = $null,
    [string]$WaitPattern = "KazuOS kernel started",
    [string]$EarlyPattern = "KazuOS Bootloader"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$SerialLog = Join-Path $Root "serial.log"
$QemuLog = Join-Path $Root "qemu-debug.log"
$QemuStdout = Join-Path $Root "qemu-stdout.log"
$QemuStderr = Join-Path $Root "qemu-stderr.log"
$MonitorPort = 55555

function Find-FirstExisting($paths) {
    foreach ($path in $paths) {
        if ($path -and (Test-Path $path)) { return $path }
    }
    return $null
}

function Stop-OldQemu {
    Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 300
}

function Wait-Port([string]$HostName, [int]$Port, [int]$TimeoutSeconds) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $client = [System.Net.Sockets.TcpClient]::new()
            $iar = $client.BeginConnect($HostName, $Port, $null, $null)
            if ($iar.AsyncWaitHandle.WaitOne(250)) {
                $client.EndConnect($iar)
                $client.Close()
                return $true
            }
            $client.Close()
        } catch {}
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function Wait-ForSerialPattern([string]$Pattern, [int]$TimeoutSeconds, [int]$PollMs = 300) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $SerialLog) {
            $content = Get-Content $SerialLog -Raw -ErrorAction SilentlyContinue
            if ($content -and ($content -match $Pattern)) {
                return $true
            }
        }
        Start-Sleep -Milliseconds $PollMs
    }
    return $false
}

function New-MonitorClient {
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline) {
        try { return [System.Net.Sockets.TcpClient]::new("127.0.0.1", $MonitorPort) } catch {}
        Start-Sleep -Milliseconds 250
    }
    throw "monitor connection failed"
}

function Send-HmpLines([string[]]$Lines) {
    $client = New-MonitorClient
    try {
        $stream = $client.GetStream()
        Start-Sleep -Milliseconds 200
        while ($stream.DataAvailable) { $null = $stream.ReadByte() }
        foreach ($line in $Lines) {
            $bytes = [System.Text.Encoding]::ASCII.GetBytes($line + "`n")
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush()
            Start-Sleep -Milliseconds 500
        }
    } finally {
        $client.Close()
    }
}

function Text-ToSendKeys([string]$Text) {
    $keys = New-Object System.Collections.Generic.List[string]
    foreach ($ch in $Text.ToCharArray()) {
        switch ($ch) {
            "`n" { $keys.Add("ret"); continue }
            " " { $keys.Add("spc"); continue }
            "/" { $keys.Add("slash"); continue }
            "." { $keys.Add("dot"); continue }
            "-" { $keys.Add("minus"); continue }
            "_" { $keys.Add("shift-minus"); continue }
            default { $keys.Add([string]$ch); continue }
        }
    }
    return $keys
}

function Send-TextToGuest([string]$Text) {
    $keys = (Text-ToSendKeys $Text) -join "-"
    Send-HmpLines @("sendkey $keys")
}

Stop-OldQemu
Remove-Item $SerialLog, $QemuLog, $QemuStdout, $QemuStderr -Force -ErrorAction SilentlyContinue

$EspDir = Join-Path $Root "esp"

if (!$NoBuild) {
    Write-Host "[1/4] Building ESP"

    Write-Host "  Building bootloader..."
    cargo build -p kazuos-bootloader --target x86_64-unknown-uefi --release
    if ($LASTEXITCODE -ne 0) { throw "Bootloader build failed." }

    Write-Host "  Building kernel..."
    cargo build -p kazuos-kernel --target crates/kernel/x86_64-kazuos.json '-Zbuild-std=core,alloc' '-Zbuild-std-features=compiler-builtins-mem' -Zjson-target-spec --release
    if ($LASTEXITCODE -ne 0) { throw "Kernel build failed." }

    Write-Host "  Setting up ESP..."
    if (Test-Path $EspDir) { Remove-Item $EspDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path (Join-Path $EspDir "EFI\BOOT"), (Join-Path $EspDir "KazuOS") | Out-Null
    Copy-Item (Join-Path $Root "target\x86_64-unknown-uefi\release\kazuos-bootloader.efi") `
              (Join-Path $EspDir "EFI\BOOT\BOOTX64.EFI") -Force
    Copy-Item (Join-Path $Root "target\x86_64-kazuos\release\kazuos-kernel") `
              (Join-Path $EspDir "KazuOS\kernel.elf") -Force
    $initrdPath = Join-Path $Root "target\initrd.kfs"
    if (Test-Path $initrdPath) {
        Copy-Item $initrdPath (Join-Path $EspDir "KazuOS\initrd.kfs") -Force
    } else {
        throw "initrd.kfs not found. Kernel build may have failed."
    }
    if (Test-Path (Join-Path $Root "font.ttf")) {
        Copy-Item (Join-Path $Root "font.ttf") (Join-Path $EspDir "KazuOS\font.ttf") -Force
    }
} elseif (!(Test-Path $EspDir)) {
    throw "ESP directory not found: $EspDir"
}

$OvmfPath = if ($env:OVMF_PATH) { $env:OVMF_PATH } else {
    Find-FirstExisting @(
        "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
        "C:\Program Files\qemu\share\qemu\edk2-x86_64-code.fd",
        "C:\Program Files\qemu\share\ovmf-x64\OVMF_CODE.fd",
        "C:\ProgramData\chocolatey\lib\qemu\tools\qemu\share\edk2-x86_64-code.fd",
        "C:\msys64\usr\share\qemu\edk2-x86_64-code.fd"
    )
}
if (!$OvmfPath) { throw "OVMF firmware not found" }

$QemuPath = if ($env:QEMU_PATH) { $env:QEMU_PATH } else {
    Find-FirstExisting @(
        "C:\Program Files\qemu\qemu-system-x86_64.exe",
        "C:\ProgramData\chocolatey\bin\qemu-system-x86_64.exe",
        "C:\msys64\mingw64\bin\qemu-system-x86_64.exe",
        "C:\msys64\usr\bin\qemu-system-x86_64.exe"
    )
}
if (!$QemuPath) { throw "qemu-system-x86_64.exe not found" }

$OvmfDir = Split-Path -Parent $OvmfPath
$OvmfVars = Find-FirstExisting @(
    (Join-Path $OvmfDir "edk2-i386-vars.fd"),
    (Join-Path $OvmfDir "edk2-x86_64-vars.fd"),
    (Join-Path $OvmfDir "OVMF_VARS.fd")
)
$TempVars = Join-Path $Root "ovmf_vars_pipeline.tmp.fd"
if ($OvmfVars) { Copy-Item $OvmfVars $TempVars -Force }

$qemuArgs = @(
    "-machine", "q35,pcspk-audiodev=snd0",
    "-drive", "if=pflash,format=raw,readonly=on,file=$OvmfPath"
)
if ($OvmfVars) { $qemuArgs += @("-drive", "if=pflash,format=raw,file=$TempVars") }
$qemuArgs += @(
    "-drive", "format=raw,file=fat:rw:esp",
    "-boot", "order=a,menu=on",
    "-m", "4G",
    "-net", "none",
    "-display", "none",
    "-vnc", "127.0.0.1:1",
    "-audiodev", "none,id=snd0",
    "-device", "ac97,audiodev=snd0",
    "-serial", "file:$SerialLog",
    "-monitor", "tcp:127.0.0.1:$MonitorPort,server,nowait",
    "-no-reboot",
    "-d", "int,guest_errors",
    "-D", $QemuLog
)

    Write-Host "[2/4] Starting QEMU"
$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName = $QemuPath
$argLine = ($qemuArgs | ForEach-Object {
    if ($_ -match '[\s,=:]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
}) -join ' '
$psi.Arguments = $argLine
$psi.WorkingDirectory = $Root
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.UseShellExecute = $false
$process = [System.Diagnostics.Process]::Start($psi)

try {
    Start-Sleep -Milliseconds 500
    if ($process.HasExited) {
        $process.StandardOutput.ReadToEnd() | Set-Content $QemuStdout
        $process.StandardError.ReadToEnd() | Set-Content $QemuStderr
        throw "QEMU exited immediately. See qemu-stderr.log"
    }

    Write-Host "[3/4] Waiting for monitor"
    if (!(Wait-Port "127.0.0.1" $MonitorPort 20)) { throw "QEMU monitor did not open" }

    Write-Host "[4/4] Waiting for bootloader (pattern: '$EarlyPattern')"
    if (!(Wait-ForSerialPattern $EarlyPattern $BootTimeoutSeconds)) {
        Write-Host "Warning: early pattern '$EarlyPattern' not found within ${BootTimeoutSeconds}s, trying anyway"
    }
    Start-Sleep -Seconds 1
    # Press Enter to boot the default OS
    Send-HmpLines @("sendkey ret")
    Start-Sleep -Milliseconds 500

    Write-Host "[4/4] Waiting for shell prompt (pattern: '$WaitPattern')"
    if (!(Wait-ForSerialPattern $WaitPattern $BootTimeoutSeconds)) {
        Write-Host "Warning: pattern '$WaitPattern' not found within ${BootTimeoutSeconds}s, sending commands anyway"
    }
    Start-Sleep -Seconds 5
    foreach ($line in $SendLines) {
        if ($line -eq "ret") {
            Send-HmpLines @("sendkey ret")
            Start-Sleep -Milliseconds 500
        } else {
            Send-TextToGuest "$line`n"
            Start-Sleep -Milliseconds 500
        }
    }
    Start-Sleep -Seconds $AfterWaitSeconds
} finally {
    if (!$KeepAlive -and $process -and !$process.HasExited) {
        try {
            $client = New-MonitorClient
            $stream = $client.GetStream()
            $bytes = [System.Text.Encoding]::ASCII.GetBytes("quit`n")
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush()
            Start-Sleep -Milliseconds 800
            $client.Close()
        } catch {}
        if (!$process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            Start-Sleep -Milliseconds 300
        }
    }
    if ($process) {
        try { $process.StandardOutput.ReadToEnd() | Set-Content $QemuStdout } catch {}
        try { $process.StandardError.ReadToEnd() | Set-Content $QemuStderr } catch {}
    }
}

Write-Host "[4/4] Results"
Write-Host "=== serial.log tail ==="
if (Test-Path $SerialLog) { Get-Content $SerialLog -Tail 120 } else { Write-Host "serial.log missing" }
Write-Host "=== qemu-debug.log faults ==="
if (Test-Path $QemuLog) {
    Select-String -Path $QemuLog -Pattern "check_exception|Triple|v=0d|v=0e|v=06|v=08|guest_errors" | Select-Object -Last 120 | ForEach-Object { $_.Line }
} else {
    Write-Host "qemu-debug.log missing"
}
Write-Host "=== qemu stderr ==="
if (Test-Path $QemuStderr) { Get-Content $QemuStderr -Tail 40 }

if ($ExpectPattern -and (Test-Path $SerialLog)) {
    $match = Select-String -Path $SerialLog -Pattern $ExpectPattern -Quiet
    if ($match) {
        Write-Host "[PASS] Expected pattern found: '$ExpectPattern'"
    } else {
        Write-Host "[FAIL] Expected pattern not found: '$ExpectPattern'"
        exit 1
    }
}

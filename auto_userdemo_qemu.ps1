param(
    [int]$BootWaitSeconds = 8,
    [int]$AfterWaitSeconds = 8,
    [switch]$KeepAlive
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$EspDir = Join-Path $Root "esp"
$SerialLog = Join-Path $Root "serial.log"
$QemuLog = Join-Path $Root "qemu-debug.log"
$MonitorPort = 55555

if (!(Test-Path $EspDir)) {
    throw "ESP directory not found: $EspDir"
}

$ovmfCandidates = @(
    "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    "C:\Program Files\qemu\share\qemu\edk2-x86_64-code.fd",
    "C:\Program Files\qemu\share\ovmf-x64\OVMF_CODE.fd",
    "C:\ProgramData\chocolatey\lib\qemu\tools\qemu\share\edk2-x86_64-code.fd",
    "C:\msys64\usr\share\qemu\edk2-x86_64-code.fd"
)
$OvmfPath = if ($env:OVMF_PATH) { $env:OVMF_PATH } else { $ovmfCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1 }
if (!$OvmfPath) { throw "OVMF firmware not found" }

$qemuCandidates = @(
    "C:\Program Files\qemu\qemu-system-x86_64.exe",
    "C:\ProgramData\chocolatey\bin\qemu-system-x86_64.exe",
    "C:\msys64\mingw64\bin\qemu-system-x86_64.exe",
    "C:\msys64\usr\bin\qemu-system-x86_64.exe"
)
$QemuPath = if ($env:QEMU_PATH) { $env:QEMU_PATH } else { $qemuCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1 }
if (!$QemuPath) { throw "qemu-system-x86_64.exe not found" }

$ovmfDir = Split-Path -Parent $OvmfPath
$varsCandidates = @(
    (Join-Path $ovmfDir "edk2-i386-vars.fd"),
    (Join-Path $ovmfDir "edk2-x86_64-vars.fd"),
    (Join-Path $ovmfDir "OVMF_VARS.fd")
)
$OvmfVars = $varsCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
$TempVars = Join-Path $Root "ovmf_vars_auto.tmp.fd"

Remove-Item $SerialLog, $QemuLog -Force -ErrorAction SilentlyContinue
if ($OvmfVars) { Copy-Item $OvmfVars $TempVars -Force }

$args = @(
    "-machine", "q35,pcspk-audiodev=snd0",
    "-drive", "if=pflash,format=raw,readonly=on,file=$OvmfPath"
)
if ($OvmfVars) { $args += @("-drive", "if=pflash,format=raw,file=$TempVars") }
$args += @(
    "-drive", "format=raw,file=fat:rw:esp",
    "-boot", "order=a,menu=on",
    "-m", "4G",
    "-net", "none",
    "-device", "VGA",
    "-audiodev", "dsound,id=snd0",
    "-serial", "file:$SerialLog",
        "-monitor", "telnet:localhost:$MonitorPort,server,nowait",
    "-no-reboot",
    "-d", "int,guest_errors",
    "-D", $QemuLog
)

function New-MonitorClient {
    $deadline = (Get-Date).AddSeconds(15)
    do {
        try {
            return [System.Net.Sockets.TcpClient]::new("localhost", $MonitorPort)
        } catch {
            Start-Sleep -Milliseconds 250
        }
    } while ((Get-Date) -lt $deadline)
    throw "QEMU monitor did not accept connection"
}

function Send-MonitorLine([string]$Line) {
    $client = New-MonitorClient
    try {
        $stream = $client.GetStream()
        Start-Sleep -Milliseconds 100
        while ($stream.DataAvailable) { $null = $stream.ReadByte() }
        $bytes = [System.Text.Encoding]::ASCII.GetBytes($Line + "`n")
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush()
        Start-Sleep -Milliseconds 100
    } finally {
        $client.Close()
    }
}

function Send-Key([string]$Key) {
    Send-MonitorLine "sendkey $Key"
    Start-Sleep -Milliseconds 90
}

function Send-Text([string]$Text) {
    foreach ($ch in $Text.ToCharArray()) {
        if ($ch -eq "`n") { Send-Key "ret"; continue }
        if ($ch -eq " ") { Send-Key "spc"; continue }
        Send-Key ([string]$ch)
    }
}

Write-Host "Starting QEMU with ESP: $EspDir"
$process = Start-Process -FilePath $QemuPath -ArgumentList $args -PassThru -WindowStyle Minimized
try {
    $deadline = (Get-Date).AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 200
        $portOpen = Test-NetConnection localhost -Port $MonitorPort -InformationLevel Quiet -WarningAction SilentlyContinue
    } while (!$portOpen -and (Get-Date) -lt $deadline)
    if (!$portOpen) { throw "QEMU monitor did not open" }

    Start-Sleep -Seconds $BootWaitSeconds
    Send-Key "ret"
    Start-Sleep -Seconds 2
    Send-Text "userdemo`n"
    Start-Sleep -Seconds $AfterWaitSeconds
} finally {
    if (!$KeepAlive -and !$process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 300
    }
}

Write-Host "=== serial.log tail ==="
if (Test-Path $SerialLog) { Get-Content $SerialLog -Tail 80 } else { Write-Host "serial.log missing" }
Write-Host "=== qemu-debug.log faults ==="
if (Test-Path $QemuLog) {
    Select-String -Path $QemuLog -Pattern "check_exception|Triple|v=0d|v=0e|v=06|v=08|guest_errors" | Select-Object -Last 80 | ForEach-Object { $_.Line }
} else {
    Write-Host "qemu-debug.log missing"
}

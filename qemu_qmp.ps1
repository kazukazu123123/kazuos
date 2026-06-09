<#
.SYNOPSIS
    Headless QEMU controller using QMP (QEMU Machine Protocol).

.DESCRIPTION
    Starts QEMU headlessly with QMP and VNC, provides functions to send keys,
    take screenshots, and read serial output. Can be dot-sourced to use the
    functions in other scripts, or run directly with -Action.

.EXAMPLE
    # Start QEMU and keep it running
    .\qemu_qmp.ps1 -Action start -KeepAlive

    # Send a command to the running shell
    .\qemu_qmp.ps1 -Action text -Text "help"
    .\qemu_qmp.ps1 -Action key  -Key "ret"

    # Take a screenshot (saves as .ppm, auto-converted to .png if magick available)
    .\qemu_qmp.ps1 -Action screenshot -Out screen.png

    # Start, run commands, screenshot, stop
    .\qemu_qmp.ps1 -Action run -SendText "ps" -Out screen.png

    # Stop the running QEMU
    .\qemu_qmp.ps1 -Action stop
#>
param(
    [string]$Action      = "run",     # start | stop | key | text | screenshot | serial | run
    [string]$Iso         = "kazuos.iso",
    [string]$Key         = "ret",     # QEMU qcode key name
    [string]$Text        = "",        # Text to type (for -Action text or run)
    [string]$SendText    = "",        # Alias for -Text in run mode
    [string]$Out         = "C:\Users\kazu\AppData\Local\Temp\kazuos_screen.ppm",
    [int]$QmpPort        = 4444,
    [int]$VncDisplay     = 2,         # VNC :2 = port 5902
    [int]$BootWait       = 30,        # seconds to wait for "KazuOS> "
    [int]$AfterWait      = 2,         # seconds to wait after sending text before screenshot
    [switch]$NoBuild,
    [switch]$KeepAlive,               # keep QEMU running after action
    [switch]$NoWait                   # skip waiting for shell prompt
)

$ErrorActionPreference = "Stop"
$Root       = Split-Path -Parent $MyInvocation.MyCommand.Path
$SerialLog  = Join-Path $Root "serial.log"
$PidFile    = Join-Path $Root ".qemu_qmp.pid"

# ---------------------------------------------------------------------------
# QMP helpers
# ---------------------------------------------------------------------------

function Invoke-Qmp {
    param([string]$Json, [int]$Port = $QmpPort)
    $client = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $Port)
    try {
        $stream = $client.GetStream()
        $reader = [System.IO.StreamReader]::new($stream)
        $writer = [System.IO.StreamWriter]::new($stream); $writer.AutoFlush = $true

        # Read greeting
        $null = $reader.ReadLine()

        # Negotiate capabilities
        $writer.WriteLine('{"execute":"qmp_capabilities"}')
        $null = $reader.ReadLine()

        # Send command
        $writer.WriteLine($Json)
        Start-Sleep -Milliseconds 100
        $result = ""
        while ($stream.DataAvailable) {
            $result += [char]$stream.ReadByte()
        }
        return $result
    } finally {
        $client.Close()
    }
}

function Send-QmpKey([string]$QCode) {
    $json = '{"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":"' + $QCode + '"}]}}'
    Invoke-Qmp $json | Out-Null
}

function Send-QmpText([string]$Text) {
    foreach ($ch in $Text.ToCharArray()) {
        $qcode = switch ($ch) {
            " "  { "spc" }
            "/"  { "slash" }
            "."  { "dot" }
            "-"  { "minus" }
            "_"  { "shift-minus" }
            "="  { "equal" }
            "+"  { "shift-equal" }
            default { [string]$ch }
        }
        Send-QmpKey $qcode
        Start-Sleep -Milliseconds 30
    }
}

function Send-QmpEnter { Send-QmpKey "ret" }

function Get-Screenshot([string]$Path) {
    # QMP screendump requires an absolute path the QEMU process can write to
    $ppm = $Path -replace '\.png$', '.ppm'
    # Use Unix-style path for QEMU (it runs on Windows but accepts both)
    $qemuPath = $ppm -replace '\\', '/'
    $json = '{"execute":"screendump","arguments":{"filename":"' + ($qemuPath -replace '"','\"') + '"}}'
    Invoke-Qmp $json | Out-Null
    Start-Sleep -Milliseconds 300

    # Convert PPM -> PNG if ImageMagick is available
    if ($ppm -ne $Path -and (Test-Path $ppm)) {
        if (Get-Command magick -ErrorAction SilentlyContinue) {
            magick $ppm $Path
            Remove-Item $ppm -Force -ErrorAction SilentlyContinue
        } else {
            # Rename to .ppm since no converter
            if ($Path -ne $ppm) { Copy-Item $ppm $Path -Force -ErrorAction SilentlyContinue }
        }
    }
    return $ppm
}

function Wait-ForSerial([string]$Pattern, [int]$Seconds = 30) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $SerialLog) {
            $content = Get-Content $SerialLog -Raw -ErrorAction SilentlyContinue
            if ($content -match $Pattern) { return $true }
        }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Stop-Qemu {
    if (Test-Path $PidFile) {
        $qpid = Get-Content $PidFile -ErrorAction SilentlyContinue
        if ($qpid) {
            Stop-Process -Id ([int]$qpid) -Force -ErrorAction SilentlyContinue
        }
        Remove-Item $PidFile -Force -ErrorAction SilentlyContinue
    }
    Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 400
}

function Start-Qemu([string]$IsoPath) {
    $OvmfCandidates = @(
        "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
        "C:\Program Files\qemu\share\qemu\edk2-x86_64-code.fd",
        "C:\Program Files\qemu\share\ovmf-x64\OVMF_CODE.fd"
    )
    $OvmfPath = $OvmfCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (!$OvmfPath) { throw "OVMF not found" }

    $OvmfDir  = Split-Path -Parent $OvmfPath
    $OvmfVars = @("edk2-i386-vars.fd","edk2-x86_64-vars.fd","OVMF_VARS.fd") `
        | ForEach-Object { Join-Path $OvmfDir $_ } `
        | Where-Object { Test-Path $_ } | Select-Object -First 1
    $TempVars = Join-Path $Root "ovmf_vars_qmp.tmp.fd"
    if ($OvmfVars) { Copy-Item $OvmfVars $TempVars -Force }

    $QemuExe = @(
        "C:\Program Files\qemu\qemu-system-x86_64.exe",
        "C:\ProgramData\chocolatey\bin\qemu-system-x86_64.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (!$QemuExe) { throw "qemu-system-x86_64.exe not found" }

    $args = @(
        "-machine", "q35",
        "-drive",   "if=pflash,format=raw,readonly=on,file=$OvmfPath"
    )
    if ($OvmfVars) { $args += "-drive", "if=pflash,format=raw,file=$TempVars" }
    $args += @(
        "-cdrom",   $IsoPath,
        "-boot",    "order=d,menu=on",
        "-m",       "4G",
        "-net",     "none",
        "-no-reboot",
        "-d",       "int,guest_errors",
        "-D",       (Join-Path $Root "qemu-debug.log"),
        "-display", "none",
        "-vnc",     "127.0.0.1:$VncDisplay",
        "-serial",  "file:$SerialLog",
        "-qmp",     "tcp:127.0.0.1:$QmpPort,server,nowait"
    )

    Write-Host "Starting QEMU headlessly..."
    $psi = [System.Diagnostics.ProcessStartInfo]::new($QemuExe)
    $psi.Arguments      = ($args | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } }) -join ' '
    $psi.WorkingDirectory = $Root
    $psi.UseShellExecute  = $false
    $proc = [System.Diagnostics.Process]::Start($psi)
    $proc.Id | Set-Content $PidFile
    Write-Host "QEMU PID: $($proc.Id)  QMP port: $QmpPort  VNC: 127.0.0.1:$(5900+$VncDisplay)"
    return $proc
}

function Wait-QmpReady([int]$Seconds = 15) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $c = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $QmpPort)
            $c.Close()
            return $true
        } catch { Start-Sleep -Milliseconds 300 }
    }
    return $false
}

# ---------------------------------------------------------------------------
# Actions
# ---------------------------------------------------------------------------

switch ($Action) {

    "start" {
        Stop-Qemu
        Remove-Item $SerialLog -Force -ErrorAction SilentlyContinue

        if (!$NoBuild) {
            Write-Host "Building ISO..."
            & "$Root\make_iso.ps1" -Output $Iso
        }

        $proc = Start-Qemu (Join-Path $Root $Iso)
        if (!(Wait-QmpReady 20)) { throw "QMP did not open" }
        Write-Host "QMP ready."

        # Boot: press Enter at bootloader menu
        if (!(Wait-ForSerial "KazuOS Bootloader" 20)) { Write-Warning "Bootloader prompt not seen" }
        Start-Sleep -Milliseconds 500
        Send-QmpKey "ret"

        if (!$NoWait) {
            Write-Host "Waiting for kernel start..."
            if (!(Wait-ForSerial "KazuOS kernel started" $BootWait)) { Write-Warning "Kernel start not seen in ${BootWait}s" }
            Start-Sleep -Seconds 2
        }

        if (!$KeepAlive) {
            Write-Host "QEMU started. Use -Action stop to shut it down."
            Write-Host "(PID saved to .qemu_qmp.pid)"
        }
    }

    "stop" {
        Stop-Qemu
        Write-Host "QEMU stopped."
    }

    "key" {
        Send-QmpKey $Key
        Write-Host "Sent key: $Key"
    }

    "text" {
        $t = if ($Text) { $Text } else { $SendText }
        Send-QmpText $t
        Send-QmpEnter
        Write-Host "Sent text: $t"
    }

    "screenshot" {
        $saved = Get-Screenshot $Out
        Write-Host "Screenshot saved: $saved"
    }

    "serial" {
        if (Test-Path $SerialLog) { Get-Content $SerialLog -Raw } else { Write-Host "(no serial.log)" }
    }

    "run" {
        # All-in-one: start -> boot -> send text -> screenshot -> stop
        Stop-Qemu
        Remove-Item $SerialLog -Force -ErrorAction SilentlyContinue

        if (!$NoBuild) {
            Write-Host "Building ISO..."
            & "$Root\make_iso.ps1" -Output $Iso
        }

        $proc = Start-Qemu (Join-Path $Root $Iso)
        if (!(Wait-QmpReady 20)) { throw "QMP did not open" }

        if (!(Wait-ForSerial "KazuOS Bootloader" 20)) { Write-Warning "Bootloader prompt not seen" }
        Start-Sleep -Milliseconds 500
        Send-QmpKey "ret"

        Write-Host "Waiting for kernel start..."
        if (!(Wait-ForSerial "KazuOS kernel started" $BootWait)) { Write-Warning "Kernel start not seen" }
        Start-Sleep -Seconds 2

        $cmd = if ($SendText) { $SendText } elseif ($Text) { $Text } else { $null }
        if ($cmd) {
            Write-Host "Sending: $cmd"
            Send-QmpText $cmd
            Send-QmpEnter
            Start-Sleep -Seconds $AfterWait
        }

        $saved = Get-Screenshot $Out
        Write-Host "Screenshot: $saved"

        Write-Host "=== Serial output ==="
        if (Test-Path $SerialLog) { Get-Content $SerialLog -Raw } else { Write-Host "(none)" }

        if (!$KeepAlive) { Stop-Qemu }
    }

    default { Write-Error "Unknown action: $Action. Use: start | stop | key | text | screenshot | serial | run" }
}

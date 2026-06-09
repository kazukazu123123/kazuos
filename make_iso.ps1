param(
    [string]$Output = "kazuos.iso",
    [string]$Xorriso = $env:XORRISO
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$IsoRoot = Join-Path $Root "iso_root"
$EfiBootDir = Join-Path $IsoRoot "EFI\BOOT"
$EfiDir = Join-Path $IsoRoot "EFI"
$KazuOsDir = Join-Path $IsoRoot "KazuOS"

function Find-File($name, $roots) {
    foreach ($root in $roots) {
        if ([string]::IsNullOrWhiteSpace($root)) { continue }
        if (-not (Test-Path $root)) { continue }
        $found = Get-ChildItem -Path $root -Recurse -File -Filter $name -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found) { return $found.FullName }
    }
    return $null
}

function Convert-CygwinPath($path) {
    $arg = $path -replace "\\", "/"
    if ($arg -match "^([A-Za-z]):/(.*)$") {
        return "/cygdrive/" + $matches[1].ToLowerInvariant() + "/" + $matches[2]
    }
    return $arg
}

# Build a FAT16 disk image containing all files needed by the bootloader.
# UEFI El Torito requires the EFI "boot image" to be a proper FAT filesystem,
# not a raw EFI binary.  Including kernel.elf/initrd.kfs here avoids any
# dependency on OVMF having an ISO 9660 driver at boot time.
#
# Layout on disk:
#   Sector 0              Boot Sector (BPB)
#   Sectors 1..nFAT       FAT1
#   Sectors ..            FAT2
#   Sectors ..            Root directory  (512 entries, fixed in FAT16)
#   Data area (cluster 2+):
#     cluster  2          EFI/ directory
#     cluster  3          EFI/BOOT/ directory
#     cluster  4          KazuOS/ directory
#     clusters 5..        BOOTX64.EFI  data
#     clusters ..         kernel.elf   data
#     clusters ..         initrd.kfs   data
#     clusters ..         font.ttf     data (optional)
function Build-FatImage {
    param(
        [string]$BootloaderPath,
        [string]$KernelPath,
        [string]$InitrdPath,
        [string]$FontPath = $null
    )

    $SS  = 512
    $SPC = 8        # sectors per cluster = 4 KB; good for FAT16 up to ~256 MB
    $CB  = $SS * $SPC

    $bootData   = [System.IO.File]::ReadAllBytes($BootloaderPath)
    $kernelData = [System.IO.File]::ReadAllBytes($KernelPath)
    $initrdData = [System.IO.File]::ReadAllBytes($InitrdPath)
    $fontData   = if ($FontPath -and (Test-Path $FontPath)) { [System.IO.File]::ReadAllBytes($FontPath) } else { $null }

    function NClusters([byte[]]$d) { [int][Math]::Ceiling($d.Length / $CB) }

    $Nb = NClusters $bootData
    $Nk = NClusters $kernelData
    $Ni = NClusters $initrdData
    $Nf = if ($fontData) { NClusters $fontData } else { 0 }

    # Fixed cluster assignments
    $cEfi   = [uint16]2
    $cBoot  = [uint16]3
    $cKazuOS = [uint16]4
    $cBootX64First = [uint16]5
    $cKernelFirst  = [uint16](5 + $Nb)
    $cInitrdFirst  = [uint16](5 + $Nb + $Nk)
    $cFontFirst    = [uint16](5 + $Nb + $Nk + $Ni)

    $totalDataClusters = 3 + $Nb + $Nk + $Ni + $Nf
    $maxCluster        = 2 + $totalDataClusters  # highest cluster index + 1

    # FAT16: 2 bytes per entry; entries 0..$maxCluster-1
    $nFATSectors = [int][Math]::Ceiling($maxCluster * 2 / $SS)
    $nReserved   = 1
    $nFATs       = 2
    $nRootEnts   = 512
    $nRootSec    = [int][Math]::Ceiling($nRootEnts * 32 / $SS)
    $nDataSec    = $totalDataClusters * $SPC
    $nTotalSec   = $nReserved + $nFATs * $nFATSectors + $nRootSec + $nDataSec

    $img = [byte[]]::new($nTotalSec * $SS)

    # ---- Boot Sector ----
    $img[0] = 0xEB; $img[1] = 0x5A; $img[2] = 0x90
    [System.Text.Encoding]::ASCII.GetBytes("MSDOS5.0").CopyTo($img, 3)
    [BitConverter]::GetBytes([uint16]$SS).CopyTo($img, 11)
    $img[13] = [byte]$SPC
    [BitConverter]::GetBytes([uint16]$nReserved).CopyTo($img, 14)
    $img[16] = [byte]$nFATs
    [BitConverter]::GetBytes([uint16]$nRootEnts).CopyTo($img, 17)
    if ($nTotalSec -le 65535) {
        [BitConverter]::GetBytes([uint16]$nTotalSec).CopyTo($img, 19)
    } else {
        [BitConverter]::GetBytes([uint16]0).CopyTo($img, 19)
        [BitConverter]::GetBytes([uint32]$nTotalSec).CopyTo($img, 32)
    }
    $img[21] = 0xF8
    [BitConverter]::GetBytes([uint16]$nFATSectors).CopyTo($img, 22)
    [BitConverter]::GetBytes([uint16]63).CopyTo($img, 24)
    [BitConverter]::GetBytes([uint16]255).CopyTo($img, 26)
    [BitConverter]::GetBytes([uint32]0).CopyTo($img, 28)
    $img[36] = 0x80
    $img[38] = 0x29
    $img[39] = 0x78; $img[40] = 0x56; $img[41] = 0x34; $img[42] = 0x12
    [System.Text.Encoding]::ASCII.GetBytes("EFI SYSTEM ").CopyTo($img, 43)
    [System.Text.Encoding]::ASCII.GetBytes("FAT16   ").CopyTo($img, 54)
    $img[510] = 0x55; $img[511] = 0xAA

    # ---- FAT16 table ----
    # Write one FAT16 entry (16-bit little-endian)
    function SetFAT([int]$n, [uint16]$v) {
        $off = $nReserved * $SS + $n * 2
        $img[$off]     = [byte]($v -band 0xFF)
        $img[$off + 1] = [byte](($v -shr 8) -band 0xFF)
    }
    # Write a file cluster chain starting at $first, length $count
    function WriteChain([uint16]$first, [int]$count) {
        for ($i = 0; $i -lt $count; $i++) {
            $next = if ($i -lt $count - 1) { [uint16]($first + $i + 1) } else { [uint16]0xFFFF }
            SetFAT ($first + $i) $next
        }
    }

    SetFAT 0 0xFFF8  # media type
    SetFAT 1 0xFFFF  # reserved
    SetFAT $cEfi    0xFFFF   # EFI/ dir       (1 cluster)
    SetFAT $cBoot   0xFFFF   # EFI/BOOT/ dir  (1 cluster)
    SetFAT $cKazuOS 0xFFFF   # KazuOS/ dir    (1 cluster)
    WriteChain $cBootX64First $Nb
    WriteChain $cKernelFirst  $Nk
    WriteChain $cInitrdFirst  $Ni
    if ($Nf -gt 0) { WriteChain $cFontFirst $Nf }

    # Copy FAT1 -> FAT2
    $fat2Off = ($nReserved + $nFATSectors) * $SS
    [Array]::Copy($img, $nReserved * $SS, $img, $fat2Off, $nFATSectors * $SS)

    # ---- Directory entry helpers ----
    function To83([string]$name) {
        $dot = $name.LastIndexOf(".")
        if ($dot -ge 0) {
            $n = $name.Substring(0, $dot).PadRight(8).Substring(0, 8)
            $e = $name.Substring($dot + 1).PadRight(3).Substring(0, 3)
            return ($n + $e).ToUpper()
        }
        return $name.PadRight(11).Substring(0, 11).ToUpper()
    }

    function WriteDirEnt([int]$off, [string]$name83, [byte]$attr, [uint16]$clus, [uint32]$sz) {
        $nb = [System.Text.Encoding]::ASCII.GetBytes($name83.PadRight(11).Substring(0, 11))
        [Array]::Copy($nb, 0, $img, $off, 11)
        $img[$off + 11] = $attr
        $img[$off + 26] = [byte]($clus -band 0xFF)
        $img[$off + 27] = [byte](($clus -shr 8) -band 0xFF)
        $img[$off + 28] = [byte]($sz -band 0xFF)
        $img[$off + 29] = [byte](($sz -shr 8) -band 0xFF)
        $img[$off + 30] = [byte](($sz -shr 16) -band 0xFF)
        $img[$off + 31] = [byte](($sz -shr 24) -band 0xFF)
    }

    $rootBase = ($nReserved + $nFATs * $nFATSectors) * $SS
    $dataBase = $rootBase + $nRootSec * $SS

    function ClusterOffset([uint16]$c) { $dataBase + ($c - 2) * $SPC * $SS }

    # ---- Root directory ----
    WriteDirEnt ($rootBase + 0 * 32)  (To83 "EFI")    0x10 $cEfi    0
    WriteDirEnt ($rootBase + 1 * 32)  (To83 "KazuOS") 0x10 $cKazuOS 0

    # ---- EFI/ directory (cluster 2) ----
    $d = ClusterOffset $cEfi
    WriteDirEnt ($d + 0 * 32) ".          " 0x10 $cEfi    0
    WriteDirEnt ($d + 1 * 32) "..         " 0x10 0         0
    WriteDirEnt ($d + 2 * 32) (To83 "BOOT")     0x10 $cBoot   0

    # ---- EFI/BOOT/ directory (cluster 3) ----
    $d = ClusterOffset $cBoot
    WriteDirEnt ($d + 0 * 32) ".          " 0x10 $cBoot  0
    WriteDirEnt ($d + 1 * 32) "..         " 0x10 $cEfi   0
    WriteDirEnt ($d + 2 * 32) (To83 "BOOTX64.EFI") 0x20 $cBootX64First ([uint32]$bootData.Length)

    # ---- KazuOS/ directory (cluster 4) ----
    $d = ClusterOffset $cKazuOS
    WriteDirEnt ($d + 0 * 32) ".          " 0x10 $cKazuOS 0
    WriteDirEnt ($d + 1 * 32) "..         " 0x10 0         0
    WriteDirEnt ($d + 2 * 32) (To83 "kernel.elf")  0x20 $cKernelFirst ([uint32]$kernelData.Length)
    WriteDirEnt ($d + 3 * 32) (To83 "initrd.kfs")  0x20 $cInitrdFirst ([uint32]$initrdData.Length)
    if ($fontData) {
        WriteDirEnt ($d + 4 * 32) (To83 "font.ttf") 0x20 $cFontFirst ([uint32]$fontData.Length)
    }

    # ---- File data ----
    [Array]::Copy($bootData,   0, $img, (ClusterOffset $cBootX64First), $bootData.Length)
    [Array]::Copy($kernelData, 0, $img, (ClusterOffset $cKernelFirst),  $kernelData.Length)
    [Array]::Copy($initrdData, 0, $img, (ClusterOffset $cInitrdFirst),  $initrdData.Length)
    if ($fontData) {
        [Array]::Copy($fontData, 0, $img, (ClusterOffset $cFontFirst), $fontData.Length)
    }

    return ,$img
}

if ([string]::IsNullOrWhiteSpace($Xorriso)) {
    $localXorriso = Find-File "xorriso.exe" @((Join-Path $Root "tools\xorriso"))
    if ($localXorriso) { $Xorriso = $localXorriso }
}
if ([string]::IsNullOrWhiteSpace($Xorriso)) {
    $cmd = Get-Command xorriso.exe -ErrorAction SilentlyContinue
    if (-not $cmd) { $cmd = Get-Command xorriso -ErrorAction SilentlyContinue }
    if ($cmd) { $Xorriso = $cmd.Source }
}
if ([string]::IsNullOrWhiteSpace($Xorriso)) { throw "xorriso not found." }

Write-Host "Building bootloader..."
cargo build -p kazuos-bootloader --target x86_64-unknown-uefi --release
if ($LASTEXITCODE -ne 0) { throw "Bootloader build failed." }

Write-Host "Building kernel..."
cargo build -p kazuos-kernel --target crates/kernel/x86_64-kazuos.json '-Zbuild-std=core,alloc' '-Zbuild-std-features=compiler-builtins-mem' -Zjson-target-spec --release
if ($LASTEXITCODE -ne 0) { throw "Kernel build failed." }

if (Test-Path $IsoRoot) { Remove-Item $IsoRoot -Recurse -Force }
New-Item -ItemType Directory -Force -Path $IsoRoot, $EfiBootDir, $EfiDir, $KazuOsDir | Out-Null

$BootloaderEfi = Join-Path $Root "target\x86_64-unknown-uefi\release\kazuos-bootloader.efi"
$KernelElf     = Join-Path $Root "target\x86_64-kazuos\release\kazuos-kernel"
$InitrdKfs     = Join-Path $Root "target\initrd.kfs"
$FontTtf       = Join-Path $Root "font.ttf"

Copy-Item $BootloaderEfi (Join-Path $EfiBootDir "BOOTX64.EFI") -Force
Copy-Item $KernelElf     (Join-Path $KazuOsDir  "kernel.elf")  -Force
if (-not (Test-Path $InitrdKfs)) { throw "initrd.kfs not found. Kernel build may have failed." }
Copy-Item $InitrdKfs (Join-Path $KazuOsDir "initrd.kfs") -Force
if (Test-Path $FontTtf) { Copy-Item $FontTtf (Join-Path $KazuOsDir "font.ttf") -Force }

# Build a FAT16 EFI boot image containing all files the bootloader needs.
# This is the El Torito "boot image": UEFI mounts it as an EFI System Partition
# and loads EFI/BOOT/BOOTX64.EFI from it.  Having kernel.elf and initrd.kfs
# here means the bootloader can find them without an ISO 9660 driver.
Write-Host "Building EFI FAT image..."
$fontArg = if (Test-Path $FontTtf) { $FontTtf } else { $null }
$fatBytes = Build-FatImage -BootloaderPath $BootloaderEfi `
                           -KernelPath     $KernelElf `
                           -InitrdPath     $InitrdKfs `
                           -FontPath       $fontArg
$efiImgPath = Join-Path $EfiDir "efi.img"
[System.IO.File]::WriteAllBytes($efiImgPath, $fatBytes)
Write-Host "  efi.img: $([Math]::Round($fatBytes.Length / 1MB, 1)) MB"

$OutputPath  = if ([System.IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $Root $Output }
if (Test-Path $OutputPath) { Remove-Item $OutputPath -Force }
$IsoRootArg = Convert-CygwinPath (Resolve-Path $IsoRoot).Path
$OutputArg  = Convert-CygwinPath $OutputPath

Write-Host "Creating ISO..."
& $Xorriso -as mkisofs `
    -R -r -J `
    -V KAZUOS `
    -eltorito-platform efi `
    -b EFI/efi.img `
    -no-emul-boot `
    -o $OutputArg `
    $IsoRootArg
if ($LASTEXITCODE -ne 0) { throw "ISO creation failed." }

Write-Host "Created $OutputPath"

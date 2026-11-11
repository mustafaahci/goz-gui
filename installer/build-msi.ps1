<#
.SYNOPSIS
Builds goz-gui's MSI from a release build.

.DESCRIPTION
Makes License.rtf from LICENSE for the installer's license page, then runs
`wix build` on installer/goz-gui.wxs and writes goz-gui-v<version>-x64.msi.
Prints the .msi's path.

Needs WiX v5 and its UI extension, for example:

    dotnet tool install --global wix --version 5.0.2
    wix extension add -g WixToolset.UI.wixext/5.0.2

.EXAMPLE
cargo build --release --locked
./installer/build-msi.ps1 -Validate
# target\installer\goz-gui-v0.1.0-x64.msi
#>
[CmdletBinding()]
param(
    # The goz-gui.exe to package.
    [string] $Exe = (Join-Path $PSScriptRoot '..\target\release\goz-gui.exe'),
    # Where the .msi goes (made if missing).
    [string] $OutDir = (Join-Path $PSScriptRoot '..\target\installer'),
    # The version; Cargo.toml's when not given.
    [string] $Version,
    # The wix command, or a path to wix.exe.
    [string] $Wix = 'wix',
    # The UI extension: its name in WiX's extension cache, or a path to its .dll.
    [string] $UiExtension = 'WixToolset.UI.wixext',
    # The Start menu shortcut's System.AppUserModel.ID; none when empty.
    [string] $AppUserModelId,
    # Also run the Windows Installer ICE validation on the .msi.
    [switch] $Validate
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Exe = (Resolve-Path $Exe).Path
$license = Join-Path $root 'LICENSE'
$icon = Join-Path $root 'assets\goz.ico'

if (-not $Version) {
    $line = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' |
        Select-Object -First 1
    if (-not $line) { throw 'Cargo.toml has no version line' }
    $Version = $line.Matches[0].Groups[1].Value
}

# A Windows Installer version is numbers only (major and minor at most 255,
# build at most 65535): 1.2.3-beta.1 installs as 1.2.3.
$msiVersion = ($Version -split '[-+]')[0]
$m = [regex]::Match($msiVersion, '^(\d+)\.(\d+)\.(\d+)$')
if (-not $m.Success -or [int]$m.Groups[1].Value -gt 255 -or [int]$m.Groups[2].Value -gt 255 -or
    [int]$m.Groups[3].Value -gt 65535) {
    throw "Version '$Version' is not one Windows Installer can take (X.Y.Z)"
}

# LICENSE as RTF: its paragraphs rejoined (the text file wraps at 80
# columns, the license page wraps by itself), the title in bold.
function ConvertTo-Rtf([string] $Source) {
    $paragraphs = ($Source -replace "`r`n", "`n").Trim() -split "`n[ \t]*`n"
    $rtf = New-Object System.Text.StringBuilder
    [void]$rtf.Append('{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fswiss\fcharset0 Segoe UI;}}')
    [void]$rtf.Append('\viewkind4\uc1\pard\sa160\f0\fs18 ')
    $first = $true
    foreach ($paragraph in $paragraphs) {
        $joined = (($paragraph -split "`n") | ForEach-Object { $_.Trim() }) -join ' '
        $escaped = New-Object System.Text.StringBuilder
        foreach ($ch in $joined.ToCharArray()) {
            $code = [int]$ch
            if ($ch -eq [char]'\' -or $ch -eq [char]'{' -or $ch -eq [char]'}') {
                [void]$escaped.Append('\').Append($ch)
            } elseif ($code -gt 127) {
                if ($code -gt 32767) { $code -= 65536 }
                [void]$escaped.Append("\u$code?")
            } else {
                [void]$escaped.Append($ch)
            }
        }
        if ($first) {
            [void]$rtf.Append('\b ').Append($escaped.ToString()).Append('\b0\par ')
            $first = $false
        } else {
            [void]$rtf.Append($escaped.ToString()).Append('\par ')
        }
    }
    [void]$rtf.Append('}')
    $rtf.ToString()
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$msi = Join-Path $OutDir "goz-gui-v$Version-x64.msi"
$work = Join-Path ([IO.Path]::GetTempPath()) "goz-gui-msi-$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null

try {
    $rtfFile = Join-Path $work 'License.rtf'
    [IO.File]::WriteAllText($rtfFile, (ConvertTo-Rtf ([IO.File]::ReadAllText($license))),
        [Text.Encoding]::ASCII)

    $wixArgs = @(
        'build', (Join-Path $PSScriptRoot 'goz-gui.wxs'),
        '-arch', 'x64',
        '-ext', $UiExtension,
        '-d', "Version=$msiVersion",
        '-d', "ExeFile=$Exe",
        '-d', "LicenseFile=$license",
        '-d', "LicenseRtf=$rtfFile",
        '-d', "IconFile=$icon",
        '-intermediatefolder', (Join-Path $work 'obj'),
        '-pdbtype', 'none',
        '-o', $msi
    )
    if ($AppUserModelId) { $wixArgs += @('-d', "AppUserModelId=$AppUserModelId") }
    & $Wix @wixArgs | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "wix build failed (exit code $LASTEXITCODE)" }

    if ($Validate) {
        # ICE91 only warns that a per-user folder is not copied to each
        # profile in a per-machine install, which this package never is.
        & $Wix msi validate -sice ICE91 -intermediateFolder (Join-Path $work 'ice') $msi | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "ICE validation failed (exit code $LASTEXITCODE)" }
    }
} finally {
    Remove-Item -Recurse -Force -Path $work -ErrorAction SilentlyContinue
}

$msi

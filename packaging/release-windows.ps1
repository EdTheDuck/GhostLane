# Builds the Windows installer, signs the exe and the installer with the Certum certificate, and uploads the
# installer to the GitHub release for the version in Cargo.toml. Run after the tag's release workflow finished,
# with SimplySign Desktop connected so the certificate is in the Windows certificate store.
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object FullName | Select-Object -Last 1
if (-not $signtool) { throw 'signtool.exe not found; install the Windows SDK' }

function Invoke-Checked {
    & $args[0] $args[1..($args.Count - 1)]
    if ($LASTEXITCODE) { throw "$($args -join ' ') failed" }
}

function Sign($file) {
    Invoke-Checked $signtool.FullName sign /n 'Open Source Developer' /fd sha256 /tr http://time.certum.pl /td sha256 $file
}

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$installer = "dist\ghostlane_${version}_x64-setup.exe"

Invoke-Checked cargo build --release
Sign target\release\ghostlane.exe
Invoke-Checked cargo packager --release --formats nsis
# The packager's own build must not have replaced the signed exe that went into the installer.
Invoke-Checked $signtool.FullName verify /pa target\release\ghostlane.exe
Sign $installer
Invoke-Checked gh release upload "v$version" $installer --clobber

# Install a Plix Windows archive into a user-selected directory.
param(
    [string]$Prefix = "$HOME\\.local"
)
$source = Split-Path -Parent $MyInvocation.MyCommand.Path
$binary = Join-Path $source "bin\\plix.exe"
if (-not (Test-Path $binary)) {
    throw "Expected $binary. Run this script from an extracted Plix archive."
}
$destination = Join-Path $Prefix "bin"
New-Item -ItemType Directory -Force -Path $destination | Out-Null
Copy-Item $binary (Join-Path $destination "plix.exe") -Force
Write-Host "Installed Plix to $destination\\plix.exe"
Write-Host "Add $destination to PATH, then run: plix --version"

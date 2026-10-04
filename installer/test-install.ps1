# CI check of the installer: silent install, files, shortcut and registry, then silent
# uninstall and nothing left. Usage: pwsh installer/test-install.ps1 dist/RetroGit-windows-x64-setup.exe
param([string]$Setup)
$ErrorActionPreference = "Stop"
$app = Join-Path $env:LOCALAPPDATA "Programs\RetroGit"
$link = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\RetroGit.lnk"
$keys = @("HKCU:\Software\Classes\retrogit\shell\open\command", "HKCU:\Software\Classes\AppUserModelId\RetroGit")

Start-Process $Setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait
if (-not (Test-Path (Join-Path $app "retrogit.exe"))) { throw "retrogit.exe not installed in $app" }
if (-not (Test-Path $link)) { throw "no Start menu shortcut at $link" }
foreach ($k in $keys) { if (-not (Test-Path $k)) { throw "missing registry key $k" } }
$command = (Get-ItemProperty $keys[0]).'(default)'
if ($command -notlike "*retrogit.exe*%1*") { throw "unexpected link handler: $command" }
Write-Host "installed: $command"

# An update runs the installer again with /RELAUNCH: it must start RetroGit afterwards.
# Checked in the installer's log (the CI machine has no screen: the window itself may not
# open), then any RetroGit left running is stopped.
$log = Join-Path $env:TEMP "retrogit-relaunch.log"
Start-Process $Setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RELAUNCH", "/LOG=`"$log`"" -Wait
$text = Get-Content $log -Raw
$ran = $text -match "(?s)-- Run entry --.*?Filename: [^\r\n]*retrogit\.exe"
if (-not $ran) {
  Write-Host $text
  throw "/RELAUNCH did not start RetroGit"
}
Start-Sleep 3
Get-Process retrogit -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 2
Write-Host "relaunched after a silent update"

Start-Process (Join-Path $app "unins000.exe") -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait
# The uninstaller runs from a copy in the temporary folder: wait for it to finish.
for ($i = 0; $i -lt 60 -and (Test-Path (Join-Path $app "retrogit.exe")); $i++) { Start-Sleep 1 }
if (Test-Path (Join-Path $app "retrogit.exe")) { throw "retrogit.exe still installed" }
if (Test-Path $link) { throw "Start menu shortcut left" }
foreach ($k in $keys) { if (Test-Path $k) { throw "registry key left: $k" } }
Write-Host "uninstalled cleanly"

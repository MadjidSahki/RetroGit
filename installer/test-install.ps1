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

# An update runs the installer again with /RELAUNCH: RetroGit is started afterwards.
Start-Process $Setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RELAUNCH" -Wait
$started = $null
for ($i = 0; $i -lt 30 -and -not $started; $i++) {
  $started = Get-Process retrogit -ErrorAction SilentlyContinue
  if (-not $started) { Start-Sleep 1 }
}
if (-not $started) { throw "/RELAUNCH did not start RetroGit" }
$started | Stop-Process -Force
Start-Sleep 2
Write-Host "relaunched after a silent update"

Start-Process (Join-Path $app "unins000.exe") -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait
# The uninstaller runs from a copy in the temporary folder: wait for it to finish.
for ($i = 0; $i -lt 60 -and (Test-Path (Join-Path $app "retrogit.exe")); $i++) { Start-Sleep 1 }
if (Test-Path (Join-Path $app "retrogit.exe")) { throw "retrogit.exe still installed" }
if (Test-Path $link) { throw "Start menu shortcut left" }
foreach ($k in $keys) { if (Test-Path $k) { throw "registry key left: $k" } }
Write-Host "uninstalled cleanly"

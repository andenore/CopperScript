# Only run on an ephemeral GitHub-hosted runner. No project sources execute
# until the downloaded official installer passes the committed SHA-256 pin.
$ErrorActionPreference = 'Stop'
$ciToolchain = Get-Content .github/board-toolchain.json -Raw | ConvertFrom-Json
$ciInstaller = Join-Path $env:RUNNER_TEMP 'kicad-installer.exe'
Invoke-WebRequest $ciToolchain.kicad_installer_url -OutFile $ciInstaller
$ciDigest = (Get-FileHash -LiteralPath $ciInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ciDigest -ne $ciToolchain.kicad_installer_sha256) { throw 'KiCad installer checksum mismatch' }
$ciInstall = Start-Process -FilePath $ciInstaller -ArgumentList '/S' -WindowStyle Hidden -PassThru
if (-not $ciInstall.WaitForExit(900000)) { throw 'KiCad installation exceeded 15 minutes' }
if ($ciInstall.ExitCode -ne 0) { throw "KiCad installation failed: $($ciInstall.ExitCode)" }
$ciCli = 'C:\Program Files\KiCad\10.0\bin\kicad-cli.exe'
$ciVersion = & $ciCli version
if ($LASTEXITCODE -ne 0 -or $ciVersion.Trim() -ne $ciToolchain.kicad_version) {
    throw "Unexpected KiCad version: $ciVersion"
}
"KICAD_CLI=$ciCli" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
'KICAD10_FOOTPRINT_DIR=C:\Program Files\KiCad\10.0\share\kicad\footprints' |
    Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append

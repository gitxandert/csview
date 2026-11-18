# Small install script for Windows: builds csview for release and installs it to
# %LOCALAPPDATA%\Programs\csview. Runs in Windows PowerShell 5.1 and PowerShell 7+.
# If that directory isn't on your user PATH yet, it adds it.
#
# Run it with:
#     powershell -ExecutionPolicy Bypass -File .\install_csview.ps1

$ErrorActionPreference = 'Stop'

# Build
Write-Host 'Building csview...'
$oldFlags = $env:RUSTFLAGS
$env:RUSTFLAGS = '-Awarnings'
try {
    cargo build --release --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit code $LASTEXITCODE)" }
} finally {
    $env:RUSTFLAGS = $oldFlags
}

# Install the binary
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\csview'
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Write-Host "Installing csview to $installDir..."
Copy-Item -Force (Join-Path $PSScriptRoot 'target\release\csview.exe') (Join-Path $installDir 'csview.exe')

# Read the user PATH straight from the registry so %VARIABLES% in it stay unexpanded.
$envKey = 'HKCU:\Environment'
$key = Get-Item $envKey
$userPath = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
$entries = @($userPath -split ';' | Where-Object { $_ -ne '' })

if ($entries | Where-Object { $_.TrimEnd('\') -ieq $installDir }) {
    Write-Host "$installDir is already on your PATH."
    exit 0
}

Write-Host "Adding $installDir to your user PATH..."
$newPath = ($entries + $installDir) -join ';'
New-ItemProperty -Force -Path $envKey -Name 'Path' -Value $newPath -PropertyType ExpandString | Out-Null

# Writing the registry doesn't tell running programs PATH changed. Setting a user
# variable through .NET broadcasts WM_SETTINGCHANGE, so new terminals opened from
# Explorer or Windows Terminal pick up the new PATH; then remove the variable again.
[Environment]::SetEnvironmentVariable('CSVIEW_INSTALLER', '1', 'User')
[Environment]::SetEnvironmentVariable('CSVIEW_INSTALLER', $null, 'User')

# Also make it usable in this session right away.
$env:Path = "$env:Path;$installDir"
Write-Host 'Open a new terminal to use csview.'

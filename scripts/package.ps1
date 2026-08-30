<#
.SYNOPSIS
    Builds the application and packages it for another machine.

.DESCRIPTION
    Produces dist\Measure\ and a zip beside it, holding the executable, the
    PDFium library it loads at run time, and the notices that travel with both.
    Nothing else is needed on the machine it is unzipped onto: no toolchain, no
    installer, no system-wide library.

    The PDFium library is not built here and is not in the repository. Put a
    copy, and the LICENSE file that came with it, in runtime\ before running
    this.
#>

[CmdletBinding()]
param(
    [string] $Runtime = '',
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Resolve-Path (Join-Path $here '..')
if (-not $Runtime) { $Runtime = Join-Path $root 'runtime' }
Set-Location $root

# The version the package is named for, from the manifest that defines it.
$version = ''
foreach ($line in Get-Content 'Cargo.toml') {
    if ($line -match '^version\s*=\s*"([^"]+)"') { $version = $Matches[1]; break }
}
if (-not $version) { throw 'No version found in Cargo.toml' }

# The library has to come from somewhere: it is a native binary this
# repository neither builds nor carries.
$library = Join-Path $Runtime 'pdfium.dll'
if (-not (Test-Path $library)) {
    throw @"
No PDFium library at $library

Put a Windows build of the PDFium dynamic library there, along with the
LICENSE file that came with it. The library is loaded at run time and is not
built from this repository; the crate `pdfium-render` documents where builds
are published.
"@
}

$licence = Get-ChildItem -Path $Runtime -File |
    Where-Object { $_.Name -match '^(PDFIUM-)?LICEN[CS]E' } |
    Select-Object -First 1

if (-not $licence) {
    throw @"
No PDFium licence file in $Runtime

PDFium is offered under a BSD-3-Clause licence, which requires its notice to
travel with the binary. Put the LICENSE file that came with the library there,
so it can be packaged beside it.
"@
}

if (-not $SkipBuild) {
    Write-Host 'Building...'
    & cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'the release build failed' }
}

$exe = Join-Path $root 'target\release\Measure.exe'
if (-not (Test-Path $exe)) { throw "No executable at $exe" }

$dist = Join-Path $root 'dist'
$folder = Join-Path $dist 'Measure'

if (Test-Path $folder) { Remove-Item -Recurse -Force $folder }
New-Item -ItemType Directory -Force -Path $folder | Out-Null

Copy-Item $exe $folder
Copy-Item $library $folder
Copy-Item $licence.FullName (Join-Path $folder 'PDFIUM-LICENSE.txt')
Copy-Item (Join-Path $root 'README.md') $folder
Copy-Item (Join-Path $root 'THIRD-PARTY-NOTICES.md') $folder

$zip = Join-Path $dist "Measure-$version.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $folder '*') -DestinationPath $zip

Write-Host ''
Write-Host "Packaged Measure $version"
foreach ($file in Get-ChildItem $folder -File | Sort-Object Name) {
    Write-Host ("  {0,-28} {1,10:N0} bytes" -f $file.Name, $file.Length)
}
Write-Host ''
Write-Host ("  {0}  ({1:N0} bytes)" -f $zip, (Get-Item $zip).Length)
Write-Host ''
Write-Host 'Unzip it anywhere and run Measure.exe. Windows will warn that the'
Write-Host 'file is unsigned the first time it is opened on a machine.'

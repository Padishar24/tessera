<#
.SYNOPSIS
  Builds the MSIX packages and the .msixbundle for the Microsoft Store.

.DESCRIPTION
  Takes the release builds of tessera.exe and tesserac.exe per architecture,
  lays out one package folder per architecture (manifest, assets,
  resources.pri, licenses), packs each into an .msix and bundles them.

  The bundle is unsigned: the Store signs it on submission. To try a package
  locally, enable Developer Mode and register a layout folder:
      Add-AppxPackage -Register target\msix\layout-x64\AppxManifest.xml

  Needs the Windows SDK (makeappx.exe, makepri.exe).

.EXAMPLE
  cargo build --release
  pwsh packaging/msix/build.ps1 -X64 target/release

.EXAMPLE
  pwsh packaging/msix/build.ps1 -X64 bin/x64 -Arm64 bin/arm64 `
      -ThirdPartyLicenses metadata/THIRD_PARTY_LICENSES.html `
      -IdentityName 12345Example.Tessera -Publisher "CN=..." -PublisherDisplayName Example
#>
[CmdletBinding()]
param(
    # Folder with tessera.exe and tesserac.exe for x64 / ARM64 (at least one).
    [string] $X64,
    [string] $Arm64,
    # Output folder.
    [string] $Out = 'target/msix',
    # THIRD_PARTY_LICENSES.html from cargo-about; included when given.
    [string] $ThirdPartyLicenses,
    # From Partner Center -> Product identity. The defaults only suit local tests.
    [string] $IdentityName = 'Tessera.Local',
    [string] $Publisher = 'CN=TesseraLocal',
    [string] $PublisherDisplayName = 'Tessera (local build)'
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot/../.."

function Invoke-Tool([string] $exe, [string[]] $arguments) {
    & $exe @arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "$(Split-Path -Leaf $exe) failed with exit code $LASTEXITCODE" }
}

function Find-SdkTool([string] $name) {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $tool = Get-ChildItem $kits -Directory -Filter '10.*' -ErrorAction SilentlyContinue |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\$name" } |
        Where-Object { Test-Path $_ } |
        Select-Object -First 1
    if (-not $tool) { throw "$name not found; install the Windows SDK." }
    $tool
}

$archs = [ordered]@{}
if ($X64) { $archs.x64 = $X64 }
if ($Arm64) { $archs.arm64 = $Arm64 }
if ($archs.Count -eq 0) { throw 'Pass -X64 and/or -Arm64.' }

if ($IdentityName -eq 'Tessera.Local') {
    $message = 'Using placeholder package identity: fine for a local test, rejected by the Store. ' +
        'Pass -IdentityName, -Publisher and -PublisherDisplayName from Partner Center.'
    if ($env:GITHUB_ACTIONS) { Write-Host "::warning::$message" } else { Write-Warning $message }
}

# Store packages need a four-part version whose last part is 0.
$cargoVersion = (Select-String -Path "$root/Cargo.toml" -Pattern '^version = "(\d+\.\d+\.\d+)"').Matches[0].Groups[1].Value
$version = "$cargoVersion.0"

$makeappx = Find-SdkTool 'makeappx.exe'
$makepri = Find-SdkTool 'makepri.exe'
Write-Host "Tessera $version, SDK tools from $(Split-Path $makeappx)"

New-Item -ItemType Directory -Force $Out | Out-Null
$Out = Resolve-Path $Out
$bundleSource = Join-Path $Out 'bundle'
Remove-Item -Recurse -Force $bundleSource -ErrorAction SilentlyContinue
New-Item -ItemType Directory $bundleSource | Out-Null

$priConfig = Join-Path $Out 'priconfig.xml'
Invoke-Tool $makepri @('createconfig', '/cf', $priConfig, '/dq', 'en-US', '/pv', '10.0.0', '/o')
# Without <packaging>, all scale variants go into the one resources.pri
# instead of separate resource-pack PRIs that a single package never loads.
$config = [xml](Get-Content -Raw $priConfig)
$packaging = $config.SelectSingleNode('/resources/packaging')
if ($packaging) { [void]$packaging.ParentNode.RemoveChild($packaging) }
$config.Save($priConfig)

$template = Get-Content -Raw "$PSScriptRoot/AppxManifest.xml"
foreach ($arch in $archs.Keys) {
    $layout = Join-Path $Out "layout-$arch"
    Remove-Item -Recurse -Force $layout -ErrorAction SilentlyContinue
    New-Item -ItemType Directory $layout | Out-Null

    foreach ($exe in 'tessera.exe', 'tesserac.exe') {
        Copy-Item (Join-Path $archs[$arch] $exe) $layout
    }
    Copy-Item "$root/LICENSE" "$layout/LICENSE.txt"
    if ($ThirdPartyLicenses) { Copy-Item $ThirdPartyLicenses "$layout/THIRD_PARTY_LICENSES.html" }
    Copy-Item -Recurse "$PSScriptRoot/Assets" "$layout/Assets"

    $manifest = $template.
        Replace('$IDENTITY_NAME$', [Security.SecurityElement]::Escape($IdentityName)).
        Replace('$PUBLISHER$', [Security.SecurityElement]::Escape($Publisher)).
        Replace('$PUBLISHER_DISPLAY_NAME$', [Security.SecurityElement]::Escape($PublisherDisplayName)).
        Replace('$VERSION$', $version).
        Replace('$ARCH$', $arch)
    [IO.File]::WriteAllText("$layout/AppxManifest.xml", $manifest, [Text.UTF8Encoding]::new($false))

    # resources.pri maps Assets\Square44x44Logo.png etc. to the scale and
    # target-size variants.
    Invoke-Tool $makepri @('new', '/pr', $layout, '/cf', $priConfig, '/mn', "$layout/AppxManifest.xml",
        '/of', "$layout/resources.pri", '/o')

    $msix = Join-Path $bundleSource "Tessera_${version}_$arch.msix"
    Invoke-Tool $makeappx @('pack', '/d', $layout, '/p', $msix, '/o')
}

$bundle = Join-Path $Out "Tessera_$version.msixbundle"
Invoke-Tool $makeappx @('bundle', '/d', $bundleSource, '/p', $bundle, '/bv', $version, '/o')
Write-Host "Bundle: $bundle"

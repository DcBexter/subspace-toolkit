param([string]$Dotnet = 'dotnet', [string]$CacheDirectory = '')
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$dist = Join-Path $repo 'dist'
$package = Join-Path $dist 'SubspaceToolkit'
$payload = Join-Path $package 'payload'
$sources = Join-Path $package 'sources'
if (!$CacheDirectory) { $CacheDirectory = Join-Path $dist 'downloads' }
New-Item -ItemType Directory -Force $CacheDirectory, $sources | Out-Null
function Download($name, $url, $sha256) {
    $file = Join-Path $CacheDirectory $name
    if (!(Test-Path -LiteralPath $file)) { Invoke-WebRequest -Uri $url -OutFile $file }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $sha256) { throw "Checksum mismatch: $name" }
    return [IO.Path]::GetFullPath($file)
}
function Remove-BuildDirectory($path) {
    $resolved = [IO.Path]::GetFullPath($path)
    if (!$resolved.StartsWith($dist + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Build cleanup escapes dist' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
$melon = Download 'MelonLoader-0.7.3-x64.zip' 'https://github.com/LavaGang/MelonLoader/releases/download/v0.7.3/MelonLoader.x64.zip' '5b2b2f3d1cd42b59ec886c5bdc2663edae87a0097a4f4a8f58c0965a99dda416'
$runtime = Download 'dotnet-runtime-6.0.36-win-x64.zip' 'https://builds.dotnet.microsoft.com/dotnet/Runtime/6.0.36/dotnet-runtime-6.0.36-win-x64.zip' 'c077ccc342c5571ceb7a3c06d1dfbc99b989ed5a6e0f3a0b0b450531c1259615'
$compatibility = Download 'SubspaceToolkit-0.1.0-windows-x64.zip' 'https://github.com/DcBexter/subspace-toolkit/releases/download/v0.1.0/SubspaceToolkit-0.1.0-windows-x64.zip' '52d3624c4e9322296c2f3517161b8b14d1e8a0038c799bedb65d0af73b3a9e34'
Remove-BuildDirectory $payload
Expand-Archive -LiteralPath $melon -DestinationPath $payload
Expand-Archive -LiteralPath $runtime -DestinationPath (Join-Path $payload 'MelonLoader/Dependencies/dotnet')
# Keep the game-tested compatibility library byte-identical across toolkit upgrades.
$archive = [IO.Compression.ZipFile]::OpenRead($compatibility)
try {
    foreach ($name in @('payload/MelonLoader/net6/Il2CppInterop.Runtime.dll', 'sources/Il2CppInterop-source.zip', 'sources/Il2CppInterop-LICENSE.txt', 'sources/interop-291.patch')) {
        $entry = $archive.GetEntry($name)
        if (!$entry) { throw "Compatibility archive missing $name" }
        [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $package $name), $true)
    }
} finally {
    $archive.Dispose()
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'LOADER.md') -Destination (Join-Path $sources 'README.md')
& $Dotnet build (Join-Path $repo 'mod/SubspaceToolkit.csproj') -c Release "-p:GameDirectory=$payload" --nologo
if ($LASTEXITCODE) { throw 'Camera mod build failed' }
New-Item -ItemType Directory -Force (Join-Path $payload 'Mods'), (Join-Path $payload 'UserData') | Out-Null
Copy-Item -LiteralPath (Join-Path $repo 'mod/bin/Release/net6.0/SubspaceToolkit.dll') -Destination (Join-Path $payload 'Mods/SubspaceToolkit.dll')
[IO.File]::WriteAllText((Join-Path $payload 'UserData/Loader.cfg'), "[loader]`ncapture_player_logs = false`nforce_quit = false`n`n[console]`nhide_console = true`n")
$files = @(Get-ChildItem -LiteralPath $payload -File -Recurse | Sort-Object FullName | ForEach-Object {
    [ordered]@{path=[IO.Path]::GetRelativePath($payload, $_.FullName).Replace('\','/'); sha256=(Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant()}
})
foreach ($required in @('version.dll', 'Mods/SubspaceToolkit.dll', 'MelonLoader/net6/Il2CppInterop.Runtime.dll', 'MelonLoader/Dependencies/dotnet/shared/Microsoft.NETCore.App/6.0.36/coreclr.dll')) {
    if ($required -notin $files.path) { throw "Payload missing $required" }
}
if ($files.path -match '(^dxgi\.dll$|Il2CppAssemblies|GameAssembly|global-metadata|StarTrekOutpostsUnknown_Data)') { throw 'Forbidden game/proxy files in payload' }
[IO.File]::WriteAllText((Join-Path $payload 'manifest.json'), ([ordered]@{schema=1;files=$files} | ConvertTo-Json -Depth 4))
Write-Output "Prepared $($files.Count) loader/mod/runtime files; corresponding patched source and licenses included."

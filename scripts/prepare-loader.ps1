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
$interop = Download 'Il2CppInterop-f03c8f4.zip' 'https://codeload.github.com/BepInEx/Il2CppInterop/zip/f03c8f4ae507d47ea814f3d11d1ec6b0391c1576' '85c68282251b4fe86c864ea02f2a107ce04bef07ad39c2246d02516f0e4c475d'
$stage = Join-Path $dist ('.loader-build-' + [guid]::NewGuid().ToString('N'))
try {
    Remove-BuildDirectory $payload
    Expand-Archive -LiteralPath $melon -DestinationPath $payload
    Expand-Archive -LiteralPath $runtime -DestinationPath (Join-Path $payload 'MelonLoader/Dependencies/dotnet')
    Expand-Archive -LiteralPath $interop -DestinationPath $stage
    $source = Join-Path $stage 'Il2CppInterop-f03c8f4ae507d47ea814f3d11d1ec6b0391c1576'
    $patch = Join-Path $PSScriptRoot 'interop-291.patch'
    & git -C $repo apply --unsafe-paths "--directory=$source" --check $patch
    if ($LASTEXITCODE) { throw 'Interop patch preflight failed' }
    & git -C $repo apply --unsafe-paths "--directory=$source" $patch
    if ($LASTEXITCODE) { throw 'Interop patch failed' }
    $hook = [IO.File]::ReadAllText((Join-Path $source 'Il2CppInterop.Runtime/Injection/Hooks/Class_GetFieldDefaultValue_Hook.cs'))
    if (!$hook.Contains('getStaticFieldValueTargets.Count == 2 &&')) { throw 'Interop fallback missing after patch' }
    Copy-Item -LiteralPath (Join-Path $source 'LICENSE') -Destination (Join-Path $sources 'Il2CppInterop-LICENSE.txt')
    Copy-Item -LiteralPath $patch -Destination $sources
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'LOADER.md') -Destination (Join-Path $sources 'README.md')
    Compress-Archive -Path $source -DestinationPath (Join-Path $sources 'Il2CppInterop-source.zip') -Force
    & $Dotnet build (Join-Path $source 'Il2CppInterop.Runtime/Il2CppInterop.Runtime.csproj') -c Release -p:Version=1.5.1-ci.845 -p:GeneratePackageOnBuild=false "-p:PathMap=$source=/_/Il2CppInterop" -p:SourceRevisionId=f03c8f4ae507d47ea814f3d11d1ec6b0391c1576 --nologo --verbosity quiet -clp:ErrorsOnly
    if ($LASTEXITCODE) { throw 'Interop build failed' }
    Copy-Item -LiteralPath (Join-Path $source 'bin/Il2CppInterop.Runtime/net6.0/Il2CppInterop.Runtime.dll') -Destination (Join-Path $payload 'MelonLoader/net6/Il2CppInterop.Runtime.dll')
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
} finally {
    Remove-BuildDirectory $stage
}

param([string]$Dotnet = 'dotnet')
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$package = Join-Path $repo 'dist/SubspaceToolkit'
$loader = Join-Path $package 'payload/MelonLoader/net6'
$live = Join-Path $package 'live'
$generator = Join-Path $live 'Generator'
New-Item -ItemType Directory -Force $live, $generator | Out-Null
if (!(Test-Path -LiteralPath (Join-Path $loader 'Il2CppInterop.Runtime.dll'))) { throw 'Run prepare-loader.ps1 first' }

function Copy-Dependencies([string[]]$names, [string]$destination) {
    $pending = [Collections.Generic.Queue[string]]::new()
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in $names) { $pending.Enqueue($name) }
    while ($pending.Count) {
        $name = $pending.Dequeue()
        if (!$seen.Add($name)) { continue }
        $file = Join-Path $loader ($name + '.dll')
        if (!(Test-Path -LiteralPath $file)) { continue }
        Copy-Item -LiteralPath $file -Destination $destination
        foreach ($reference in [Reflection.Assembly]::LoadFile($file).GetReferencedAssemblies()) { $pending.Enqueue($reference.Name) }
    }
}

& $Dotnet build (Join-Path $repo 'live/SubspaceLive.csproj') -c Release --nologo
if ($LASTEXITCODE) { throw 'Live bridge build failed' }
& cargo build --manifest-path (Join-Path $repo 'live/host/Cargo.toml') --release --locked --target-dir (Join-Path $repo 'target/live-host')
if ($LASTEXITCODE) { throw 'Native live host build failed' }
& $Dotnet build (Join-Path $repo 'live/Generator/SubspaceInterop.csproj') -c Release --nologo
if ($LASTEXITCODE) { throw 'Interop helper build failed' }
Copy-Item -LiteralPath (Join-Path $repo 'target/live-host/release/subspace_live_host.dll') -Destination (Join-Path $live 'SubspaceLiveHost.dll')
foreach ($name in @('SubspaceLive.dll','SubspaceLive.deps.json','SubspaceLive.runtimeconfig.json')) { Copy-Item -LiteralPath (Join-Path $repo "live/bin/Release/net6.0/$name") -Destination $live }
foreach ($name in @('SubspaceInterop.dll','SubspaceInterop.deps.json','SubspaceInterop.runtimeconfig.json')) { Copy-Item -LiteralPath (Join-Path $repo "live/Generator/bin/Release/net6.0/$name") -Destination $generator }
Copy-Dependencies @('0Harmony','Il2CppInterop.Runtime','Il2CppInterop.Common','Il2CppInterop.HarmonySupport','MonoMod.RuntimeDetour','MonoMod.Backports','MonoMod.ILHelpers') $live
Copy-Dependencies @('Il2CppInterop.Generator','Il2CppInterop.Common','AsmResolver.DotNet','AsmResolver','AsmResolver.PE','AsmResolver.PE.File') $generator
$runtime = Join-Path $package 'payload/MelonLoader/Dependencies/dotnet'
New-Item -ItemType Directory -Force (Join-Path $live 'runtime') | Out-Null
Copy-Item -Path (Join-Path $runtime '*') -Destination (Join-Path $live 'runtime') -Recurse -Force

$cpp = Join-Path $repo 'dist/downloads/Cpp2IL-2022.1.0-pre-release.21.exe'
if (!(Test-Path -LiteralPath $cpp)) {
    $local = Join-Path $repo 'analysis/tools/Cpp2IL.exe'
    if (Test-Path -LiteralPath $local) { Copy-Item -LiteralPath $local -Destination $cpp }
    else { Invoke-WebRequest 'https://github.com/SamboyCoding/Cpp2IL/releases/download/2022.1.0-pre-release.21/Cpp2IL-2022.1.0-pre-release.21-Windows.exe' -OutFile $cpp }
}
if ((Get-FileHash -LiteralPath $cpp).Hash.ToLowerInvariant() -ne '663fb432433b4371fd1ee0ebc321a8fff2a9aac5ac4230c843f9e03ddee4e04c') { throw 'Cpp2IL checksum mismatch' }
Copy-Item -LiteralPath $cpp -Destination (Join-Path $generator 'Cpp2IL.exe')
Invoke-WebRequest 'https://raw.githubusercontent.com/SamboyCoding/Cpp2IL/58fc404ac503f4e512055cafc48c03088fc6e224/LICENSE' -OutFile (Join-Path $generator 'Cpp2IL-LICENSE.txt')
Copy-Item -LiteralPath (Join-Path $repo 'live/README.md') -Destination $live
Write-Output 'Live host, managed bridge, private runtime and local wrapper generator prepared. No game files included.'

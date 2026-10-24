param([Parameter(Mandatory)][string]$GameDirectory, [string]$CacheDirectory = '', [string]$Dotnet = 'dotnet')
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
& $Dotnet build (Join-Path $repo 'live/Generator/SubspaceInterop.csproj') -c Release --nologo
if ($LASTEXITCODE) { throw 'Interop preparation tool build failed' }
$output = Join-Path $repo 'live/Generator/bin/Release/net6.0'
$cpp = Join-Path $repo 'analysis/tools/Cpp2IL.exe'
if (!(Test-Path -LiteralPath $cpp)) { throw 'Build live package first or provide analysis/tools/Cpp2IL.exe from pinned official release' }
Copy-Item -LiteralPath $cpp -Destination $output
Copy-Item -Path (Join-Path $repo 'dist/SubspaceToolkit/payload/MelonLoader/net6/*.dll') -Destination $output
$runtime = Join-Path $repo 'dist/SubspaceToolkit/payload/MelonLoader/Dependencies/dotnet/dotnet.exe'
$arguments = @((Join-Path $output 'SubspaceInterop.dll'), $GameDirectory)
if ($CacheDirectory) { $arguments += $CacheDirectory }
& $runtime @arguments
if ($LASTEXITCODE) { throw 'Interop generation failed' }

# Bundled loader sources and notices

This package uses [MelonLoader 0.7.3](https://github.com/LavaGang/MelonLoader/releases/tag/v0.7.3) (Apache-2.0), its included dependencies, and the private Microsoft .NET 6.0.36 runtime. Their original licenses and notices remain in `payload/MelonLoader/Documentation` and `payload/MelonLoader/Dependencies/dotnet`.

`Il2CppInterop.Runtime.dll` is modified LGPL-3.0-only software by BepInEx/knah and contributors. `Il2CppInterop-LICENSE.txt` includes LGPLv3 and GPLv3. `Il2CppInterop-source.zip` contains the complete corresponding source at commit `f03c8f4ae507d47ea814f3d11d1ec6b0391c1576`, including the local Unity 6000.3 fallback backport from [PR #291](https://github.com/BepInEx/Il2CppInterop/pull/291). The original modification is separately recorded in `interop-291.patch`; do not apply it a second time to the included source archive.

To rebuild this library, extract the source ZIP, install .NET SDK 8.0 (tested 8.0.426), then run inside the extracted source directory:

```powershell
dotnet build Il2CppInterop.Runtime/Il2CppInterop.Runtime.csproj -c Release -p:Version=1.5.1-ci.845 -p:GeneratePackageOnBuild=false "-p:PathMap=$((Get-Location).Path)=/_/Il2CppInterop" -p:SourceRevisionId=f03c8f4ae507d47ea814f3d11d1ec6b0391c1576
```

Output: `bin/Il2CppInterop.Runtime/net6.0/Il2CppInterop.Runtime.dll`. The library remains replaceable in the installed game's `MelonLoader/net6` directory; modification and reverse engineering for debugging such modifications are permitted. The toolkit refuses to overwrite a different existing library. Keep modified files when removing or upgrading the toolkit.

The toolkit build script `scripts/prepare-loader.ps1` in [its source repository](https://github.com/DcBexter/subspace-toolkit) downloads checksum-pinned originals and builds the mod without game assemblies. It reuses the game-tested compatibility library and its corresponding sources from release 0.1.0: rebuilding the library can change its bytes and cause installation conflicts with an existing loader. The source rebuild above remains available for modifications. Build the complete payload with PowerShell 7, .NET SDK 8.0.426 or 10.0.402 and that script. No game files, game dumps or generated game assemblies are included.

Compatibility is currently tested only with Outposts build 273416. This local Interop backport is not an upstream release. .NET 6 is retained for this loader's tested compatibility; upgrading the loader/runtime requires a fresh startup and shutdown test.

# Live trainer runtime

Rust GUI loads `SubspaceLiveHost.dll` into the running Outposts process and calls exported `Start`. CLR initialization happens after `LoadLibraryW` returns, never inside `DllMain`. Host accepts compatible existing CLR initialization, and managed bootstrap shares `AssemblyLoadContext.Default` with an already loaded MelonLoader.

Without MelonLoader, `Generator/SubspaceInterop.dll` prepares wrappers in `%LOCALAPPDATA%/SubspaceToolkit/interop`, keyed to the game binary, metadata and generation tools. Unity reference libraries download once. No generated game assemblies ship in this package, and preparation never writes into the game folder.

Bootstrap initializes standalone Il2CppInterop/Harmony only when not already active. Game changes run on Unity's main thread: existing Melon update event, otherwise a native camera Update callback. The callback publishes its original trampoline before activation; Harmony feature hooks are then installed on the main thread. Shared Engine owns all camera/resource/build behavior and trainer/permanent precedence. Closing trainer releases its lease; bootstrap stays idle in process until game exit. It does not install startup persistence.

Private .NET 6 runtime and Il2CppInterop dependencies match the tested permanent-loader build. Their notices and complete modified Interop sources are included in package `sources/` and `payload/MelonLoader/Documentation`. Cpp2IL is MIT-licensed; its license accompanies the generator.

Source compilation and cache-generation tests do not prove live detour compatibility. Main-thread attach, every feature, detach/reconnect and shutdown require game tests.

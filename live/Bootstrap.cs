using System.Diagnostics;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.RegularExpressions;
using HarmonyLib;
using Il2CppInterop.HarmonySupport;
using Il2CppInterop.Runtime.Injection;
using Il2CppInterop.Runtime.Startup;
using MonoMod.RuntimeDetour;
using IDetour = Il2CppInterop.Runtime.Injection.IDetour;

namespace SubspaceToolkit.Live;

public static class Bootstrap
{
    static readonly string LogPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "SubspaceToolkit", $"live-bootstrap-{Environment.ProcessId}.log");
    static readonly List<(object Source, Delegate Callback)> callbacks = new();
    static Engine? engine;
    static HarmonyLib.Harmony? harmony;
    static IDetour? updateHook;
    static Update? originalUpdate;
    static string gameRoot = "";
    static bool stopped, initialized;
    static string? lastError;

    [UnmanagedCallersOnly]
    public static int NativeEntry(nint unused, int size)
    {
        try
        {
            string location = typeof(Bootstrap).Assembly.Location;
            string folder = Path.GetDirectoryName(location)!;
            // hostfxr uses a component context; the game and existing Melon use Default.
            AssemblyLoadContext.Default.Resolving += (context, name) => Resolve(context, name, folder);
            var assembly = AssemblyLoadContext.Default.LoadFromAssemblyPath(location);
            return (int)assembly.GetType(typeof(Bootstrap).FullName!)!.GetMethod(nameof(Initialize), BindingFlags.NonPublic | BindingFlags.Static)!.Invoke(null, null)!;
        }
        catch (Exception error) { Log(error.GetBaseException().ToString()); return 1; }
    }

    static Assembly? Resolve(AssemblyLoadContext context, AssemblyName name, string folder)
    {
        var existing = context.Assemblies.FirstOrDefault(a => a.GetName().Name == name.Name);
        if (existing != null) return existing;
        string path = Path.Combine(folder, name.Name + ".dll");
        return File.Exists(path) ? context.LoadFromAssemblyPath(path) : null;
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    static int Initialize()
    {
        if (initialized) return 0;
        gameRoot = Path.GetDirectoryName(Environment.ProcessPath)!;
        if (!string.Equals(Path.GetFileName(Environment.ProcessPath), "StarTrekOutpostsUnknown.exe", StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("Wrong target process");
        nint assembly = NativeLibrary.Load(Path.Combine(gameRoot, "GameAssembly.dll"));
        T Export<T>(string name) where T : Delegate => Marshal.GetDelegateForFunctionPointer<T>(NativeLibrary.GetExport(assembly, name));
        var current = Export<GetPointer>("il2cpp_thread_current");
        nint attached = current() == 0 ? Export<Attach>("il2cpp_thread_attach")(Export<GetPointer>("il2cpp_domain_get")()) : 0;
        try { return InitializeAttached(); }
        finally { if (attached != 0) Export<Detach>("il2cpp_thread_detach")(attached); }
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate nint GetPointer();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate nint Attach(nint domain);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate void Detach(nint thread);

    [MethodImpl(MethodImplOptions.NoInlining)]
    static int InitializeAttached()
    {
        var melon = AssemblyLoadContext.Default.Assemblies.FirstOrDefault(a => a.GetName().Name == "MelonLoader");
        if (melon == null) PrepareWrappers(gameRoot);
        InitializeInterop();
        harmony = new HarmonyLib.Harmony("DcBexter.SubspaceToolkit.Live");
        try {
        if (melon != null)
        {
            Subscribe(melon, "OnUpdate", nameof(Tick));
            Subscribe(melon, "OnApplicationDefiniteQuit", nameof(Shutdown));
        }
        else
        {
            var camera = Assembly.Load("Il2CppMFGSimClient").GetType("Il2CppMFGClient.Camera.MFGSimCameraController", true)!;
            var update = camera.GetMethod("Update", BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic, null, Type.EmptyTypes, null);
            if (update?.ReturnType != typeof(void)) throw new MissingMethodException("Camera Update contract unavailable");
            var field = Il2CppInterop.Common.Il2CppInteropUtils.GetIl2CppMethodInfoPointerFieldForGeneratedMethod(update);
            nint methodInfo = (nint)(field?.GetValue(null) ?? throw new MissingFieldException("Camera Update metadata unavailable"));
            // The game is already updating: publish the original before enabling the callback.
            updateHook = new Native<Update>(Marshal.ReadIntPtr(methodInfo), CameraUpdate);
            originalUpdate = updateHook.GenerateTrampoline<Update>();
            updateHook.Apply();
        }
        initialized = true;
        AppDomain.CurrentDomain.ProcessExit += (_, _) => Shutdown();
        Log("Live bridge scheduled on game main thread");
        return 0;
        } catch {
            updateHook?.Dispose();
            updateHook = null;
            foreach (var callback in callbacks)
                try { callback.Source.GetType().GetMethod("Unsubscribe", new[] { callback.Callback.GetType() })!.Invoke(callback.Source, new object[] { callback.Callback }); } catch (Exception) { }
            callbacks.Clear();
            try { harmony.UnpatchSelf(); } catch (Exception) { }
            harmony = null;
            throw;
        }
    }

    static void PrepareWrappers(string root)
    {
        string index = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "SubspaceToolkit", "interop", "active.json");
        using var document = JsonDocument.Parse(File.ReadAllText(index));
        var data = document.RootElement;
        static string Normalized(string path) {
            path = Path.GetFullPath(path).TrimEnd('\\');
            return path.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase) ? @"\\" + path[8..] : path.StartsWith(@"\\?\", StringComparison.Ordinal) ? path[4..] : path;
        }
        if (!Normalized(data.GetProperty("game_dir").GetString()!).Equals(Normalized(root), StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("Interop cache belongs to another game");
        string Hash(string file) { using var stream = File.OpenRead(file); using var sha = SHA256.Create(); return Convert.ToHexString(sha.ComputeHash(stream)).ToLowerInvariant(); }
        if (Hash(Path.Combine(root, "GameAssembly.dll")) != data.GetProperty("game_assembly_sha256").GetString()
            || Hash(Path.Combine(root, "StarTrekOutpostsUnknown_Data", "il2cpp_data", "Metadata", "global-metadata.dat")) != data.GetProperty("metadata_sha256").GetString())
            throw new InvalidOperationException("Interop cache is stale; regenerate wrappers");
        string wrappers = data.GetProperty("wrappers_dir").GetString()!;
        AssemblyLoadContext.Default.Resolving += (context, name) => Resolve(context, name, wrappers);
        // Metadata remains local; this package never distributes game-generated assemblies.
        foreach (string file in Directory.GetFiles(wrappers, "*.dll")) AssemblyLoadContext.Default.LoadFromAssemblyPath(Path.GetFullPath(file));
    }

    static void InitializeInterop()
    {
        try { _ = Il2CppInteropRuntime.Instance; return; }
        catch (InvalidOperationException) { }
        string version = FileVersionInfo.GetVersionInfo(Path.Combine(gameRoot, "UnityPlayer.dll")).FileVersion ?? "";
        var match = Regex.Match(version, @"^(\d+)\.(\d+)\.(\d+)");
        if (!match.Success) throw new InvalidOperationException("Unity version unavailable");
        var unity = new Version(int.Parse(match.Groups[1].Value), int.Parse(match.Groups[2].Value), int.Parse(match.Groups[3].Value));
        Il2CppInteropRuntime.Create(new RuntimeConfiguration { UnityVersion = unity, DetourProvider = new Detours() }).AddHarmonySupport().Start();
    }

    static void Subscribe(Assembly melon, string field, string method)
    {
        object source = melon.GetType("MelonLoader.MelonEvents", true)!.GetField(field)!.GetValue(null)!;
        var subscribe = source.GetType().GetMethods().Single(m => m.Name == "Subscribe" && m.GetParameters().Length == 3);
        var callback = Delegate.CreateDelegate(subscribe.GetParameters()[0].ParameterType, typeof(Bootstrap).GetMethod(method, BindingFlags.Static | BindingFlags.NonPublic)!);
        subscribe.Invoke(source, new object[] { callback, -1000, false });
        callbacks.Add((source, callback));
    }

    static void Tick()
    {
        if (stopped) return;
        try
        {
            engine ??= new Engine(harmony!, gameRoot, Log);
            engine.Tick();
        }
        catch (Exception error)
        {
            string message = error.GetBaseException().ToString();
            if (lastError != message) { lastError = message; Log(message); }
            try {
                string status = Path.Combine(Path.GetDirectoryName(LogPath)!, $"status-{Environment.ProcessId}.json");
                string temp = status + ".bootstrap.tmp";
                File.WriteAllText(temp, JsonSerializer.Serialize(new { schema = 1, bridge_version = 3, pid = Environment.ProcessId, game_dir = gameRoot,
                    updated_utc_ms = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(), compatible = false, camera_ready = false, error = message }));
                File.Move(temp, status, true);
            } catch (Exception) { }
        }
    }
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate void Update(nint instance, nint method);
    static void CameraUpdate(nint instance, nint method) { originalUpdate!(instance, method); Tick(); }
    static void Shutdown() { stopped = true; engine?.Shutdown(); }
    static void Log(string message)
    {
        try { Directory.CreateDirectory(Path.GetDirectoryName(LogPath)!); File.AppendAllText(LogPath, $"{DateTime.UtcNow:O} {message}\n"); } catch (IOException) { }
    }

    sealed class Detours : IDetourProvider
    {
        public IDetour Create<TDelegate>(nint original, TDelegate target) where TDelegate : Delegate => new Native<TDelegate>(original, target);
    }
    sealed class Native<TDelegate> : IDetour where TDelegate : Delegate
    {
        readonly NativeDetour hook;
        readonly Delegate target, trampoline;
        public nint Target { get; }
        public nint Detour { get; }
        public nint OriginalTrampoline { get; }
        public Native(nint original, TDelegate callback)
        {
            Target = original;
            target = callback;
            Detour = Marshal.GetFunctionPointerForDelegate(callback);
            hook = new NativeDetour(original, Detour, new NativeDetourConfig { ManualApply = true });
            try { trampoline = ((MethodInfo)hook.GenerateTrampoline(callback.GetType().GetMethod("Invoke"))).CreateDelegate(callback.GetType()); }
            catch { hook.Dispose(); throw; }
            OriginalTrampoline = Marshal.GetFunctionPointerForDelegate(trampoline);
        }
        public void Apply() { if (!hook.IsApplied) hook.Apply(); }
        public T GenerateTrampoline<T>() where T : Delegate => Marshal.GetDelegateForFunctionPointer<T>(OriginalTrampoline);
        public void Dispose() { hook.Dispose(); GC.KeepAlive(target); GC.KeepAlive(trampoline); }
    }
}

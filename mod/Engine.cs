using System.Reflection;
using System.Text.Json;
using HarmonyLib;
using System.Collections;

namespace SubspaceToolkit;

public sealed class Engine
{
    const BindingFlags InstanceFlags = BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic;
    readonly int pid = Environment.ProcessId;
    readonly string directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "SubspaceToolkit");
    readonly string[] names = { "maxZoomDistance", "directionalMoveSpeed", "panSpeed" };
    PropertyInfo? instance, pointer, distance, minimum, crewController, zoomPercent;
    static Engine? active;
    readonly HarmonyLib.Harmony harmony;
    PropertyInfo[] fields = Array.Empty<PropertyInfo>();
    MethodInfo? alive;
    object? camera;
    nint cameraPointer;
    float[] original = Array.Empty<float>(), applied = Array.Empty<float>();
    long nextTick;
    float originalMinimum;
    bool compatible, stopped, cameraFault;
    string? error, lastLoggedError;
    ResourceHighlight? resourceHighlight;
    readonly string gameRoot;
    readonly Action<string> log;
    public const string ActiveEngineKey = "SubspaceToolkit.ActiveEngine";
    string PermanentPath => Path.Combine(gameRoot, "UserData", "SubspaceToolkit.json");
    string LivePath => Path.Combine(directory, $"live-{pid}.json");
    string StatusPath => Path.Combine(directory, $"status-{pid}.json");

    public Engine(HarmonyLib.Harmony harmony, string gameRoot, Action<string> log)
    {
        this.gameRoot = gameRoot;
        this.log = log;
        this.harmony = harmony;
        if (!string.Equals(Path.GetFileName(Environment.ProcessPath), "StarTrekOutpostsUnknown.exe", StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("Wrong target process");
        Directory.CreateDirectory(directory);
        TakeOverPrevious();
        resourceHighlight = new ResourceHighlight(harmony);
        active = this;
        AppDomain.CurrentDomain.SetData(ActiveEngineKey, this);
    }

    void TakeOverPrevious()
    {
        var previous = AppDomain.CurrentDomain.GetData(ActiveEngineKey);
        if (previous != null)
        {
            if (previous.GetType().GetMethod(nameof(Suspend))?.Invoke(previous, null) is not true)
                throw new InvalidOperationException("Previous toolkit could not restore camera ownership");
            return;
        }
        // Transitional handoff from the earlier camera-only build, on Unity's main thread.
        var melon = AppDomain.CurrentDomain.GetAssemblies().FirstOrDefault(a => a.GetName().Name == "MelonLoader");
        var registered = melon?.GetType("MelonLoader.MelonBase")?.GetProperty("RegisteredMelons")?.GetValue(null) as IEnumerable;
        if (registered == null) return;
        foreach (object mod in registered)
        {
            var type = mod.GetType();
            if (type.FullName != "SubspaceToolkit.CameraMod" || type.Assembly.GetName().Name != "SubspaceToolkit") continue;
            var stoppedField = type.GetField("stopped", InstanceFlags);
            if (stoppedField == null) continue;
            object? Value(string name) => type.GetField(name, InstanceFlags)?.GetValue(mod);
            var oldCamera = Value("camera");
            var oldFields = Value("fields") as PropertyInfo[];
            var oldApplied = Value("applied") as float[];
            var oldOriginal = Value("original") as float[];
            var oldAlive = Value("alive") as MethodInfo;
            bool valid = oldCamera != null && oldAlive?.Invoke(null, new[] { oldCamera }) is true;
            if (valid && (oldFields?.Length != 3 || oldApplied?.Length != 3 || oldOriginal?.Length != 3
                || oldFields.Where((field, i) => (float)field.GetValue(oldCamera)! != oldApplied[i]).Any()))
                throw new InvalidOperationException("Legacy camera changed externally; handoff refused");
            type.GetMethod("Restore", InstanceFlags)!.Invoke(mod, null);
            if (valid && oldFields!.Where((field, i) => (float)field.GetValue(oldCamera)! != oldOriginal![i]).Any())
                throw new InvalidOperationException("Legacy camera restoration failed");
            stoppedField.SetValue(mod, true);
            var highlight = Value("resourceHighlight");
            highlight?.GetType().GetMethod("Stop")?.Invoke(highlight, null);
            type.Assembly.GetType("SubspaceToolkit.RepeatBuild")?.GetMethod("Stop")?.Invoke(null, null);
            log("Previous camera mod suspended; original values restored");
        }
    }

    public bool Suspend()
    {
        if (!Restore()) return false;
        resourceHighlight?.Stop();
        RepeatBuild.Stop();
        stopped = true;
        return true;
    }

    public void Tick()
    {
        long now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        if (stopped || now < nextTick) return;
        nextTick = now + 200;
        try
        {
            var desired = SelectSettings(LivePath, PermanentPath, now);
            resourceHighlight!.Tick(desired?.ResourceHighlight ?? false);
            RepeatBuild.Configure(desired?.RepeatBuild ?? false);
            if (instance == null) Bind();
            var candidate = instance!.GetValue(null);
            if (candidate == null || !(bool)alive!.Invoke(null, new[] { candidate })!)
            {
                camera = null;
                cameraPointer = 0;
                Publish(now, false);
                return;
            }
            var identity = (nint)pointer!.GetValue(candidate)!;
            if (cameraPointer != identity)
            {
                Restore();
                camera = candidate;
                cameraPointer = identity;
                original = fields.Select(p => (float)p.GetValue(camera)!).ToArray();
                originalMinimum = (float)minimum!.GetValue(camera)!;
                if (!original.All(v => float.IsFinite(v) && v > 0 && v <= 10000)
                    || original[0] < 300 || original[0] > 1000
                    || !float.IsFinite(originalMinimum) || originalMinimum <= 0 || originalMinimum >= 300)
                    throw new InvalidOperationException("Unsupported original camera values");
                applied = (float[])original.Clone();
                cameraFault = false;
                error = null;
                log("Camera connected through named IL2CPP members");
            }
            if (!cameraFault)
            {
                float zoom = desired?.Zoom ?? original[0], speed = desired?.Speed ?? 1;
                var values = new[] { zoom, original[1] * speed, original[2] * speed };
                for (int i = 0; i < fields.Length; i++)
                    if ((float)fields[i].GetValue(camera)! != applied[i])
                        throw new InvalidOperationException("Camera changed by another mod; stopped writing");
                for (int i = 0; i < fields.Length; i++)
                {
                    if (applied[i] == values[i]) continue;
                    fields[i].SetValue(camera, values[i]);
                    applied[i] = values[i];
                }
                error = null;
            }
            Publish(now, true);
        }
        catch (Exception ex)
        {
            error = ex.GetBaseException().Message;
            if (ex is FormatException)
            {
                resourceHighlight?.Stop();
                RepeatBuild.Stop();
                try { Restore(); } catch (Exception) { cameraFault = true; }
            }
            // Windows can deny an atomic rename while another process reads the status file.
            else if (camera != null && ex is not (IOException or UnauthorizedAccessException)) cameraFault = true;
            if (error != lastLoggedError)
            {
                log(error);
                lastLoggedError = error;
            }
            try { Publish(now, false); } catch (Exception) { }
        }
    }

    void Bind()
    {
        Assembly assembly;
        try { assembly = Assembly.Load("Il2CppMFGSimClient"); }
        catch (FileNotFoundException) { assembly = Assembly.Load("MFGSimClient"); }
        var type = assembly.GetType("Il2CppMFGClient.Camera.MFGSimCameraController")
            ?? assembly.GetType("MFGClient.Camera.MFGSimCameraController")
            ?? throw new MissingMemberException("Camera type not supported by this game version");
        PropertyInfo Float(string name, bool write)
        {
            var property = type.GetProperty(name, InstanceFlags);
            if (property?.PropertyType != typeof(float) || !property.CanRead
                || (write && !property.CanWrite) || property.GetIndexParameters().Length != 0)
                throw new MissingMemberException($"Unsupported camera member: {name}");
            return property;
        }
        fields = names.Select(name => Float(name, true)).ToArray();
        distance = Float("currentZoomDistance", false);
        minimum = Float("minZoomDistance", false);
        zoomPercent = Float("currentZoomPercent", true);
        pointer = type.GetProperty("Pointer", InstanceFlags);
        if (pointer?.PropertyType != typeof(nint) || !pointer.CanRead)
            throw new MissingMemberException("Missing IL2CPP object identity");
        var unityObject = Assembly.Load("UnityEngine.CoreModule").GetType("UnityEngine.Object")
            ?? throw new MissingMemberException("Missing Unity object wrapper");
        alive = unityObject.GetMethod("op_Implicit", BindingFlags.Static | BindingFlags.Public, null, new[] { unityObject }, null)
            ?? throw new MissingMemberException("Missing Unity object lifetime check");
        var getter = type.GetProperty("Instance", BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic);
        if (getter?.PropertyType != type || !getter.CanRead || getter.GetIndexParameters().Length != 0)
            throw new MissingMemberException("Missing camera singleton");
        var crewType = assembly.GetType("Il2CppMagicFuel.CrewWorldIcons.CrewWorldIconsCameraZoomEnable")
            ?? assembly.GetType("MagicFuel.CrewWorldIcons.CrewWorldIconsCameraZoomEnable")
            ?? throw new MissingMemberException("Missing crew camera visibility component");
        crewController = crewType.GetProperty("controller", InstanceFlags);
        if (crewController?.PropertyType != type || !crewController.CanRead)
            throw new MissingMemberException("Missing crew camera reference");
        var crewUpdate = crewType.GetMethod("Update", InstanceFlags, null, Type.EmptyTypes, null)
            ?? throw new MissingMethodException("Missing crew visibility update");
        HarmonyMethod Hook(string name) => new(typeof(Engine).GetMethod(name, BindingFlags.Static | BindingFlags.NonPublic)!);
        harmony.Patch(crewUpdate, prefix: Hook(nameof(BeforeCrewZoom)), finalizer: Hook(nameof(AfterCrewZoom)));
        instance = getter;
        compatible = true;
    }

    static void BeforeCrewZoom(object __instance, out (PropertyInfo Field, object Camera, float Percent)? __state)
    {
        __state = null;
        var engine = active;
        if (engine == null || engine.stopped || engine.cameraFault || engine.original.Length == 0) return;
        try
        {
            var controller = engine.crewController!.GetValue(__instance);
            if (controller == null || (nint)engine.pointer!.GetValue(controller)! != engine.cameraPointer) return;
            float distance = (float)engine.distance!.GetValue(controller)!;
            float percent = (float)engine.zoomPercent!.GetValue(controller)!;
            if (!float.IsFinite(distance) || !float.IsFinite(percent)) return;
            __state = (engine.zoomPercent, controller, percent);
            // Crew visibility uses the original range; other camera consumers retain their own percentage.
            engine.zoomPercent.SetValue(controller, Math.Clamp((distance - engine.originalMinimum)
                / (engine.original[0] - engine.originalMinimum), 0, 1));
        }
        catch (Exception ex) { engine.cameraFault = true; engine.error = ex.GetBaseException().Message; }
    }

    static void AfterCrewZoom((PropertyInfo Field, object Camera, float Percent)? __state)
    {
        if (__state is not { } state) return;
        try { state.Field.SetValue(state.Camera, state.Percent); }
        catch (Exception ex)
        {
            if (active == null) return;
            active.cameraFault = true;
            active.error = ex.GetBaseException().Message;
        }
    }

    public readonly record struct Settings(float Zoom, float Speed, bool ResourceHighlight = false, bool RepeatBuild = false);

    public static Settings? ParseSettings(JsonElement root, long now, bool temporary)
    {
        if (root.GetProperty("schema").GetInt32() != 1)
            throw new FormatException("Unsupported settings schema");
        float zoom = root.GetProperty("zoom_limit").GetSingle();
        float speed = root.GetProperty("movement_speed").GetSingle();
        if (!float.IsFinite(zoom) || zoom < 300 || zoom > 1000
            || !float.IsFinite(speed) || speed < .25f || speed > 3)
            throw new FormatException("Camera setting outside supported range");
        if (temporary)
        {
            long expires = root.GetProperty("expires_utc_ms").GetInt64();
            string? owner = root.GetProperty("owner").GetString();
            if (string.IsNullOrWhiteSpace(owner) || owner.Length > 128 || expires <= now || expires - now > 5000)
                return null;
        }
        bool Flag(string key) => root.TryGetProperty(key, out var value) && value.GetBoolean();
        return new(zoom, speed, Flag("resource_highlight"), Flag("repeat_build"));
    }

    static Settings? SelectSettings(string live, string permanent, long now)
        => ReadSettings(live, now, true) ?? ReadSettings(permanent, now, false);

    static Settings? ReadSettings(string path, long now, bool temporary)
    {
        try
        {
            using var input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            if (input.Length > 4096) throw new FormatException("Settings file exceeds limit");
            using var document = JsonDocument.Parse(input);
            return ParseSettings(document.RootElement, now, temporary);
        }
        catch (FileNotFoundException) { return null; }
        catch (DirectoryNotFoundException) { return null; }
        catch (Exception e) when (temporary && e is IOException or UnauthorizedAccessException or JsonException or FormatException or KeyNotFoundException or InvalidOperationException or OverflowException)
        {
            return null;
        }
        catch (Exception e) when (e is JsonException or KeyNotFoundException or InvalidOperationException or OverflowException)
        {
            throw new FormatException("Invalid permanent camera settings", e);
        }
    }

    void Publish(long now, bool ready)
    {
        float current = ready ? (float)distance!.GetValue(camera)! : 0;
        if (!float.IsFinite(current)) throw new InvalidOperationException("Invalid camera distance");
        var status = new
        {
            schema = 1, pid, game_dir = gameRoot, bridge_version = 2, updated_utc_ms = now,
            compatible, camera_ready = ready && !cameraFault,
            maximum = ready ? applied[0] : 0, current,
            minimum = ready ? originalMinimum : 0,
            original = ready ? original[0] : 0,
            speed = ready ? applied[1] / original[1] : 1,
            error,
            feature_error = resourceHighlight?.Error ?? RepeatBuild.Error
        };
        string temp = StatusPath + ".tmp";
        File.WriteAllText(temp, JsonSerializer.Serialize(status));
        File.Move(temp, StatusPath, true);
    }

    bool Restore()
    {
        if (camera == null || applied.Length != fields.Length || original.Length != fields.Length
            || !(bool)alive!.Invoke(null, new[] { camera })!) return true;
        for (int i = 0; i < fields.Length; i++)
            if ((float)fields[i].GetValue(camera)! != applied[i]) return false;
        for (int i = 0; i < fields.Length; i++) fields[i].SetValue(camera, original[i]);
        applied = (float[])original.Clone();
        return true;
    }

    public void Shutdown()
    {
        // Unity may already have destroyed camera objects during shutdown.
        stopped = true;
        resourceHighlight?.Abandon();
        RepeatBuild.Stop();
        if (ReferenceEquals(AppDomain.CurrentDomain.GetData(ActiveEngineKey), this))
            try { File.Delete(StatusPath); } catch (IOException) { }
    }
}

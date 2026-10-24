using System.Reflection;
using HarmonyLib;

namespace SubspaceToolkit;

internal static class RepeatBuild
{
    const BindingFlags Members = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.Static;
    static HarmonyLib.Harmony? harmony;
    static bool enabled;
    static PropertyInfo pointer = null!, moving = null!, blocked = null!, keyboard = null!, shift = null!, pressed = null!, focused = null!;
    [ThreadStatic] static nint repeating;
    public static string? Error { get; private set; }

    public static void Configure(bool value)
    {
        enabled = false;
        if (!value) { Error = null; return; }
        if (Error != null) return;
        if (harmony == null)
        {
            try
            {
                var game = Assembly.Load("Il2CppMFGSimClient").GetType("Il2CppMFGClient.BuildingPlacer", true)!;
                var keys = Assembly.Load("Unity.InputSystem").GetType("UnityEngine.InputSystem.Keyboard", true)!;
                var app = Assembly.Load("UnityEngine.CoreModule").GetType("UnityEngine.Application", true)!;
                PropertyInfo Property(Type type, string name, Type? expected = null)
                {
                    var p = type.GetProperty(name, Members);
                    if (p == null || !p.CanRead || (expected != null && p.PropertyType != expected))
                        throw new MissingMemberException(type.FullName, name);
                    return p;
                }
                pointer = Property(game, "Pointer", typeof(nint));
                moving = Property(game, "agentInMotion");
                blocked = Property(game, "blockCancel", typeof(bool));
                keyboard = Property(keys, "current", keys);
                shift = Property(keys, "shiftKey");
                pressed = Property(shift.PropertyType, "isPressed", typeof(bool));
                focused = Property(app, "isFocused", typeof(bool));
                var place = game.GetMethod("TryPlace", Members, null, Type.EmptyTypes, null)
                    ?? throw new MissingMethodException(game.FullName, "TryPlace");
                var cleanup = game.GetMethod("CleanupPlacement", Members, null, new[] { typeof(bool) }, null)
                    ?? throw new MissingMethodException(game.FullName, "CleanupPlacement");
                if (!cleanup.ReturnType.IsValueType || cleanup.ReturnType.FullName != "Il2CppCysharp.Threading.Tasks.UniTaskVoid")
                    throw new MissingMethodException("Unexpected placement cleanup contract");
                harmony = new HarmonyLib.Harmony("DcBexter.SubspaceToolkit.RepeatBuild");
                HarmonyMethod Hook(string name) => new(typeof(RepeatBuild).GetMethod(name, Members)!);
                harmony.Patch(place, prefix: Hook(nameof(BeforePlace)), finalizer: Hook(nameof(AfterPlace)));
                harmony.Patch(cleanup, prefix: Hook(nameof(BeforeCleanup)));
            }
            catch (Exception ex)
            {
                try { harmony?.UnpatchSelf(); } catch (Exception) { /* Disabled callbacks cannot change placement. */ }
                harmony = null;
                Error = ex.GetBaseException().Message;
                return;
            }
        }
        enabled = true;
    }

    static void BeforePlace(object __instance, out nint __state)
    {
        __state = repeating;
        repeating = 0;
        if (!enabled) return;
        try
        {
            var keys = keyboard.GetValue(null);
            var key = keys == null ? null : shift.GetValue(keys);
            if (key != null && (bool)pressed.GetValue(key)! && (bool)focused.GetValue(null)!
                && moving.GetValue(__instance) == null && !(bool)blocked.GetValue(__instance)!)
                repeating = (nint)pointer.GetValue(__instance)!;
        }
        catch (Exception ex)
        {
            enabled = false;
            Error = ex.GetBaseException().Message;
        }
    }

    static bool BeforeCleanup(object __instance)
    {
        if (!enabled || repeating == 0) return true;
        try
        {
            // ExitOnPlacement shares native code with unrelated constant-return methods.
            // Only suppress cleanup inside this exact successful placement call instead.
            return (nint)pointer.GetValue(__instance)! != repeating;
        }
        catch (Exception ex)
        {
            enabled = false;
            Error = ex.GetBaseException().Message;
            return true;
        }
    }

    static void AfterPlace(nint __state) => repeating = __state;
    public static void Stop() => enabled = false;
}

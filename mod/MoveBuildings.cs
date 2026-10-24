using System.Collections;
using System.Reflection;
using HarmonyLib;

namespace SubspaceToolkit;

internal sealed class MoveBuildings
{
    const BindingFlags Flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.Static;
    static MoveBuildings? current;
    readonly HarmonyLib.Harmony harmony;
    PropertyInfo agentType = null!, definition = null!, movable = null!;
    MethodInfo update = null!, getAgent = null!, findPanels = null!, alive = null!;
    object building = null!;
    bool enabled, patched, failed;
    public string? Error { get; private set; }

    public MoveBuildings(HarmonyLib.Harmony harmony) { this.harmony = harmony; current = this; }

    public void Tick(bool value)
    {
        if ((value == enabled && !failed) || (value && failed)) return;
        try
        {
            if (value && !patched) Bind();
            enabled = value;
            RefreshPanels();
            if (!value) { failed = false; Error = null; }
        }
        catch (Exception ex) { Fail(ex); }
    }

    void Bind()
    {
        var sim = Assembly.Load("Il2CppMFGSim");
        var agent = sim.GetType("Il2CppMFGSim.Agent", true)!;
        var panels = Assembly.Load("Il2CppMFGSimClient").GetType("Il2CppMFGClient.AgentInfoButtons", true)!;
        var unity = Assembly.Load("UnityEngine.CoreModule").GetType("UnityEngine.Object", true)!;
        PropertyInfo Property(Type type, string name) => type.GetProperty(name, Flags)
            ?? throw new MissingMemberException(type.FullName, name);
        agentType = Property(agent, "AgentType");
        building = Enum.Parse(agentType.PropertyType, "Building");
        definition = Property(agent, "RulesDefinition");
        movable = Property(definition.PropertyType, "IsBuildingMovable");
        if (movable.PropertyType != typeof(bool) || !movable.CanRead || !movable.CanWrite)
            throw new MissingMemberException("Unexpected building movement contract");
        update = panels.GetMethod("UpdateButtonStates", Flags, null, new[] { agent }, null)
            ?? throw new MissingMethodException(panels.FullName, "UpdateButtonStates");
        getAgent = panels.GetMethod("GetAgent", Flags, null, Type.EmptyTypes, null)!;
        alive = unity.GetMethod("op_Implicit", Flags, null, new[] { unity }, null)!;
        findPanels = unity.GetMethods(Flags).Single(m => m.Name == "FindObjectsOfType" && m.IsGenericMethodDefinition
            && m.GetParameters().Length == 1 && m.GetParameters()[0].ParameterType == typeof(bool)).MakeGenericMethod(panels);
        HarmonyMethod Hook(string name) => new(typeof(MoveBuildings).GetMethod(name, Flags)!);
        harmony.Patch(update, prefix: Hook(nameof(BeforeButtons)), finalizer: Hook(nameof(AfterButtons)));
        patched = true;
    }

    static void BeforeButtons(object __0, out object? __state)
    {
        __state = null;
        var self = current;
        if (self == null || !self.enabled || __0 == null) return;
        try
        {
            if (!Equals(self.agentType.GetValue(__0), self.building)) return;
            var rules = self.definition.GetValue(__0);
            if (rules == null || (bool)self.movable.GetValue(rules)!) return;
            // IL2CPP inlines this flag in the button check; keep its construction guards intact.
            __state = rules;
            self.movable.SetValue(rules, true);
        }
        catch (Exception ex) { self.Fail(ex); }
    }

    static void AfterButtons(object? __state)
    {
        if (__state == null) return;
        try { current!.movable.SetValue(__state, false); }
        catch (Exception ex) { current?.Fail(ex); }
    }

    void RefreshPanels()
    {
        if (!patched) return;
        foreach (var panel in (IEnumerable)findPanels.Invoke(null, new object[] { true })!)
        {
            if (panel == null || alive.Invoke(null, new[] { panel }) is not true) continue;
            var agent = getAgent.Invoke(panel, null);
            if (agent != null) update.Invoke(panel, new[] { agent });
        }
    }

    void Fail(Exception ex) { enabled = false; failed = true; Error = ex.GetBaseException().Message; }
    public void Stop() => Tick(false);
    public void Abandon() { enabled = false; if (current == this) current = null; }
}

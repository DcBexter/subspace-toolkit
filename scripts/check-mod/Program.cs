#nullable enable annotations
using System.Reflection;
using System.Runtime.Loader;
using System.Text.Json;
using System.Runtime.CompilerServices;

string root = Path.GetFullPath(".");
string loader = Path.Combine(root, "dist/SubspaceToolkit/payload/MelonLoader/net6");
AssemblyLoadContext.Default.Resolving += (context, name) => {
    string file = Path.Combine(loader, name.Name + ".dll");
    return File.Exists(file) ? context.LoadFromAssemblyPath(file) : null;
};
var assembly = AssemblyLoadContext.Default.LoadFromAssemblyPath(Path.Combine(root, "mod/bin/Release/net6.0/SubspaceToolkit.dll"));
var parse = assembly.GetType("SubspaceToolkit.Engine")!.GetMethod("ParseSettings")!;
object Read(string json, bool temporary = false) {
    using var document = JsonDocument.Parse(json);
    return parse.Invoke(null, new object[] { document.RootElement, 100000L, temporary });
}
int checks = 0;
void Assert(bool value, string name) { if (!value) throw new Exception(name); checks++; }
string Settings(int schema, double zoom, double speed, long expires = 103000, string owner = "owner") => JsonSerializer.Serialize(new {
    schema, zoom_limit = zoom, movement_speed = speed, expires_utc_ms = expires, owner
});
foreach (double zoom in new[] { 300.0, 1000 })
foreach (double speed in new[] { .25, 1, 3 }) {
    Assert(Read(Settings(1, zoom, speed)) != null, "valid permanent");
    Assert(Read(Settings(1, zoom, speed), true) != null, "valid lease");
}
Assert(Read(Settings(1, 1000, 3, 100000), true) == null, "expired lease");
Assert(Read(Settings(1, 1000, 3, 105001), true) == null, "unbounded lease");
Assert(Read(Settings(1, 1000, 3, 103000, ""), true) == null, "missing owner");
foreach (string json in new[] { Settings(2, 1000, 3), Settings(1, 1001, 3), Settings(1, 299, 1), Settings(1, 1000, .24), Settings(1, 1000, 3.01), "{}" }) {
    bool rejected = false;
    try { Read(json); } catch (TargetInvocationException) { rejected = true; }
    Assert(rejected, "invalid settings accepted");
}
var legacy = Read(Settings(1, 700, 1));
bool Flag(object value, string name) => (bool)value.GetType().GetProperty(name)!.GetValue(value)!;
Assert(!Flag(legacy, "ResourceHighlight") && !Flag(legacy, "RepeatBuild") && !Flag(legacy, "MoveBuildings"), "legacy defaults");
var enabled = Read("""{"schema":1,"zoom_limit":700,"movement_speed":1,"resource_highlight":true,"repeat_build":true,"move_buildings":true}""");
Assert(Flag(enabled, "ResourceHighlight") && Flag(enabled, "RepeatBuild") && Flag(enabled, "MoveBuildings"), "feature switches");
var readFile = assembly.GetType("SubspaceToolkit.Engine")!.GetMethod("ReadSettings", BindingFlags.NonPublic | BindingFlags.Static)!;
string temp = Path.Combine(Path.GetTempPath(), "subspace-config-check-" + Guid.NewGuid() + ".json");
try {
    foreach (string bad in new[] {
        """{"schema":1,"zoom_limit":700,"movement_speed":1,"repeat_build":"true"}""",
        """{"schema":1,"zoom_limit":700,"movement_speed":1,"resource_highlight":null}""",
        """{"schema":1,"zoom_limit":700,"movement_speed":1,"move_buildings":"true"}""",
        """{"schema":9999999999999999999999999,"zoom_limit":700,"movement_speed":1}"""
    }) {
        File.WriteAllText(temp, bad);
        Assert(readFile.Invoke(null, new object[] { temp, 100000L, true }) == null, "invalid temporary file ignored");
        bool rejected = false;
        try { readFile.Invoke(null, new object[] { temp, 100000L, false }); }
        catch (TargetInvocationException e) { rejected = e.InnerException is FormatException; }
        Assert(rejected, "invalid permanent settings rejected");
    }
    File.WriteAllText(temp, Settings(1, 700, 1));
    using (var locked = new FileStream(temp, FileMode.Open, FileAccess.Read, FileShare.None))
        Assert(readFile.Invoke(null, new object[] { temp, 100000L, true }) == null, "locked temporary file ignored");
    Assert(readFile.Invoke(null, new object[] { temp, 100000L, true }) != null, "temporary file recovers after unlock");
} finally { File.Delete(temp); }
Console.WriteLine($"{checks} config, feature, lease and lock recovery checks passed");
var select = assembly.GetType("SubspaceToolkit.Engine")!.GetMethod("SelectSettings", BindingFlags.NonPublic | BindingFlags.Static)!;
string permanent = temp + ".permanent";
try {
    File.WriteAllText(permanent, """{"schema":1,"zoom_limit":1000,"movement_speed":3,"resource_highlight":true,"repeat_build":false}""");
    File.WriteAllText(temp, """{"schema":1,"zoom_limit":500,"movement_speed":0.5,"resource_highlight":false,"repeat_build":true,"move_buildings":true,"expires_utc_ms":103000,"owner":"trainer"}""");
    var before = File.ReadAllBytes(permanent);
    object Selected(long now) => select.Invoke(null, new object[] { temp, permanent, now });
    var live = Selected(100000);
    Assert(!Flag(live, "ResourceHighlight") && Flag(live, "RepeatBuild") && Flag(live, "MoveBuildings"), "trainer overrides every permanent flag");
    Assert((float)live.GetType().GetProperty("Speed")!.GetValue(live)! == .5f, "trainer camera independent");
    var expired = Selected(104000);
    Assert(Flag(expired, "ResourceHighlight") && !Flag(expired, "RepeatBuild") && !Flag(expired, "MoveBuildings"), "lease expiry restores permanent flags");
    Assert(File.ReadAllBytes(permanent).SequenceEqual(before), "reading trainer never changes permanent file");
    File.WriteAllText(permanent, "broken permanent file");
    Assert(Selected(100000) != null, "valid trainer independent of invalid permanent config");
    File.Delete(permanent);
    Assert(Selected(100000) != null, "trainer works without permanent config");
    File.Delete(temp);
    Assert(Selected(100000) == null, "no trainer and no permanent config means defaults");
} finally { File.Delete(temp); File.Delete(permanent); }
Console.WriteLine($"{checks} total checks including all-feature trainer/permanent separation passed");

var highlightType = assembly.GetType("SubspaceToolkit.ResourceHighlight")!;
var highlight = RuntimeHelpers.GetUninitializedObject(highlightType);
const BindingFlags privateInstance = BindingFlags.Instance | BindingFlags.NonPublic;
void Field(string name, object value) => highlightType.GetField(name, privateInstance)!.SetValue(highlight, value);
Field("properties", new Dictionary<(Type, string), PropertyInfo>());
Field("alive", typeof(InventoryNode).GetMethod(nameof(InventoryNode.Alive))!);
Field("parent", typeof(InventoryNode).GetMethod(nameof(InventoryNode.GetComponentInParent))!);
Field("toggleType", typeof(InventoryNode));
var legacyToggle = new InventoryNode { enabled = false };
var actualToggle = new InventoryNode();
var inventory = new InventoryNode { toggle = legacyToggle, parent = new InventoryNode { Component = actualToggle } };
var resolveToggle = highlightType.GetMethod("InventoryToggle", privateInstance)
    ?? throw new Exception("Inventory selection still uses disabled legacy toggle");
object? ResolveInventory() => resolveToggle.Invoke(highlight, new object[] { inventory });
Assert(ReferenceEquals(ResolveInventory(), actualToggle), "uses active parent toggle, never broken legacy toggle");
actualToggle.enabled = false;
Assert(ResolveInventory() == null, "disabled inventory control excluded");
actualToggle.enabled = true;
actualToggle.activeInHierarchy = false;
Assert(ResolveInventory() == null, "hidden popup inventory excluded");
actualToggle.activeInHierarchy = true;
inventory.toggle = null;
Assert(ResolveInventory() == null, "category lists cannot masquerade as inventory root");
Console.WriteLine($"{checks} total checks including inventory toggle regression passed");

Field("outlines", new List<(object Target, object Effect)>());
Field("entered", new HashSet<nint>());
Field("getComponent", typeof(InventoryNode).GetMethod(nameof(InventoryNode.GetComponent))!);
Field("viewType", typeof(InventoryNode));
Field("setVisible", typeof(InventoryNode).GetMethod(nameof(InventoryNode.SetVisible))!);
inventory.toggle = legacyToggle;
inventory.Component = new InventoryNode { CurrentState = 2 };
actualToggle.isOn = false;
highlightType.GetMethod("OpenInventory", privateInstance)!.Invoke(highlight, new object[] { inventory });
Assert(!(bool)highlightType.GetField("opened", privateInstance)!.GetValue(highlight)!, "visible inventory with stale off-toggle must not become mod-owned");
highlightType.GetMethod("Clear", privateInstance)!.Invoke(highlight, null);
Assert(inventory.Component.CurrentState == 2 && !actualToggle.isOn, "hover leaves already visible panel and toggle unchanged");
inventory.Component.CurrentState = 0;
inventory.Component.alpha = 1;
highlightType.GetMethod("OpenInventory", privateInstance)!.Invoke(highlight, new object[] { inventory });
Assert(!(bool)highlightType.GetField("opened", privateInstance)!.GetValue(highlight)!, "visible prefab with stale hidden UIView state must remain user-owned");
highlightType.GetMethod("Clear", privateInstance)!.Invoke(highlight, null);
Assert(inventory.Component.alpha == 1, "prefab-visible panel remains visible after hover");
inventory.Component.alpha = 0;
foreach (int state in new[] { 0, 1, 2, 3, 4 }) {
    inventory.Component.CurrentState = state;
    actualToggle.isOn = true;
    highlightType.GetMethod("OpenInventory", privateInstance)!.Invoke(highlight, new object[] { inventory });
    bool owned = state is 0 or 4;
    Assert(inventory.Component.CurrentState == (owned ? 2 : state), "hidden or closing panel opens; visible or opening panel remains unchanged");
    highlightType.GetMethod("Clear", privateInstance)!.Invoke(highlight, null);
    Assert(inventory.Component.CurrentState == (owned ? 0 : state) && actualToggle.isOn, "only mod-opened panel closes; button state preserved");
}
Field("imageType", typeof(InventoryNode));
Field("find", typeof(InventoryNode).GetMethod(nameof(InventoryNode.Find))!);
Field("instantiate", typeof(InventoryNode).GetMethod(nameof(InventoryNode.Instantiate))!);
Field("destroy", typeof(InventoryNode).GetMethod(nameof(InventoryNode.Destroy))!);
var tile = new InventoryNode { Frame = new InventoryNode { Component = new InventoryNode { enabled = false } } };
var border = (InventoryNode)highlightType.GetMethod("CreateBorder", privateInstance)!.Invoke(highlight, new object[] { tile })!;
Assert(ReferenceEquals(border.Source, tile.Frame.Component) && ReferenceEquals(border.parent, tile), "frame cloned inside tile instead of resource icon");
Assert(!tile.Frame.Component.enabled, "original frame remains untouched");
var effects = (List<(object Target, object Effect)>)highlightType.GetField("outlines", privateInstance)!.GetValue(highlight)!;
effects.Add((tile, border));
highlightType.GetMethod("Clear", privateInstance)!.Invoke(highlight, null);
Assert(border.Destroyed && !tile.Frame.Component.Destroyed && effects.Count == 0, "cleanup destroys only owned border clone");
Console.WriteLine($"{checks} total checks including panel visibility ownership and tile border cleanup passed");

var engineType = assembly.GetType("SubspaceToolkit.Engine")!;
var engine = RuntimeHelpers.GetUninitializedObject(engineType);
var beforeCrew = engineType.GetMethod("BeforeCrewZoom", BindingFlags.NonPublic | BindingFlags.Static)
    ?? throw new Exception("Crew portraits still use the expanded zoom range");
var afterCrew = engineType.GetMethod("AfterCrewZoom", BindingFlags.NonPublic | BindingFlags.Static)!;
void CameraField(string name, object value) => engineType.GetField(name, privateInstance)!.SetValue(engine, value);
var camera = new CrewCamera();
var crew = new CrewVisibility { controller = camera };
CameraField("cameraPointer", (nint)1);
CameraField("pointer", typeof(CrewCamera).GetProperty(nameof(CrewCamera.Pointer))!);
CameraField("distance", typeof(CrewCamera).GetProperty(nameof(CrewCamera.currentZoomDistance))!);
CameraField("zoomPercent", typeof(CrewCamera).GetProperty(nameof(CrewCamera.currentZoomPercent))!);
CameraField("crewController", typeof(CrewVisibility).GetProperty(nameof(CrewVisibility.controller))!);
CameraField("original", new float[] { 300, 40, 120 });
CameraField("originalMinimum", 35f);
engineType.GetField("active", BindingFlags.NonPublic | BindingFlags.Static)!.SetValue(null, engine);
foreach (float limit in new[] { 300f, 600f, 1000f })
foreach (float distance in new[] { 259f, 261f, 280f, 900f }) {
    camera.currentZoomDistance = distance;
    float before = camera.currentZoomPercent = Math.Clamp((distance - 35) / (limit - 35), 0, 1);
    object?[] hookArgs = { crew, null };
    beforeCrew.Invoke(null, hookArgs);
    Assert((camera.currentZoomPercent >= .85f) == (distance >= 260.25f), "portrait visibility preserves original physical distance");
    afterCrew.Invoke(null, new[] { hookArgs[1] });
    Assert(camera.currentZoomPercent == before, "crew callback restores camera percentage for other consumers");
}
CameraField("stopped", true);
camera.currentZoomPercent = .2f;
object?[] stoppedArgs = { crew, null };
beforeCrew.Invoke(null, stoppedArgs);
Assert(camera.currentZoomPercent == .2f && stoppedArgs[1] == null, "suspended engine cannot affect crew visibility");
CameraField("stopped", false);
crew.controller = new CrewCamera { Pointer = 2, currentZoomPercent = .3f };
beforeCrew.Invoke(null, stoppedArgs);
Assert(crew.controller.currentZoomPercent == .3f && stoppedArgs[1] == null, "unowned camera remains untouched");
Console.WriteLine($"{checks} total checks including crew visibility and camera isolation passed");

var moveType = assembly.GetType("SubspaceToolkit.MoveBuildings")!;
var move = RuntimeHelpers.GetUninitializedObject(moveType);
void MoveField(string name, object value) => moveType.GetField(name, privateInstance)!.SetValue(move, value);
MoveField("agentType", typeof(MovingAgent).GetProperty(nameof(MovingAgent.AgentType))!);
MoveField("definition", typeof(MovingAgent).GetProperty(nameof(MovingAgent.RulesDefinition))!);
MoveField("movable", typeof(MovingRules).GetProperty(nameof(MovingRules.IsBuildingMovable))!);
MoveField("building", MovingKind.Building);
MoveField("enabled", true);
moveType.GetField("current", BindingFlags.NonPublic | BindingFlags.Static)!.SetValue(null, move);
var beforeButtons = moveType.GetMethod("BeforeButtons", BindingFlags.NonPublic | BindingFlags.Static)!;
var afterButtons = moveType.GetMethod("AfterButtons", BindingFlags.NonPublic | BindingFlags.Static)!;
var movingAgent = new MovingAgent();
foreach (int flags in new[] { 0, 256, 2048, 256 | 2048 }) {
    movingAgent.Flags = flags;
    object?[] movementArgs = { movingAgent, null };
    beforeButtons.Invoke(null, movementArgs);
    try {
        Assert(movingAgent.RulesDefinition!.IsBuildingMovable, "unlocks building only during button calculation");
        Assert(movingAgent.NativeButtonVisible() == (flags == 0), "construction and demolition guards stay authoritative");
        object?[] nested = { movingAgent, null };
        beforeButtons.Invoke(null, nested);
        afterButtons.Invoke(null, new[] { nested[1] });
        Assert(movingAgent.RulesDefinition.IsBuildingMovable, "nested callback cannot restore outer scope early");
    } finally { afterButtons.Invoke(null, new[] { movementArgs[1] }); }
    Assert(!movingAgent.RulesDefinition!.IsBuildingMovable, "building definition restored after callback");
}
movingAgent.RulesDefinition!.IsBuildingMovable = true;
object?[] originalMovable = { movingAgent, null };
beforeButtons.Invoke(null, originalMovable);
afterButtons.Invoke(null, new[] { originalMovable[1] });
Assert(movingAgent.RulesDefinition.IsBuildingMovable && originalMovable[1] == null, "already movable building untouched");
movingAgent.RulesDefinition.IsBuildingMovable = false;
foreach (MovingKind kind in new[] { MovingKind.Worker, MovingKind.Resource }) {
    movingAgent.AgentType = kind;
    beforeButtons.Invoke(null, originalMovable);
    Assert(!movingAgent.RulesDefinition.IsBuildingMovable && originalMovable[1] == null, "nonbuilding never unlocked");
}
movingAgent.AgentType = MovingKind.Building;
MoveField("enabled", false);
beforeButtons.Invoke(null, originalMovable);
Assert(!movingAgent.RulesDefinition.IsBuildingMovable, "disabled mod leaves original button policy");
MoveField("enabled", true);
beforeButtons.Invoke(null, originalMovable);
MoveField("enabled", false);
afterButtons.Invoke(null, new[] { originalMovable[1] });
Assert(!movingAgent.RulesDefinition.IsBuildingMovable, "finalizer restores even if mod disabled during callback");
MoveField("enabled", true);
movingAgent.RulesDefinition = null;
beforeButtons.Invoke(null, originalMovable);
Assert(originalMovable[1] == null, "missing definition left untouched");
Console.WriteLine($"{checks} total checks including scoped building movement passed");

// The native early occupancy check blocks the moving agent before footprint exclusions run.
MoveField("moving", typeof(MovingPlacer).GetProperty(nameof(MovingPlacer.agentInMotion))!);
MoveField("runtimeId", typeof(MovingAgent).GetProperty(nameof(MovingAgent.RuntimeId))!);
MoveField("simulation", typeof(MovingSim).GetProperty(nameof(MovingSim.Instance))!);
MoveField("networks", typeof(MovingSim).GetProperty(nameof(MovingSim.NetworkLayerManager))!);
MoveField("ground", typeof(MovingNetworks).GetProperty(nameof(MovingNetworks.GroundLayer))!);
MoveField("occupants", typeof(MovingLayer).GetProperty(nameof(MovingLayer.OccupantGrid))!);
MoveField("tryOccupant", typeof(MovingGrid).GetMethod(nameof(MovingGrid.TryGetValue))!);
var afterOccupied = moveType.GetMethod("AfterOccupied", BindingFlags.NonPublic | BindingFlags.Static)!;
var placer = new MovingPlacer { agentInMotion = new MovingAgent { RuntimeId = 42 } };
var grid = MovingSim.Instance.NetworkLayerManager.GroundLayer.OccupantGrid;
bool Occupied(int coordinate, bool original = true) {
    object?[] args = { placer, coordinate, original };
    afterOccupied.Invoke(null, args);
    return (bool)args[2]!;
}
grid.Cells[0] = 42;
grid.Cells[1] = 42;
grid.Cells[2] = 99;
Assert(!Occupied(0) && !Occupied(1), "moving building may overlap its old footprint");
Assert(Occupied(2), "neighbor building still blocks movement");
Assert(Occupied(3), "unknown or unreadable cell keeps original rejection");
Assert(!Occupied(2, false), "original allowed result remains allowed");
MoveField("enabled", false);
Assert(Occupied(0), "disabled mod retains native occupancy check");
MoveField("enabled", true);
placer.agentInMotion!.AgentType = MovingKind.Worker;
Assert(Occupied(0), "crew placement does not bypass occupancy");
placer.agentInMotion.AgentType = MovingKind.Building;
placer.agentInMotion.RuntimeId = 0;
grid.Cells[4] = 0;
Assert(Occupied(4), "unassigned agent ID never grants an exception");
placer.agentInMotion = null;
Assert(Occupied(0), "new construction cannot overlap existing building");
placer.agentInMotion = new MovingAgent { RuntimeId = 42 };
grid.Throw = true;
Assert(Occupied(0), "lookup failure leaves placement blocked");
grid.Throw = false;
Assert(Occupied(0), "failed feature remains disabled");
Console.WriteLine($"{checks} total checks including self-overlap occupancy passed");

public enum MovingKind { Worker, Building, Resource }
public class MovingRules { public bool IsBuildingMovable { get; set; } }
public class MovingAgent
{
    public uint RuntimeId { get; set; }
    public MovingKind AgentType { get; set; } = MovingKind.Building;
    public MovingRules? RulesDefinition { get; set; } = new();
    public int Flags { get; set; }
    // Predicate recovered from this build's inlined UpdateButtonStates, not a replacement in the mod.
    public bool NativeButtonVisible() => AgentType == MovingKind.Building && RulesDefinition!.IsBuildingMovable && (Flags & 0x900) == 0;
}

public class MovingPlacer { public MovingAgent? agentInMotion { get; set; } }
public class MovingSim {
    public static MovingSim Instance { get; } = new();
    public MovingNetworks NetworkLayerManager { get; } = new();
}
public class MovingNetworks { public MovingLayer GroundLayer { get; } = new(); }
public class MovingLayer { public MovingGrid OccupantGrid { get; } = new(); }
public class MovingGrid {
    public Dictionary<int, uint> Cells { get; } = new();
    public bool Throw { get; set; }
    public bool TryGetValue(int coordinate, out uint id) {
        if (Throw) throw new IOException("grid unavailable");
        return Cells.TryGetValue(coordinate, out id);
    }
}

public class CrewCamera
{
    public nint Pointer { get; set; } = 1;
    public float currentZoomDistance { get; set; }
    public float currentZoomPercent { get; set; }
}
public class CrewVisibility { public CrewCamera controller { get; set; } = new(); }

public class InventoryNode
{
    public bool enabled { get; set; } = true;
    public bool activeInHierarchy { get; set; } = true;
    public bool isOn { get; set; }
    public int CurrentState { get; set; }
    public float alpha { get; set; }
    public InventoryNode CanvasGroup => this;
    public InventoryNode gameObject => this;
    public InventoryNode transform => this;
    public InventoryNode? parent { get; set; }
    public InventoryNode? toggle { get; set; }
    public InventoryNode? Component { get; set; }
    public InventoryNode? Frame { get; set; }
    public InventoryNode? Source { get; set; }
    public bool Destroyed { get; set; }
    public T? GetComponent<T>() where T : class => Component as T;
    public InventoryNode? Find(string name) => name == "inner" ? Frame : null;
    public void SetVisible(bool visible) => CurrentState = visible ? 2 : 0;
    public static T Instantiate<T>(T original, InventoryNode parent, bool worldPositionStays) where T : InventoryNode
        => (T)new InventoryNode { Source = original, parent = parent };
    public static void Destroy(object value) => ((InventoryNode)value).Destroyed = true;
    public T? GetComponentInParent<T>() where T : class => Component as T;
    public static bool Alive(object? value) => value is InventoryNode;
}

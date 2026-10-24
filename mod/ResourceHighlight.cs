using System.Reflection;
using HarmonyLib;

namespace SubspaceToolkit;

// Game types stay behind reflection so builds never redistribute game assemblies.
internal sealed class ResourceHighlight
{
    const BindingFlags Flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.Static;
    static ResourceHighlight? current;
    readonly HarmonyLib.Harmony harmony;
    readonly Dictionary<(Type, string), PropertyInfo> properties = new();
    readonly HashSet<nint> entered = new();
    readonly List<(object Target, object Effect)> outlines = new();
    Type component = null!, unityObject = null!, iconType = null!, requestType = null!, resupplyType = null!, menuType = null!, listType = null!, imageType = null!, toggleType = null!, viewType = null!;
    MethodInfo alive = null!, destroy = null!, parent = null!, children = null!, getComponent = null!, find = null!, instantiate = null!, setVisible = null!;
    object? icon, owner, view;
    object[] lists = Array.Empty<object>();
    int stat;
    bool enabled, patched, failed, opened;
    public string? Error { get; private set; }

    public ResourceHighlight(HarmonyLib.Harmony harmony) { this.harmony = harmony; current = this; }

    public void Tick(bool enable)
    {
        if (!enable) { Stop(); failed = false; Error = null; return; }
        if (failed) return;
        enabled = true;
        try
        {
            if (!patched) Bind();
            if (icon == null) return;
            if (entered.Count == 0 || !Active(icon) || !Active(owner) || (int)Get(icon, "Stat") != stat)
            {
                Clear();
                return;
            }
            if (outlines.Any(item => !Active(item.Target) || (int)Get(item.Target, "Stat") != stat)) ClearOutlines();
            if (outlines.Count == 0) Highlight();
        }
        catch (Exception e) { Fail(e); }
    }

    public void Stop()
    {
        enabled = false;
        if (icon != null || outlines.Count != 0 || opened)
            try { Clear(); } catch (Exception e) { Error = e.GetBaseException().Message; }
    }

    // OnApplicationQuit must not touch Unity objects already being destroyed.
    public void Abandon() { enabled = false; if (current == this) current = null; }

    void Bind()
    {
        Type Load(string assembly, string name) => Assembly.Load(assembly).GetType(name)
            ?? throw new MissingMemberException(name);
        component = Load("UnityEngine.CoreModule", "UnityEngine.Component");
        unityObject = Load("UnityEngine.CoreModule", "UnityEngine.Object");
        var transform = Load("UnityEngine.CoreModule", "UnityEngine.Transform");
        iconType = Load("Il2CppMFGStatViews", "Il2CppMFGClient.StatIcon");
        requestType = Load("Il2CppMFGSimClient", "Il2CppMFGClient.RecipeOrder.AvailableOrderView");
        resupplyType = Load("Il2CppMFGSimClient", "Il2CppMFGClient.ResupplyButton");
        menuType = Load("Il2CppMFGSimClient", "Il2CppMFGClient.ResupplyMenu");
        listType = Load("Il2CppMFGSimClient", "Il2CppMFGClient.WorldAgentStatList");
        imageType = Load("UnityEngine.UI", "UnityEngine.UI.Image");
        toggleType = Load("UnityEngine.UI", "UnityEngine.UI.Toggle");
        viewType = Load("Il2CppMFGUI", "Il2CppMFGClient.UIView");
        var tooltip = Load("Il2CppMFGTooltips", "Il2CppMFGClient.TooltipTriggerBase");
        alive = unityObject.GetMethod("op_Implicit", Flags, null, new[] { unityObject }, null)!;
        destroy = unityObject.GetMethod("Destroy", Flags, null, new[] { unityObject }, null)!;
        parent = component.GetMethods().Single(m => m.Name == "GetComponentInParent" && m.IsGenericMethodDefinition && m.GetParameters().Length == 0);
        children = component.GetMethods().Single(m => m.Name == "GetComponentsInChildren" && m.IsGenericMethodDefinition && m.GetParameters().Length == 1 && m.GetParameters()[0].ParameterType == typeof(bool));
        getComponent = component.GetMethods().Single(m => m.Name == "GetComponent" && m.IsGenericMethodDefinition && m.GetParameters().Length == 0);
        find = transform.GetMethod("Find", new[] { typeof(string) })!;
        instantiate = unityObject.GetMethods().Single(m => m.Name == "Instantiate" && m.IsGenericMethodDefinition && m.GetParameters().Length == 3 && m.GetParameters()[1].ParameterType == transform && m.GetParameters()[2].ParameterType == typeof(bool));
        setVisible = viewType.GetMethod("SetVisible", new[] { typeof(bool) })!;
        if (alive == null || destroy == null) throw new MissingMemberException("Unity object lifetime methods");
        // Only postfix: preserve game hover delegates, tooltips, selection and quantities.
        var hooks = (from type in new[] { iconType, tooltip }
                     from pair in new[] { ("OnPointerEnter", nameof(Enter)), ("OnPointerExit", nameof(Exit)) }
                     select (Target: type.GetMethod(pair.Item1, Flags) ?? throw new MissingMemberException(type.FullName, pair.Item1),
                             Postfix: typeof(ResourceHighlight).GetMethod(pair.Item2, Flags)!)).ToArray();
        try
        {
            foreach (var hook in hooks) harmony.Patch(hook.Target, postfix: new HarmonyMethod(hook.Postfix));
        }
        catch
        {
            foreach (var hook in hooks) harmony.Unpatch(hook.Target, hook.Postfix);
            throw;
        }
        patched = true;
    }

    static void Enter(object __instance)
    {
        var self = current;
        if (self == null || !self.enabled || self.failed) return;
        try { self.Hover(__instance); } catch (Exception e) { self.Fail(e); }
    }
    static void Exit(object __instance)
    {
        var self = current;
        if (self == null || !self.enabled || self.failed) return;
        try { self.entered.Remove(self.Id(__instance)); } catch (Exception e) { self.Fail(e); }
    }

    void Hover(object source)
    {
        object? next = null, popup = null;
        if (iconType.IsInstanceOfType(source))
        {
            var request = Parent(source, requestType);
            if (Active(request) && Items(Get(Get(request!, "inputPool"), "Instances")).Any(item => Same(item, source)))
                (next, popup) = (source, request);
        }
        if (next == null)
        {
            var button = Parent(source, resupplyType);
            var menu = Alive(button) ? Parent(button!, menuType) : null;
            if (!Active(button) || !Active(menu) || (bool)Get(button!, "isCrew") || (int)Get(button!, "DataIndexResource") < 0) return;
            var container = Get(button!, "container");
            if (!Same(container, Get(menu!, "pickSelectionResource")) && !Same(container, Get(menu!, "pickedResupplyContainerResource"))) return;
            next = Get(button!, "statIcon");
            if (!Same(source, next) && !Same(source, Get(button!, "tooltipTrigger"))) return;
            popup = menu;
        }
        if (!Active(next)) return;
        if (Same(icon, next)) { entered.Add(Id(source)); return; }
        Clear();
        icon = next;
        owner = popup;
        stat = (int)Get(next!, "Stat");
        entered.Add(Id(source));
        // Nearest shared popup ancestor avoids highlighting another hidden inventory variant.
        for (object? ancestor = Get(popup!, "transform"); Alive(ancestor); ancestor = Get(ancestor!, "parent"))
        {
            var found = Items(children.MakeGenericMethod(listType).Invoke(ancestor, new object[] { true })!).ToArray();
            var root = found.FirstOrDefault(item => InventoryToggle(item) != null);
            if (root == null) continue;
            lists = Items(children.MakeGenericMethod(listType).Invoke(root, new object[] { true })!).ToArray();
            OpenInventory(root);
            Highlight();
            break;
        }
    }

    object? InventoryToggle(object list)
    {
        if (!Alive(Get(list, "toggle"))) return null;
        // ResourceContents retains a disabled legacy toggle with a broken OnToggle listener.
        var container = Get(Get(list, "transform"), "parent");
        var candidate = Alive(container) ? Parent(container, toggleType) : null;
        return Active(candidate) && (bool)Get(candidate!, "enabled") ? candidate : null;
    }

    void OpenInventory(object root)
    {
        view = getComponent.MakeGenericMethod(viewType).Invoke(root, null)!;
        // The game can show this UIView without synchronizing the button's toggle.
        var state = Convert.ToInt32(Get(view, "CurrentState"));
        var visible = Active(view) && Convert.ToSingle(Get(Get(view, "CanvasGroup"), "alpha")) > 0;
        opened = state == 4 || (state == 0 && !visible);
        if (opened) setVisible.Invoke(view, new object[] { true });
    }

    void Highlight()
    {
        var colorType = Assembly.Load("UnityEngine.CoreModule").GetType("UnityEngine.Color")!;
        foreach (var list in lists.Where(Alive))
        {
            var pool = Get(list, "iconPool");
            if (!Alive(pool)) continue;
            foreach (var target in Items(Get(pool, "Instances")))
            {
                if (!Active(target) || (int)Get(target, "Stat") != stat) continue;
                var outline = CreateBorder(target);
                outlines.Add((target, outline));
                Set(outline, "color", Activator.CreateInstance(colorType, 1f, .85f, .2f, 1f)!);
                Set(outline, "fillCenter", false);
                Set(outline, "raycastTarget", false);
                Set(outline, "enabled", true);
            }
        }
    }

    object CreateBorder(object target)
    {
        var transform = Get(target, "transform");
        var frame = find.Invoke(transform, new object[] { "inner" });
        if (!Alive(frame)) throw new MissingMemberException("Resource tile frame");
        var image = getComponent.MakeGenericMethod(imageType).Invoke(frame, null);
        if (!Alive(image)) throw new MissingMemberException("Resource tile frame image");
        // Clone the tile's border, leaving the game's icon, quantity and own hover effects intact.
        return instantiate.MakeGenericMethod(imageType).Invoke(null, new[] { image, transform, false })!;
    }

    void Clear()
    {
        ClearOutlines();
        if (opened && Alive(view)) setVisible.Invoke(view, new object[] { false });
        opened = false;
        view = icon = owner = null;
        lists = Array.Empty<object>();
        entered.Clear();
    }
    void ClearOutlines()
    {
        foreach (var outline in outlines.Select(item => item.Effect).Where(Alive))
        {
            Set(outline, "enabled", false);
            destroy.Invoke(null, new[] { Get(outline, "gameObject") });
        }
        outlines.Clear();
    }
    void Fail(Exception e)
    {
        failed = true;
        Error = e.GetBaseException().Message;
        Stop();
    }
    object? Parent(object value, Type type) => parent.MakeGenericMethod(type).Invoke(value, null);
    bool Alive(object? value) => value != null && (bool)alive.Invoke(null, new[] { value })!;
    bool Active(object? value) => Alive(value) && (bool)Get(Get(value!, "gameObject"), "activeInHierarchy");
    nint Id(object value) => (nint)Get(value, "Pointer");
    bool Same(object? a, object? b) => a != null && b != null && Id(a) == Id(b);
    PropertyInfo Property(object value, string name)
    {
        var key = (value.GetType(), name);
        if (!properties.TryGetValue(key, out var property))
            properties[key] = property = key.Item1.GetProperty(name, Flags) ?? throw new MissingMemberException(key.Item1.FullName, name);
        return property;
    }
    object Get(object value, string name) => Property(value, name).GetValue(value)!;
    void Set(object value, string name, object data) => Property(value, name).SetValue(value, data);
    IEnumerable<object> Items(object list)
    {
        var type = list.GetType();
        int count = Convert.ToInt32((type.GetProperty("Count") ?? type.GetProperty("Length") ?? throw new MissingMemberException(type.FullName, "Count/Length")).GetValue(list));
        var item = type.GetProperty("Item") ?? throw new MissingMemberException(type.FullName, "Item");
        for (int i = 0; i < count; i++) yield return item.GetValue(list, new object[] { i })!;
    }
}

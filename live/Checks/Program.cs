using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Runtime.Loader;

string live = Path.GetFullPath(args[0]);
AssemblyLoadContext.Default.Resolving += (context, name) => {
    string file = Path.Combine(live, name.Name + ".dll");
    return File.Exists(file) ? context.LoadFromAssemblyPath(file) : null;
};
var assembly = AssemblyLoadContext.Default.LoadFromAssemblyPath(Path.Combine(live, "SubspaceLive.dll"));
var bootstrap = assembly.GetType("SubspaceToolkit.Live.Bootstrap", true)!;
foreach (var delegateType in new[] { typeof(Callback), typeof(Delegate) })
{
    var wrapper = bootstrap.GetNestedType("Native`1", BindingFlags.NonPublic)!.MakeGenericType(delegateType);
    nint original = NativeLibrary.GetExport(NativeLibrary.Load(Path.GetFullPath(args[1])), "Add");
    var invoke = Marshal.GetDelegateForFunctionPointer<Callback>(original);
    if (invoke(5) != 6) throw new Exception("Native fixture failed");
    Callback replacement = value => value + 10;
    var detour = Activator.CreateInstance(wrapper, original, replacement)!;
    if (invoke(5) != 6) throw new Exception("Detour activated before original trampoline was published");
    wrapper.GetMethod("Apply")!.Invoke(detour, null);
    if (invoke(5) != 15) throw new Exception("Native callback detour failed");
    nint trampoline = (nint)wrapper.GetProperty("OriginalTrampoline")!.GetValue(detour)!;
    if (Marshal.GetDelegateForFunctionPointer<Callback>(trampoline)(5) != 6) throw new Exception("Original trampoline failed");
    wrapper.GetMethod("Apply")!.Invoke(detour, null);
    wrapper.GetMethod("Dispose")!.Invoke(detour, null);
    if (invoke(5) != 6) throw new Exception("Native callback restoration failed");
}
Console.WriteLine("Standalone detour applies, preserves trampoline, reapplies safely and restores in owned test process");

[UnmanagedFunctionPointer(CallingConvention.Cdecl)]
public delegate int Callback(int value);

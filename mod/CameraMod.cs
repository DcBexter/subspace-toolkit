using MelonLoader;
using MelonLoader.Utils;

[assembly: MelonInfo(typeof(SubspaceToolkit.CameraMod), "Subspace Toolkit", "0.1.0", "DcBexter")]
namespace SubspaceToolkit;

public sealed class CameraMod : MelonMod
{
    Engine? engine;
    public override void OnInitializeMelon()
    {
        if (Path.GetFileName(MelonEnvironment.GameExecutablePath).Equals("StarTrekOutpostsUnknown.exe", StringComparison.OrdinalIgnoreCase))
            engine = new Engine(HarmonyInstance, MelonEnvironment.GameRootDirectory, message => LoggerInstance.Msg(message));
    }
    public override void OnUpdate() => engine?.Tick();
    public override void OnApplicationQuit() => engine?.Shutdown();
}

using System.Diagnostics;
using System.IO.Compression;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Runtime.Loader;
using AsmResolver.DotNet;
using AsmResolver.DotNet.Serialized;
using Il2CppInterop.Generator;
using Il2CppInterop.Generator.Runners;

AssemblyLoadContext.Default.Resolving += (context, name) => {
    string path = Path.Combine(AppContext.BaseDirectory, name.Name + ".dll");
    return File.Exists(path) ? context.LoadFromAssemblyPath(path) : null;
};
if (args.Length is < 1 or > 2) { Console.Error.WriteLine("Usage: SubspaceInterop <game-folder> [cache-folder]"); return 2; }
try { await Prepare(Path.GetFullPath(args[0]), args.Length == 2 ? Path.GetFullPath(args[1]) : Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "SubspaceToolkit", "interop")); return 0; }
catch (Exception e) { Console.Error.WriteLine(e.ToString()); return 1; }

static string Hash(string path) { using var stream = File.OpenRead(path); using var hash = SHA256.Create(); return Convert.ToHexString(hash.ComputeHash(stream)).ToLowerInvariant(); }
static bool Within(string child, string parent) => child.Equals(parent, StringComparison.OrdinalIgnoreCase) || child.StartsWith(parent.TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
static void Save(string path, object value) { string temp = path + ".tmp"; File.WriteAllText(temp, JsonSerializer.Serialize(value)); File.Move(temp, path, true); }

static async Task Prepare(string game, string cache)
{
    const string unityVersion = "6000.3.19";
    const string unityHash = "d56696d71b4dd8eed625ad688edc050c065e02d5866149e48e52a151bb20e477";
    const string cppHash = "663fb432433b4371fd1ee0ebc321a8fff2a9aac5ac4230c843f9e03ddee4e04c";
    if (!File.Exists(Path.Combine(game, "StarTrekOutpostsUnknown.exe"))) throw new IOException("Not an Outposts game folder");
    if (Within(cache, game) || Within(game, cache)) throw new IOException("Wrapper cache must be outside game folder");
    string binary = Path.Combine(game, "GameAssembly.dll"), metadata = Path.Combine(game, "StarTrekOutpostsUnknown_Data", "il2cpp_data", "Metadata", "global-metadata.dat");
    string version = FileVersionInfo.GetVersionInfo(Path.Combine(game, "UnityPlayer.dll")).FileVersion ?? "";
    if (!version.StartsWith(unityVersion + ".", StringComparison.Ordinal) && !version.StartsWith(unityVersion + "f", StringComparison.Ordinal)) throw new NotSupportedException("Unsupported Unity version: " + version);
    string cpp = Path.Combine(AppContext.BaseDirectory, "Cpp2IL.exe");
    if (Hash(cpp) != cppHash) throw new IOException("Cpp2IL checksum mismatch");
    string binaryHash = Hash(binary), metadataHash = Hash(metadata);
    string generatorHash = Hash(typeof(Il2CppInteropGenerator).Assembly.Location);
    string identity = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes($"{binaryHash}|{metadataHash}|{generatorHash}|{cppHash}|{unityHash}|optout-v1"))).ToLowerInvariant();
    Directory.CreateDirectory(cache);
    using var gate = new FileStream(Path.Combine(cache, "prepare.lock"), FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None);
    string destination = Path.Combine(cache, identity), wrappers = Path.Combine(destination, "assemblies");
    if (Valid(wrappers, identity)) { Activate(); return; }
    string stage = Path.Combine(cache, ".prepare-" + Guid.NewGuid().ToString("N"));
    Directory.CreateDirectory(stage);
    try
    {
        string zip = Path.Combine(cache, "Unity-" + unityVersion + ".zip");
        if (!File.Exists(zip) || Hash(zip) != unityHash)
        {
            Console.Error.WriteLine("Downloading Unity reference assemblies…");
            using var client = new HttpClient { Timeout = TimeSpan.FromMinutes(3) };
            using var response = await client.GetAsync($"https://github.com/LavaGang/MelonLoader.UnityDependencies/releases/download/{unityVersion}/Managed.zip", HttpCompletionOption.ResponseHeadersRead);
            response.EnsureSuccessStatusCode();
            string temp = Path.Combine(stage, "Unity.zip");
            await using (var output = File.Create(temp)) { await response.Content.CopyToAsync(output); }
            if (Hash(temp) != unityHash) throw new IOException("Unity reference archive checksum mismatch");
            File.Move(temp, zip, true);
        }
        string unity = Path.Combine(stage, "unity");
        ZipFile.ExtractToDirectory(zip, unity);
        string dummy = Path.Combine(stage, "dummy"), outputDir = Path.Combine(stage, "assemblies");
        Console.Error.WriteLine("Reading game metadata; first preparation can take several minutes…");
        var command = new ProcessStartInfo(cpp) { WorkingDirectory = stage, UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (string arg in new[] { "--game-path", game, "--exe-name", "StarTrekOutpostsUnknown", "--output-as", "dummydll", "--use-processor", "attributeanalyzer,attributeinjector", "--output-to", dummy }) command.ArgumentList.Add(arg);
        command.Environment["NO_COLOR"] = "1";
        using (var process = Process.Start(command) ?? throw new IOException("Cpp2IL did not start"))
        {
            Task<string> stdout = process.StandardOutput.ReadToEndAsync(), stderr = process.StandardError.ReadToEndAsync();
            await process.WaitForExitAsync();
            await File.WriteAllTextAsync(Path.Combine(cache, "prepare.log"), await stdout + "\n" + await stderr);
            if (process.ExitCode != 0) throw new IOException("Cpp2IL failed; see cache prepare.log");
        }
        Console.Error.WriteLine("Generating local interop assemblies…");
        var resolver = new Resolver();
        var inputs = Directory.GetFiles(dummy, "*.dll").Select(path => ModuleDefinition.FromFile(path, new ModuleReaderParameters { ModuleResolver = resolver })).ToList();
        foreach (var module in inputs) resolver.Add(module);
        var options = new GeneratorOptions { Source = inputs.Select(module => module.Assembly!).ToList(), GameAssemblyPath = binary, OutputDir = outputDir, UnityBaseLibsDir = unity, Il2CppPrefixMode = GeneratorOptions.PrefixMode.OptOut, Parallel = true };
        Il2CppInteropGenerator.Create(options).AddInteropAssemblyGenerator().Run();
        if (Hash(binary) != binaryHash || Hash(metadata) != metadataHash) throw new IOException("Game updated during preparation; retry");
        string[] required = { "Il2Cppmscorlib.dll", "Il2CppMFGSimClient.dll", "Il2CppMFGStatViews.dll", "Il2CppMFGTooltips.dll", "UnityEngine.CoreModule.dll", "UnityEngine.UI.dll", "Unity.InputSystem.dll" };
        foreach (string name in required) if (!File.Exists(Path.Combine(outputDir, name))) throw new IOException("Generated assembly missing: " + name);
        Save(Path.Combine(outputDir, "ready.json"), new { identity, files = Directory.GetFiles(outputDir, "*.dll").Select(path => new { name = Path.GetFileName(path), sha256 = Hash(path) }).ToArray() });
        // Never replace a cache generation another process may already have loaded.
        if (Directory.Exists(destination)) throw new IOException("Existing wrapper cache is incomplete or modified; preserve it and choose another cache folder");
        Directory.CreateDirectory(destination);
        Directory.Move(outputDir, wrappers);
        Activate();
    }
    finally { if (Within(stage, cache) && Path.GetFileName(stage).StartsWith(".prepare-", StringComparison.Ordinal)) Directory.Delete(stage, true); }

    void Activate()
    {
        Save(Path.Combine(cache, "active.json"), new { schema = 1, game_dir = game, game_assembly_sha256 = binaryHash, metadata_sha256 = metadataHash, unity_version = unityVersion, wrappers_dir = wrappers });
        Console.WriteLine(wrappers);
    }
}

static bool Valid(string directory, string identity)
{
    string marker = Path.Combine(directory, "ready.json");
    if (!File.Exists(marker)) return false;
    try
    {
        using var document = JsonDocument.Parse(File.ReadAllText(marker));
        if (document.RootElement.GetProperty("identity").GetString() != identity) return false;
        var files = document.RootElement.GetProperty("files");
        if (files.GetArrayLength() < 7) return false;
        foreach (var entry in files.EnumerateArray())
        {
            string name = entry.GetProperty("name").GetString()!;
            if (Path.GetFileName(name) != name || !name.EndsWith(".dll", StringComparison.Ordinal)) return false;
            if (Hash(Path.Combine(directory, name)) != entry.GetProperty("sha256").GetString()) return false;
        }
        return true;
    }
    catch (Exception e) when (e is IOException or JsonException or KeyNotFoundException or InvalidOperationException or UnauthorizedAccessException) { return false; }
}

sealed class Resolver : INetModuleResolver
{
    readonly Dictionary<string, ModuleDefinition> modules = new();
    public void Add(ModuleDefinition module) => modules[module.Name!] = module;
    public ModuleDefinition? Resolve(string name) => modules.GetValueOrDefault(name);
    public void Dispose() => modules.Clear();
}

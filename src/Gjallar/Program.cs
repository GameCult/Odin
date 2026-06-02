using GameCult.Caching.MessagePack;
using GameCult.Mesh;

var options = GjallarOptions.Parse(args);
var cachePath = Path.GetFullPath(options.CachePath);

if (!File.Exists(cachePath))
{
    Console.Error.WriteLine($"Gjallar Persona CultCache store not found: {cachePath}");
    return 2;
}

using var node = await CultMesh.CreateNodeAsync(
    cachePath,
    new CultMeshNodeOptions
    {
        StartServer = options.Serve,
        CacheOptions = new CultCacheOpenOptions
        {
            // The store is canonical now. Typed C# decode waits for a generated
            // gamecult.persona_state.v0 document model instead of pretending
            // JSON is the runtime body.
            PullOnOpen = false
        }
    }).ConfigureAwait(false);

Console.WriteLine("Gjallar CultMesh organ initialized.");
Console.WriteLine($"Persona store: {cachePath}");
Console.WriteLine("Persona schema: gamecult.persona_state.v0");
Console.WriteLine("Persona key: persona:gjallar");
Console.WriteLine($"CultMesh server: {(options.Serve ? "started" : "disabled")}");
Console.WriteLine("Typed C# Persona decode/publication is the next cut after the schema model is generated.");

if (!options.Serve)
{
    return 0;
}

var stopping = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
Console.CancelKeyPress += (_, eventArgs) =>
{
    eventArgs.Cancel = true;
    stopping.TrySetResult();
};

Console.WriteLine("Press Ctrl+C to stop Gjallar.");
await stopping.Task.ConfigureAwait(false);
return 0;

internal sealed record GjallarOptions(string CachePath, bool Serve)
{
    public static GjallarOptions Parse(string[] args)
    {
        var repoRoot = FindRepoRoot(Directory.GetCurrentDirectory());
        var cachePath = Path.Combine(repoRoot, "personas", "gjallar.persona_state.cc");
        var serve = false;

        for (var index = 0; index < args.Length; index++)
        {
            switch (args[index])
            {
                case "--serve":
                    serve = true;
                    break;
                case "--cache" when index + 1 < args.Length:
                    cachePath = args[++index];
                    break;
                case "--help":
                case "-h":
                    PrintUsage();
                    Environment.Exit(0);
                    break;
                default:
                    throw new ArgumentException($"Unknown Gjallar argument: {args[index]}");
            }
        }

        return new GjallarOptions(cachePath, serve);
    }

    private static string FindRepoRoot(string start)
    {
        var current = new DirectoryInfo(start);
        while (current != null)
        {
            if (File.Exists(Path.Combine(current.FullName, "package.json")) &&
                Directory.Exists(Path.Combine(current.FullName, "personas")))
            {
                return current.FullName;
            }

            current = current.Parent;
        }

        return start;
    }

    private static void PrintUsage()
    {
        Console.WriteLine("Gjallar");
        Console.WriteLine("  --cache <path>  Persona CultCache store path.");
        Console.WriteLine("  --serve         Start the CultMesh server instead of only opening the node.");
    }
}

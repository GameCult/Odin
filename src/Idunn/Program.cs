using GameCult.Caching.MessagePack;
using GameCult.Mesh;

var options = IdunnOptions.Parse(args);
var cachePath = Path.GetFullPath(options.CachePath);
var cacheDirectory = Path.GetDirectoryName(cachePath);

if (!string.IsNullOrWhiteSpace(cacheDirectory))
{
    Directory.CreateDirectory(cacheDirectory);
}

using var node = await CultMesh.CreateNodeAsync(
    cachePath,
    new CultMeshNodeOptions
    {
        StartServer = options.Serve,
        CacheOptions = new CultCacheOpenOptions
        {
            PullOnOpen = false
        }
    }).ConfigureAwait(false);

Console.WriteLine("Idunn CultMesh organ initialized.");
Console.WriteLine($"Keepalive store: {cachePath}");
Console.WriteLine("Provider id: idunn.keepalive");
Console.WriteLine("Authority: daemon lifecycle, health watching, restart intent, and operator escalation for Odin-known daemons");
Console.WriteLine("Owner escalation: Bifrost CultMesh bridge; VoidBot owner DM is compatibility delivery only");
Console.WriteLine($"CultMesh server: {(options.Serve ? "started" : "disabled")}");
Console.WriteLine("Typed keepalive records and restart adapters are the next cut.");

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

Console.WriteLine("Press Ctrl+C to stop Idunn.");
await stopping.Task.ConfigureAwait(false);
return 0;

internal sealed record IdunnOptions(string CachePath, bool Serve)
{
    public static IdunnOptions Parse(string[] args)
    {
        var repoRoot = FindRepoRoot(Directory.GetCurrentDirectory());
        var cachePath = Path.Combine(repoRoot, "scratch", "idunn", "idunn.keepalive.cc");
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
                    throw new ArgumentException($"Unknown Idunn argument: {args[index]}");
            }
        }

        return new IdunnOptions(cachePath, serve);
    }

    private static string FindRepoRoot(string start)
    {
        var current = new DirectoryInfo(start);
        while (current != null)
        {
            if (File.Exists(Path.Combine(current.FullName, "package.json")) &&
                Directory.Exists(Path.Combine(current.FullName, "src", "Gjallar")))
            {
                return current.FullName;
            }

            current = current.Parent;
        }

        return start;
    }

    private static void PrintUsage()
    {
        Console.WriteLine("Idunn");
        Console.WriteLine("  --cache <path>  Keepalive CultCache store path.");
        Console.WriteLine("  --serve         Start the CultMesh server instead of only opening the node.");
    }
}

using System.Buffers;
using System.Diagnostics;
using System.Globalization;
using System.IO.Compression;
using System.Net.WebSockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using CultMath;

var config = GjallarConfig.Parse(args);
using var renderer = new GjallarRenderer(config);
await renderer.RunAsync();

internal sealed record GjallarConfig(
    string FramebufferPath,
    string Url,
    string ProviderId,
    int RefreshHz,
    string StatusPath,
    string FrameDumpPath,
    int Frames,
    int Width,
    int Height,
    string FontPath)
{
    public static GjallarConfig Parse(IReadOnlyList<string> args) => new(
        StringArg(args, "--fb", "/dev/fb0"),
        StringArg(args, "--url", "ws://192.168.1.66:8797/eve/deck"),
        StringArg(args, "--provider", "odin.allseer"),
        IntArg(args, "--refresh-hz", 2),
        StringArg(args, "--stats-path", "/var/log/gjallar.status"),
        StringArg(args, "--frame-dump-path", ""),
        IntArg(args, "--frames", 0),
        IntArg(args, "--width", 0),
        IntArg(args, "--height", 0),
        StringArg(args, "--font", ""));

    private static string StringArg(IReadOnlyList<string> args, string name, string fallback)
    {
        for (var i = 0; i < args.Count - 1; i++)
        {
            if (string.Equals(args[i], name, StringComparison.OrdinalIgnoreCase))
            {
                return args[i + 1];
            }
        }

        return fallback;
    }

    private static int IntArg(IReadOnlyList<string> args, string name, int fallback) =>
        int.TryParse(StringArg(args, name, ""), NumberStyles.Integer, CultureInfo.InvariantCulture, out var value) ? value : fallback;
}

internal sealed class GjallarRenderer : IDisposable
{
    private static readonly JsonSerializerOptions StatusJson = new() { WriteIndented = true };
    private readonly GjallarConfig config;
    private readonly FramebufferDevice framebuffer;
    private readonly FontAtlas fonts;
    private readonly CancellationTokenSource stopping = new();
    private readonly PeriodicTimer timer;
    private readonly TimeSpan frameInterval;
    private byte[]? latestStateJson;
    private int frameIndex;
    private long lastStatusTick;
    private int lastStatusFrame;
    private SceneCache? sceneCache;
    private FrameTimings lastTimings;

    public GjallarRenderer(GjallarConfig config)
    {
        this.config = config;
        framebuffer = FramebufferDevice.Open(config.FramebufferPath, config.Width, config.Height);
        fonts = FontAtlas.Load(config.FontPath);
        frameInterval = TimeSpan.FromSeconds(1.0 / Math.Max(1, config.RefreshHz));
        timer = new PeriodicTimer(frameInterval);
        lastStatusTick = Stopwatch.GetTimestamp();
    }

    public async Task RunAsync()
    {
        var receiveTask = Task.Run(() => ConnectReceiveLoopAsync(stopping.Token));
        var rendered = 0;
        while (config.Frames <= 0 || rendered < config.Frames)
        {
            var started = DateTimeOffset.UtcNow;
            var frameStarted = Stopwatch.GetTimestamp();
            lastTimings = RenderOnce();
            rendered++;
            var paintMs = ElapsedMilliseconds(frameStarted, Stopwatch.GetTimestamp());
            WriteStatusIfDue("rendered", rendered, paintMs, lastTimings, started);
            if (!string.IsNullOrWhiteSpace(config.FrameDumpPath))
            {
                framebuffer.WritePpm(config.FrameDumpPath);
            }

            if (config.Frames > 0 && rendered >= config.Frames)
            {
                break;
            }

            await timer.WaitForNextTickAsync(stopping.Token).ConfigureAwait(false);
        }

        stopping.Cancel();
        try
        {
            await receiveTask.ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (stopping.IsCancellationRequested)
        {
        }
    }

    private static Uri ProviderUri(string url, string providerId)
    {
        if (string.IsNullOrWhiteSpace(providerId))
        {
            return new Uri(url);
        }

        var trimmed = url.TrimEnd('/');
        if (trimmed.EndsWith("/" + providerId, StringComparison.Ordinal))
        {
            return new Uri(trimmed);
        }

        return new Uri($"{trimmed}/{Uri.EscapeDataString(providerId)}");
    }

    private async Task ConnectReceiveLoopAsync(CancellationToken token)
    {
        var target = ProviderUri(config.Url, config.ProviderId);
        while (!token.IsCancellationRequested)
        {
            try
            {
                using var socket = new ClientWebSocket();
                await socket.ConnectAsync(target, token).ConfigureAwait(false);
                await ReceiveLoopAsync(socket, token).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (token.IsCancellationRequested)
            {
                return;
            }
            catch when (!token.IsCancellationRequested)
            {
                await Task.Delay(TimeSpan.FromSeconds(2), token).ConfigureAwait(false);
            }
        }
    }

    private async Task ReceiveLoopAsync(ClientWebSocket socket, CancellationToken token)
    {
        var buffer = ArrayPool<byte>.Shared.Rent(1024 * 1024);
        try
        {
            while (socket.State == WebSocketState.Open)
            {
                using var body = new MemoryStream();
                WebSocketReceiveResult result;
                do
                {
                    result = await socket.ReceiveAsync(buffer, token).ConfigureAwait(false);
                    if (result.MessageType == WebSocketMessageType.Close)
                    {
                        return;
                    }

                    body.Write(buffer, 0, result.Count);
                }
                while (!result.EndOfMessage);

                if (result.MessageType != WebSocketMessageType.Text)
                {
                    continue;
                }

                Interlocked.Exchange(ref latestStateJson, body.ToArray());
            }
        }
        finally
        {
            ArrayPool<byte>.Shared.Return(buffer);
        }
    }

    private FrameTimings RenderOnce()
    {
        var stateJson = Volatile.Read(ref latestStateJson);
        var frame = new FrameDocument(framebuffer.Width, framebuffer.Height, fonts);
        var copyMs = 0.0;
        var decorMs = 0.0;
        var gutterMs = 0.0;
        var presentMs = 0.0;
        if (stateJson is null)
        {
            frame.Clear(ToBgra(Voronoi.SampleTone(0, 0, framebuffer.Height, frameIndex, CultMathTone.Background, Math.Max(framebuffer.Width, framebuffer.Height))));
            var statusFont = fonts.HeaderFor(framebuffer.Height);
            frame.DrawText(fonts.IndexOf(statusFont), 16, 16, "Waiting for Odin Eve surface...", ToBgra(Voronoi.SampleTone(16, 16, framebuffer.Height, frameIndex, CultMathTone.Body, statusFont.Width)));
        }
        else
        {
            var scene = SceneFor(stateJson);
            var step = Stopwatch.GetTimestamp();
            frame.Clear(ColorBgra.Black);
            foreach (var panel in scene.Panels)
            {
                frame.DrawPanel(panel, frameIndex);
            }

            copyMs = ElapsedMilliseconds(step, Stopwatch.GetTimestamp());
            step = Stopwatch.GetTimestamp();
            frame.DrawPanelDecorations(scene.Panels, frameIndex);
            decorMs = ElapsedMilliseconds(step, Stopwatch.GetTimestamp());
            step = Stopwatch.GetTimestamp();
            frame.DrawGutterMaze(scene.GutterCells, frameIndex, scene.MarqueeTape);
            gutterMs = ElapsedMilliseconds(step, Stopwatch.GetTimestamp());
        }

        var presentStarted = Stopwatch.GetTimestamp();
        framebuffer.Present(frame.Pixels);
        presentMs = ElapsedMilliseconds(presentStarted, Stopwatch.GetTimestamp());
        frameIndex++;
        return new FrameTimings(copyMs, decorMs, gutterMs, presentMs);
    }

    private void WriteStatusIfDue(string status, int frames, double paintMs, FrameTimings timings, DateTimeOffset started)
    {
        var now = Stopwatch.GetTimestamp();
        if (frames > 1 && ElapsedMilliseconds(lastStatusTick, now) < 1000)
        {
            return;
        }

        var elapsedSeconds = Math.Max(0.001, (now - lastStatusTick) / (double)Stopwatch.Frequency);
        var measuredFps = (frames - lastStatusFrame) / elapsedSeconds;
        lastStatusTick = now;
        lastStatusFrame = frames;
        WriteStatus(status, frames, paintMs, measuredFps, timings, started);
    }

    private static double ElapsedMilliseconds(long started, long ended) =>
        (ended - started) * 1000.0 / Stopwatch.Frequency;

    private void WriteStatus(string status, int frames, double paintMs, double measuredFps, FrameTimings timings, DateTimeOffset started)
    {
        if (string.IsNullOrWhiteSpace(config.StatusPath))
        {
            return;
        }

        Directory.CreateDirectory(Path.GetDirectoryName(config.StatusPath) ?? ".");
        var document = new
        {
            schema = "gamecult.gjallar.frame.v1",
            service = "gjallar",
            mode = "native-csharp-odin-eve-framebuffer",
            presentMode = "single-contiguous-framebuffer-write",
            status,
            frames,
            framebuffer = new { framebuffer.Width, framebuffer.Height, bytes = framebuffer.BufferBytes },
            refreshHz = config.RefreshHz,
            measuredFps = Math.Round(measuredFps, 2),
            cultMathNative = Voronoi.NativeAvailable,
            paintMs = Math.Round(paintMs, 2),
            timings = new
            {
                copyMs = Math.Round(timings.CopyMs, 2),
                decorMs = Math.Round(timings.DecorMs, 2),
                gutterMs = Math.Round(timings.GutterMs, 2),
                presentMs = Math.Round(timings.PresentMs, 2),
            },
            updatedAtUtc = started.ToString("O", CultureInfo.InvariantCulture),
        };
        File.WriteAllText(config.StatusPath, JsonSerializer.Serialize(document, StatusJson), Encoding.UTF8);
    }

    private static ColorBgra ToBgra(Color32 color) => new(color.r, color.g, color.b);

    private SceneCache SceneFor(byte[] stateJson)
    {
        if (sceneCache is { } existing && ReferenceEquals(existing.StateJson, stateJson))
        {
            return existing;
        }

        using var state = JsonDocument.Parse(stateJson);
        if (!EveNode.TryRoot(state.RootElement, out var root))
        {
            sceneCache = new SceneCache(stateJson, [], [], "");
            return sceneCache;
        }

        var providers = root.PanelChildren().Where(node => !node.IsScaffoldOnly()).ToArray();
        var gutter = GutterSize(fonts.Edge);
        var outerY = gutter;
        var packed = AabbPacker.Pack(providers, new RectI(0, outerY, framebuffer.Width, framebuffer.Height - outerY * 2), gutter).ToArray();
        sceneCache = new SceneCache(stateJson, packed, FrameDocument.BuildGutterCells(framebuffer.Width, framebuffer.Height, fonts.Edge, packed, gutter), root.MarqueeText);
        return sceneCache;
    }

    private static int GutterSize(PsfFont font) =>
        Math.Max(28, Math.Max(font.LineHeight, font.Width) + 8);

    public void Dispose()
    {
        stopping.Cancel();
        stopping.Dispose();
        timer.Dispose();
        framebuffer.Dispose();
    }
}

internal sealed record SceneCache(byte[] StateJson, IReadOnlyList<PackedPanel> Panels, IReadOnlyList<GutterCell> GutterCells, string MarqueeTape);
internal readonly record struct FrameTimings(double CopyMs, double DecorMs, double GutterMs, double PresentMs);

internal sealed class FrameDocument
{
    private readonly FontAtlas fonts;
    public byte[] Pixels { get; }
    public int Width { get; }
    public int Height { get; }
    public int PanelCount { get; private set; }
    public int[] X { get; }
    public int[] Y { get; }
    public int[] W { get; }
    public int[] H { get; }
    public float[] Weight { get; }
    public int[] FontIndex { get; }

    public FrameDocument(int width, int height, FontAtlas fonts, int panelCapacity = 256)
    {
        Width = width;
        Height = height;
        this.fonts = fonts;
        Pixels = GC.AllocateUninitializedArray<byte>(width * height * 4);
        X = new int[panelCapacity];
        Y = new int[panelCapacity];
        W = new int[panelCapacity];
        H = new int[panelCapacity];
        Weight = new float[panelCapacity];
        FontIndex = new int[panelCapacity];
    }

    public void Clear(ColorBgra color)
    {
        for (var i = 0; i < Pixels.Length; i += 4)
        {
            Pixels[i] = color.B;
            Pixels[i + 1] = color.G;
            Pixels[i + 2] = color.R;
            Pixels[i + 3] = 255;
        }
    }

    public void CopyFrom(byte[] pixels) => Buffer.BlockCopy(pixels, 0, Pixels, 0, Math.Min(pixels.Length, Pixels.Length));

    public void DrawPanel(PackedPanel panel, int frameIndex, int depth = 0)
    {
        var rect = panel.Rect;
        var headerFont = fonts.HeaderFor(rect.Height);
        var textItems = panel.Node.TextItems().ToArray();
        var headerPad = headerFont.Height <= 5 ? 2 : 8;
        var headerBand = Math.Min(rect.Height, headerFont.Height + headerPad);
        var headerTextY = rect.Y + (headerFont.Height <= 5 ? 1 : 6);
        var contentW = Math.Max(1, rect.Width - 20);
        var rowStart = rect.Y + headerBand + (headerFont.Height <= 5 ? 1 : 6);
        var maxY = rect.Y + rect.Height - 8;
        var contentH = Math.Max(1, maxY - rowStart);
        _ = fonts.ForTextBox(contentW, contentH, textItems, panel.Weight, out var fontIndex);
        var tones = new ToneBatch(Height, frameIndex);
        var fills = new List<FillCommand>();
        var texts = new List<TextCommand>();
        var index = PanelCount++;
        if (index < X.Length)
        {
            X[index] = rect.X;
            Y[index] = rect.Y;
            W[index] = rect.Width;
            H[index] = rect.Height;
            Weight[index] = panel.Weight;
            FontIndex[index] = fontIndex;
        }

        fills.Add(new FillCommand(rect, -1));
        fills.Add(new FillCommand(
            new RectI(rect.X + 2, rect.Y + 2, Math.Max(1, rect.Width - 4), Math.Max(1, headerBand - 2)),
            tones.Add(rect.X + 8, rect.Y + 8, CultMathTone.Background, headerFont.Height)));
        AddBorderCommands(rect, tones, fills);

        var title = panel.Node.Title;
        texts.Add(new TextCommand(headerFont, rect.X + 8, headerTextY, title, Math.Max(1, rect.Width - 16), tones.Add(rect.X + 8, headerTextY, CultMathTone.Header, headerFont.Width)));

        var contentX = rect.X + 10;
        var colors = tones.Resolve();
        foreach (var fill in fills)
        {
            FillRect(fill.Rect, fill.ColorIndex < 0 ? ColorBgra.Black : colors[fill.ColorIndex]);
        }

        foreach (var text in texts)
        {
            DrawText(text.Font, text.X, text.Y, text.Text, colors[text.ColorIndex], text.MaxWidth);
        }

        if (depth >= 4 || !panel.Node.ShouldRenderNestedPanels() || rect.Width < 96 || contentH < 42)
        {
            DrawPanelText(panel, frameIndex, textItems, rowStart, maxY, contentW, contentX);
            return;
        }

        var children = panel.Node.RenderableChildren().ToArray();
        if (children.Length == 0)
        {
            DrawPanelText(panel, frameIndex, textItems, rowStart, maxY, contentW, contentX);
            return;
        }

        var childGap = Math.Max(4, Math.Max(fonts.Edge.LineHeight, fonts.Edge.Width) / 2);
        var childRect = new RectI(contentX - 2, rowStart, Math.Max(1, rect.Width - 16), Math.Max(1, maxY - rowStart));
        var packedChildren = AabbPacker.Pack(children, childRect, childGap).ToArray();
        if (packedChildren.Length == 0)
        {
            DrawPanelText(panel, frameIndex, textItems, rowStart, maxY, contentW, contentX);
            return;
        }

        foreach (var child in packedChildren)
        {
            DrawPanel(child, frameIndex, depth + 1);
        }
    }

    private void DrawPanelText(PackedPanel panel, int frameIndex, IReadOnlyList<TextItem> textItems, int rowStart, int maxY, int contentW, int contentX)
    {
        var rect = panel.Rect;
        var contentH = Math.Max(1, maxY - rowStart);
        var font = fonts.ForTextBox(contentW, contentH, textItems, panel.Weight, out _);
        var tones = new ToneBatch(Height, frameIndex);
        var texts = new List<TextCommand>();
        var row = rowStart;
        var lineHeight = font.LineHeight;
        foreach (var item in textItems)
        {
            if (row + font.Height > maxY)
            {
                texts.Add(new TextCommand(font, contentX, maxY - font.Height, "... more", contentW, tones.Add(contentX, maxY - font.Height, CultMathTone.Header, font.Width)));
                break;
            }

            foreach (var line in Wrap(item.RenderText, font, contentW, Math.Max(1, (maxY - row) / lineHeight)))
            {
                var tone = item.Prefix is "*" or "+" ? CultMathTone.Header : CultMathTone.Body;
                texts.Add(new TextCommand(font, contentX, row, line, contentW, tones.Add(contentX, row, tone, font.Width)));
                row += lineHeight;
                if (row + font.Height > maxY)
                {
                    break;
                }
            }

            row += font.BlockSpacing;
        }

        var colors = tones.Resolve();
        foreach (var text in texts)
        {
            DrawText(text.Font, text.X, text.Y, text.Text, colors[text.ColorIndex], text.MaxWidth);
        }
    }

    public void DrawPanelDecorations(IReadOnlyList<PackedPanel> panels, int frameIndex)
    {
        var tones = new ToneBatch(Height, frameIndex);
        var fills = new List<FillCommand>();
        foreach (var panel in panels)
        {
            AddBorderCommands(panel.Rect, tones, fills);
        }

        var colors = tones.Resolve();
        foreach (var fill in fills)
        {
            FillRect(fill.Rect, colors[fill.ColorIndex]);
        }
    }

    public void DrawGutterMaze(IReadOnlyList<GutterCell> cells, int frameIndex, string marqueeTape)
    {
        var font = fonts.Edge;
        var tones = new ToneBatch(Height, frameIndex);
        var glyphs = BuildMazePoetry(font, cells, frameIndex, tones, marqueeTape);
        var colors = tones.Resolve();
        foreach (var glyph in glyphs)
        {
            DrawGlyph(font, glyph.X, glyph.Y, glyph.Character, colors[glyph.ColorIndex]);
        }
    }

    public void DrawText(int fontIndex, int x, int y, string text, ColorBgra color) => DrawText(fonts[fontIndex], x, y, text, color, Width - x);

    private void DrawText(PsfFont font, int x, int y, string text, ColorBgra color, int maxWidth)
    {
        var cursor = x;
        var limit = x + Math.Max(0, maxWidth);
        foreach (var ch in text.Replace('\r', ' ').Replace('\n', ' '))
        {
            if (cursor + font.Width > limit)
            {
                break;
            }

            DrawGlyph(font, cursor, y, ch, color);
            cursor += font.Width;
        }
    }

    private void DrawGlyph(PsfFont font, int x, int y, char ch, ColorBgra color)
    {
        var glyph = font.Glyph(ch);
        for (var gy = 0; gy < font.Height; gy++)
        {
            var start = gy * font.BytesPerRow;
            for (var gx = 0; gx < font.Width; gx++)
            {
                var b = glyph[start + gx / 8];
                var mask = 0x80 >> (gx % 8);
                if ((b & mask) != 0)
                {
                    SetPixel(x + gx, y + gy, color);
                }
            }
        }
    }

    public static IReadOnlyList<GutterCell> BuildGutterCells(int width, int height, PsfFont font, IReadOnlyList<PackedPanel> panels, int edgeGutterHeight)
    {
        var columns = Math.Max(1, (width + Math.Max(1, font.Width) - 1) / Math.Max(1, font.Width));
        var spans = MergeVerticalSpans(panels.Select(panel => (Start: panel.Rect.Y, End: panel.Rect.Y + panel.Rect.Height)));
        var cells = new List<GutterCell>();
        var edge = Math.Clamp(edgeGutterHeight, font.Height, Math.Max(font.Height, height / 3));
        var lane = 0;
        AddHorizontalGutterLane(cells, columns, font, width, lane++, 0, edge);

        var cursorY = edge;
        var bottomStart = Math.Max(edge, height - edge);
        foreach (var span in spans)
        {
            var start = Math.Clamp(span.Start, edge, bottomStart);
            var end = Math.Clamp(span.End, edge, bottomStart);
            AddHorizontalGutterLane(cells, columns, font, width, lane++, cursorY, Math.Max(0, start - cursorY));
            cursorY = Math.Max(cursorY, end);
        }

        AddHorizontalGutterLane(cells, columns, font, width, lane++, cursorY, Math.Max(0, bottomStart - cursorY));
        AddHorizontalGutterLane(cells, columns, font, width, lane, bottomStart, Math.Max(0, height - bottomStart));
        return cells;
    }

    private static IReadOnlyList<(int Start, int End)> MergeVerticalSpans(IEnumerable<(int Start, int End)> spans)
    {
        var sorted = spans
            .Select(span => (Start: Math.Max(0, span.Start), End: Math.Max(0, span.End)))
            .Where(span => span.End > span.Start)
            .OrderBy(span => span.Start)
            .ToArray();
        var merged = new List<(int Start, int End)>();
        foreach (var span in sorted)
        {
            if (merged.Count == 0 || span.Start > merged[^1].End)
            {
                merged.Add(span);
                continue;
            }

            var last = merged[^1];
            merged[^1] = (last.Start, Math.Max(last.End, span.End));
        }

        return merged;
    }

    private static void AddHorizontalGutterLane(List<GutterCell> cells, int columns, PsfFont font, int width, int lane, int y, int height)
    {
        if (height < font.Height)
        {
            return;
        }

        var textY = y + Math.Max(0, (height - font.Height) / 2);
        var forward = lane % 2 == 0;
        for (var laneColumn = 0; laneColumn < columns; laneColumn++)
        {
            var x = laneColumn * font.Width;
            if (x >= width)
            {
                continue;
            }

            cells.Add(new GutterCell(x, textY, lane, laneColumn, forward));
        }
    }

    private IReadOnlyList<GlyphCommand> BuildMazePoetry(PsfFont font, IReadOnlyList<GutterCell> cells, int frameIndex, ToneBatch tones, string marqueeTape)
    {
        const int framesPerScrollCell = 8;
        var tape = MarketPoetryTape(marqueeTape);
        var scroll = (frameIndex / framesPerScrollCell) % Math.Max(1, tape.Length);
        var glyphs = new List<GlyphCommand>();
        foreach (var cell in cells)
        {
            var tapeIndex = cell.Forward
                ? cell.LaneColumn + scroll + cell.Row * 7
                : cell.LaneColumn - scroll + cell.Row * 7;
            var character = tape[((tapeIndex % tape.Length) + tape.Length) % tape.Length];
            if (character == ' ')
            {
                continue;
            }

            glyphs.Add(new GlyphCommand(cell.X, cell.Y, character, tones.Add(cell.X, cell.Y, CultMathTone.Edge, font.Width)));
        }

        return glyphs;
    }

    private static string MarketPoetryTape(string marqueeTape)
    {
        var market = string.IsNullOrWhiteSpace(marqueeTape) ? "" : marqueeTape.Trim();
        return string.IsNullOrWhiteSpace(market)
            ? " wake the colossus / coherence over velocity / cultmesh carries signal / cultcache remembers / love is disciplined openness / no cache pretends to truth / "
            : $" {market} / wake the colossus / ";
    }

    private static void AddBorderCommands(RectI rect, ToneBatch tones, List<FillCommand> fills)
    {
        const int chunk = 40;
        for (var x = rect.X; x < rect.X + rect.Width; x += chunk)
        {
            var span = Math.Min(chunk, rect.X + rect.Width - x);
            fills.Add(new FillCommand(new RectI(x, rect.Y, span, 2), tones.Add(x, rect.Y, CultMathTone.Edge, chunk)));
            fills.Add(new FillCommand(new RectI(x, rect.Y + rect.Height - 2, span, 2), tones.Add(x, rect.Y + rect.Height - 2, CultMathTone.Edge, chunk)));
        }

        for (var y = rect.Y; y < rect.Y + rect.Height; y += chunk)
        {
            var span = Math.Min(chunk, rect.Y + rect.Height - y);
            fills.Add(new FillCommand(new RectI(rect.X, y, 2, span), tones.Add(rect.X, y, CultMathTone.Edge, chunk)));
            fills.Add(new FillCommand(new RectI(rect.X + rect.Width - 2, y, 2, span), tones.Add(rect.X + rect.Width - 2, y, CultMathTone.Edge, chunk)));
        }
    }

    private void FillRect(RectI rect, ColorBgra color)
    {
        var x0 = Math.Clamp(rect.X, 0, Width);
        var y0 = Math.Clamp(rect.Y, 0, Height);
        var x1 = Math.Clamp(rect.X + rect.Width, 0, Width);
        var y1 = Math.Clamp(rect.Y + rect.Height, 0, Height);
        for (var y = y0; y < y1; y++)
        {
            var offset = (y * Width + x0) * 4;
            for (var x = x0; x < x1; x++)
            {
                Pixels[offset++] = color.B;
                Pixels[offset++] = color.G;
                Pixels[offset++] = color.R;
                Pixels[offset++] = 255;
            }
        }
    }

    private void SetPixel(int x, int y, ColorBgra color)
    {
        if ((uint)x >= (uint)Width || (uint)y >= (uint)Height)
        {
            return;
        }

        var offset = (y * Width + x) * 4;
        Pixels[offset] = color.B;
        Pixels[offset + 1] = color.G;
        Pixels[offset + 2] = color.R;
        Pixels[offset + 3] = 255;
    }

    private static IEnumerable<string> Wrap(string text, PsfFont font, int width, int maxLines)
    {
        var columns = Math.Max(1, width / Math.Max(1, font.Width));
        var current = "";
        var emitted = 0;
        foreach (var word0 in text.Replace("\r", "").Replace("\n", " / ").Split(' ', StringSplitOptions.RemoveEmptyEntries))
        {
            var word = word0;
            var candidate = current.Length == 0 ? word : current + " " + word;
            if (candidate.Length <= columns)
            {
                current = candidate;
                continue;
            }

            if (current.Length > 0)
            {
                yield return current;
                emitted++;
                if (emitted >= maxLines)
                {
                    yield break;
                }
            }

            while (word.Length > columns)
            {
                yield return word[..columns];
                emitted++;
                if (emitted >= maxLines)
                {
                    yield break;
                }

                word = word[columns..];
            }

            current = word;
        }

        if (current.Length > 0 && emitted < maxLines)
        {
            yield return current;
        }
    }
}

internal static class AabbPacker
{
    public static IReadOnlyList<PackedPanel> Pack(IReadOnlyList<EveNode> nodes, RectI rect, int gap)
    {
        var weighted = nodes
            .Select(node => new PackedPanel(node, rect, Weight(node)))
            .OrderByDescending(item => item.Weight)
            .ToArray();
        var result = new List<PackedPanel>();
        var cursor = ReserveVerticalCollections(weighted, rect, gap, result, out var remaining);
        Squarify(remaining, cursor, gap, result);
        return result;
    }

    private static RectI ReserveVerticalCollections(IReadOnlyList<PackedPanel> items, RectI rect, int gap, List<PackedPanel> output, out IReadOnlyList<PackedPanel> remaining)
    {
        var vertical = items.Where(static item => item.Node.PrefersVerticalPanel()).ToList();
        if (vertical.Count == 0 || items.Count < 2 || rect.Width < 240 || rect.Height < 96)
        {
            remaining = items;
            return rect;
        }

        var keep = items.Except(vertical).ToList();
        var cursor = rect;
        var maxReserved = Math.Max(96, (int)MathF.Round(rect.Width * 0.45f));
        var reserved = 0;
        foreach (var panel in vertical.OrderByDescending(static item => item.Weight))
        {
            if (cursor.Width < 160 || reserved >= maxReserved)
            {
                keep.Add(panel);
                continue;
            }

            var desired = PreferredVerticalWidth(panel, rect);
            var width = Math.Min(desired, Math.Min(cursor.Width - 96, maxReserved - reserved));
            if (width < 72)
            {
                keep.Add(panel);
                continue;
            }

            var x = cursor.X + cursor.Width - width;
            output.Add(panel with { Rect = Inset(new RectI(x, cursor.Y, width, cursor.Height), gap) });
            cursor = new RectI(cursor.X, cursor.Y, Math.Max(0, cursor.Width - width), cursor.Height);
            reserved += width;
        }

        remaining = keep.OrderByDescending(static item => item.Weight).ToArray();
        return cursor;
    }

    private static int PreferredVerticalWidth(PackedPanel panel, RectI rect)
    {
        var totalWeight = Math.Max(0.001f, panel.Weight);
        var areaWidth = (int)MathF.Round(MathF.Sqrt(totalWeight) * 56.0f);
        var childPressure = Math.Min(90, panel.Node.RenderableChildCount() * 6);
        return Math.Clamp(Math.Max(areaWidth, 104 + childPressure), 112, Math.Max(112, rect.Width / 3));
    }

    private static void Squarify(IReadOnlyList<PackedPanel> items, RectI rect, int gap, List<PackedPanel> output)
    {
        if (items.Count == 0 || rect.Width <= 4 || rect.Height <= 4)
        {
            return;
        }

        if (items.Count == 1)
        {
            output.Add(items[0] with { Rect = rect });
            return;
        }

        var totalWeight = Math.Max(0.001f, items.Sum(item => item.Weight));
        var totalArea = Math.Max(1.0f, rect.Width * rect.Height);
        var remaining = items
            .Select(item => new TreemapItem(item, Math.Max(1.0f, item.Weight / totalWeight * totalArea)))
            .ToList();
        var row = new List<TreemapItem>();
        var cursor = rect;
        var sideIndex = 0;
        while (remaining.Count > 0)
        {
            var next = remaining[0];
            var side = StripLength(cursor, sideIndex);
            if (row.Count == 0 ||
                ShouldGrowMosaicBand(row, cursor, sideIndex) ||
                Worst(row.Append(next), side) <= Worst(row, side))
            {
                row.Add(next);
                remaining.RemoveAt(0);
                continue;
            }

            cursor = LayoutSpiralStrip(row, cursor, gap, sideIndex, output);
            sideIndex++;
            row.Clear();
            if (cursor.Width <= 4 || cursor.Height <= 4)
            {
                break;
            }
        }

        if (row.Count > 0 && cursor.Width > 4 && cursor.Height > 4)
        {
            _ = LayoutSpiralStrip(row, cursor, gap, sideIndex, output);
        }
    }

    private static bool ShouldGrowMosaicBand(IReadOnlyList<TreemapItem> row, RectI rect, int sideIndex)
    {
        if (row.Count == 0)
        {
            return false;
        }

        var sum = row.Sum(item => item.Area);
        var side = StripLength(rect, sideIndex);
        var thickness = sum / Math.Max(1, side);
        var target = sideIndex % 2 == 0 ? 56.0f : 72.0f;
        return thickness < target;
    }

    private static float Worst(IEnumerable<TreemapItem> row, float side)
    {
        var areas = row.Select(item => item.Area).ToArray();
        if (areas.Length == 0)
        {
            return float.MaxValue;
        }

        var sum = areas.Sum();
        var max = areas.Max();
        var min = Math.Max(0.001f, areas.Min());
        var side2 = side * side;
        return Math.Max(side2 * max / (sum * sum), (sum * sum) / (side2 * min));
    }

    private static float StripLength(RectI rect, int sideIndex) => sideIndex % 2 == 0
        ? Math.Max(1, rect.Width)
        : Math.Max(1, rect.Height);

    private static RectI LayoutSpiralStrip(IReadOnlyList<TreemapItem> row, RectI rect, int gap, int sideIndex, List<PackedPanel> output)
    {
        var sum = row.Sum(item => item.Area);
        switch (sideIndex % 4)
        {
            case 0:
                return LayoutHorizontalStrip(row, rect, gap, output, sum, fromTop: true);
            case 1:
                return LayoutVerticalStrip(row, rect, gap, output, sum, fromLeft: false);
            case 2:
                return LayoutHorizontalStrip(row, rect, gap, output, sum, fromTop: false);
            default:
                return LayoutVerticalStrip(row, rect, gap, output, sum, fromLeft: true);
        }
    }

    private static RectI LayoutHorizontalStrip(IReadOnlyList<TreemapItem> row, RectI rect, int gap, List<PackedPanel> output, float sum, bool fromTop)
    {
        var rowHeight = ClampPreferred((int)MathF.Round(sum / Math.Max(1, rect.Width)), 24, rect.Height);
        var x = rect.X;
        var y = fromTop ? rect.Y : rect.Y + rect.Height - rowHeight;
        for (var i = 0; i < row.Count; i++)
        {
            var width = i == row.Count - 1
                ? rect.X + rect.Width - x
                : Math.Max(24, (int)MathF.Round(row[i].Area / Math.Max(1, rowHeight)));
            output.Add(row[i].Panel with { Rect = Inset(new RectI(x, y, width, rowHeight), gap) });
            x += width;
        }

        return fromTop
            ? new RectI(rect.X, rect.Y + rowHeight, rect.Width, Math.Max(0, rect.Height - rowHeight))
            : new RectI(rect.X, rect.Y, rect.Width, Math.Max(0, rect.Height - rowHeight));
    }

    private static RectI LayoutVerticalStrip(IReadOnlyList<TreemapItem> row, RectI rect, int gap, List<PackedPanel> output, float sum, bool fromLeft)
    {
        var rowWidth = ClampPreferred((int)MathF.Round(sum / Math.Max(1, rect.Height)), 32, rect.Width);
        var y = rect.Y;
        var x = fromLeft ? rect.X : rect.X + rect.Width - rowWidth;
        for (var i = 0; i < row.Count; i++)
        {
            var height = i == row.Count - 1
                ? rect.Y + rect.Height - y
                : Math.Max(18, (int)MathF.Round(row[i].Area / Math.Max(1, rowWidth)));
            output.Add(row[i].Panel with { Rect = Inset(new RectI(x, y, rowWidth, height), gap) });
            y += height;
        }

        return fromLeft
            ? new RectI(rect.X + rowWidth, rect.Y, Math.Max(0, rect.Width - rowWidth), rect.Height)
            : new RectI(rect.X, rect.Y, Math.Max(0, rect.Width - rowWidth), rect.Height);
    }

    private static int ClampPreferred(int value, int preferredMin, int max)
    {
        if (max <= 0)
        {
            return 0;
        }

        return max < preferredMin ? max : Math.Clamp(value, preferredMin, max);
    }

    private static RectI Inset(RectI rect, int gap)
    {
        var inset = Math.Max(1, gap / 2);
        return new RectI(rect.X, rect.Y + inset, Math.Max(1, rect.Width), Math.Max(1, rect.Height - inset * 2));
    }

    private static float Weight(EveNode node)
    {
        var items = node.TextItems().Take(160).ToArray();
        var requestedRows = 1.0f;
        var requestedCells = 0.0f;
        foreach (var item in items)
        {
            var length = Math.Clamp(item.Prefix.Length + 1 + item.Text.Length, 8, 96);
            var rows = MathF.Ceiling(length / 72.0f);
            requestedRows += rows;
            requestedCells += rows * Math.Min(72, length);
        }

        if (items.Length == 160)
        {
            requestedRows += 8.0f;
            requestedCells += 8.0f * 72.0f;
        }

        var structuralWeight = node.Kind == "interface" ? 0.75f : 0.35f;
        structuralWeight += MathF.Min(1.5f, node.Children.Count * 0.05f);
        var cellWeight = MathF.Log2(Math.Max(2.0f, requestedCells)) * 0.62f;
        var rowWeight = MathF.Sqrt(requestedRows) * 0.24f;
        return Math.Clamp(structuralWeight + cellWeight + rowWeight, 0.45f, 9.0f);
    }

    private sealed record TreemapItem(PackedPanel Panel, float Area);
}

internal sealed record PackedPanel(EveNode Node, RectI Rect, float Weight);
internal readonly record struct RectI(int X, int Y, int Width, int Height)
{
    public float CenterX => X + Width * 0.5f;
    public float CenterY => Y + Height * 0.5f;
    public bool Contains(int x, int y) => x >= X && x < X + Width && y >= Y && y < Y + Height;
    public bool Intersects(RectI other) =>
        X < other.X + other.Width &&
        X + Width > other.X &&
        Y < other.Y + other.Height &&
        Y + Height > other.Y;
}

internal sealed class EveNode
{
    public string Kind { get; init; } = "";
    public string Title { get; init; } = "";
    public string ProviderId { get; init; } = "";
    public string Role { get; init; } = "";
    public string Text { get; init; } = "";
    public string MarqueeText { get; init; } = "";
    public IReadOnlyList<EveNode> Children { get; init; } = [];

    public static bool TryRoot(JsonElement element, out EveNode root)
    {
        root = new EveNode { Title = "root" };
        if (!element.TryGetProperty("surface", out var surface) || !surface.TryGetProperty("root", out var rootElement))
        {
            return false;
        }

        root = FromJson(rootElement);
        return true;
    }

    public IEnumerable<EveNode> PanelChildren()
    {
        var nodes = Children.Count > 0 ? Children : [this];
        foreach (var node in nodes)
        {
            yield return node.AsPanelNode();
        }
    }

    private EveNode AsPanelNode()
    {
        if (Kind != "interface" || Children.Count != 1)
        {
            return this;
        }

        var child = Children[0];
        return new EveNode
        {
            Title = Title,
            Kind = child.Kind,
            ProviderId = string.IsNullOrWhiteSpace(child.ProviderId) ? ProviderId : child.ProviderId,
            Role = child.Role,
            Text = child.Text,
            MarqueeText = child.MarqueeText,
            Children = child.Children,
        };
    }

    public bool IsScaffoldOnly()
    {
        if (Kind == "verse" && Children.Count == 1 && Children[0].Kind == "service")
        {
            return true;
        }

        if (Children.Count == 1 && Children[0].Children.Count == 0 && IsServiceReference(Children[0].Text))
        {
            return true;
        }

        if (Children.Count != 0)
        {
            return false;
        }

        var text = Text.Trim();
        if (IsServiceReference(text))
        {
            return true;
        }

        if (!text.StartsWith("surface ", StringComparison.OrdinalIgnoreCase) || !text.EndsWith(" nodes", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        var countText = text["surface ".Length..^" nodes".Length].Trim();
        return int.TryParse(countText, NumberStyles.Integer, CultureInfo.InvariantCulture, out _);
    }

    private static bool IsServiceReference(string text)
    {
        var trimmed = text.Trim();
        return trimmed.StartsWith("service-", StringComparison.OrdinalIgnoreCase) && !trimmed.Contains('\n');
    }

    public IEnumerable<TextItem> TextItems()
    {
        foreach (var child in Children.Count == 0 ? [this] : Children)
        {
            if (child.ShouldShowTitleInline(Title))
            {
                yield return new TextItem("*", child.Title);
            }

            foreach (var item in child.CollectTextItems(256))
            {
                yield return item;
            }
        }
    }

    public IEnumerable<EveNode> RenderableChildren()
    {
        foreach (var child in Children)
        {
            var panel = child.AsPanelNode();
            if (!panel.IsScaffoldOnly() && panel.HasPresentationSignal())
            {
                yield return panel;
            }
        }
    }

    public bool ShouldRenderNestedPanels()
    {
        if (Children.Count < 2)
        {
            return false;
        }

        var meaningful = Children
            .Select(static child => child.AsPanelNode())
            .Where(static child => !child.IsScaffoldOnly() && child.HasPresentationSignal())
            .ToArray();
        if (meaningful.Length < 2)
        {
            return false;
        }

        if (meaningful.All(static child => child.IsTextLeaf()))
        {
            return false;
        }

        if (meaningful.All(static child => child.IsCompactPresentationLeaf()))
        {
            return false;
        }

        return meaningful.Any(static child => child.RenderableChildren().Any() || (!child.IsCompactPresentationLeaf() && IsStructuralKind(child.Kind)));
    }

    public bool PrefersVerticalPanel()
    {
        if (!IsOrderedCollectionKind(Kind))
        {
            return false;
        }

        var renderableCount = RenderableChildCount();
        if (renderableCount >= 5)
        {
            return true;
        }

        var textItems = TextItems().Take(16).ToArray();
        return textItems.Length >= 5 && textItems.Average(static item => item.Text.Length) <= 32.0;
    }

    public int RenderableChildCount() => RenderableChildren().Count();

    private bool IsTextLeaf() =>
        Children.Count == 0 && (Kind == "text" || Kind == "metric" || Kind == "avatar");

    private bool ShouldShowTitleInline(string parentTitle)
    {
        if (Title == parentTitle || IsTextLeaf())
        {
            return false;
        }

        return ShouldShowInlineTitle(Title);
    }

    private bool IsCompactPresentationLeaf() =>
        !RenderableChildren().Any() && !TextItems().Any();

    private bool HasPresentationSignal()
    {
        if (!IsPresentationNoiseTitle(Title))
        {
            return true;
        }

        if (!string.IsNullOrWhiteSpace(Text) && !IsPresentationNoiseText(Text))
        {
            return true;
        }

        return Children.Any(static child => child.AsPanelNode().HasPresentationSignal());
    }

    private static bool IsStructuralKind(string kind)
    {
        var value = kind.Trim().ToLowerInvariant();
        return value is "dashboard" or "surface" or "stack" or "grid" or "dock" or "row" or "column" or "pane" or "panel" or "card" or "group" or "list" or "tree" or "cockpit" or "rail" or "interface";
    }

    private static bool IsOrderedCollectionKind(string kind)
    {
        var value = kind.Trim().ToLowerInvariant();
        return value is "list" or "rail" or "menu";
    }

    private static bool IsContentKind(string kind)
    {
        var value = kind.Trim().ToLowerInvariant();
        return value is "text" or "metric" or "avatar";
    }

    private IEnumerable<TextItem> CollectTextItems(int limit)
    {
        if (limit <= 0)
        {
            yield break;
        }

        if (!string.IsNullOrWhiteSpace(Text) && !IsPresentationNoiseText(Text))
        {
            yield return new TextItem(TextPrefixForLeaf(), Text);
            limit--;
        }

        foreach (var child in Children)
        {
            if (child.ShouldShowTitleInline(Title))
            {
                yield return new TextItem("+", child.Title);
                limit--;
            }

            foreach (var item in child.CollectTextItems(limit))
            {
                yield return item;
            }
        }
    }

    private static EveNode FromJson(JsonElement element)
    {
        var props = element.TryGetProperty("props", out var propsElement) && propsElement.ValueKind == JsonValueKind.Object ? propsElement : default;
        var title = StringProp(props, "title");
        var provider = StringProp(props, "providerId");
        var kind = StringProp(element, "kind");
        var role = StringProp(props, "role");
        if (string.IsNullOrWhiteSpace(role))
        {
            role = StringProp(element, "role");
        }

        var text = StringProp(props, "text");
        var marqueeText = StringProp(props, "marqueeText");
        if (string.IsNullOrWhiteSpace(text))
        {
            text = StringProp(element, "text");
        }

        if (string.IsNullOrWhiteSpace(title) && !IsContentKind(kind) && !string.IsNullOrWhiteSpace(text))
        {
            title = text;
            text = "";
        }

        var children = new List<EveNode>();
        if (element.TryGetProperty("children", out var childElements) && childElements.ValueKind == JsonValueKind.Array)
        {
            foreach (var child in childElements.EnumerateArray())
            {
                children.Add(FromJson(child));
            }
        }

        return new EveNode
        {
            Kind = kind,
            Role = role,
            Title = string.IsNullOrWhiteSpace(title) ? (IsContentKind(kind) ? "" : StringProp(element, "id")) : title,
            ProviderId = provider,
            Text = text,
            MarqueeText = marqueeText,
            Children = children,
        };
    }

    private static string StringProp(JsonElement element, string name) =>
        element.ValueKind == JsonValueKind.Object && element.TryGetProperty(name, out var value) ? ValueString(value) : "";

    private static bool ShouldShowInlineTitle(string title) =>
        !string.IsNullOrWhiteSpace(title) && !IsPresentationNoiseTitle(title);

    private static bool IsPresentationNoiseTitle(string title)
    {
        var value = title.Trim();
        if (value.Length == 0)
        {
            return true;
        }

        if (value.StartsWith("fact-", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        return IsServiceReference(value);
    }

    private static bool IsPresentationNoiseText(string text)
    {
        var value = text.Trim();
        if (value.Length == 0)
        {
            return true;
        }

        if (IsServiceReference(value))
        {
            return true;
        }

        if (value.StartsWith("kind:", StringComparison.OrdinalIgnoreCase) ||
            value.StartsWith("health:", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        if (value.StartsWith("surface ", StringComparison.OrdinalIgnoreCase) &&
            value.EndsWith(" nodes", StringComparison.OrdinalIgnoreCase) &&
            int.TryParse(value["surface ".Length..^" nodes".Length].Trim(), NumberStyles.Integer, CultureInfo.InvariantCulture, out _))
        {
            return true;
        }

        return false;
    }

    private static string ValueString(JsonElement value) => value.ValueKind switch
    {
        JsonValueKind.String => value.GetString() ?? "",
        JsonValueKind.Number => value.ToString(),
        JsonValueKind.True => "true",
        JsonValueKind.False => "false",
        _ => "",
    };

    private string TextPrefixForLeaf()
    {
        if (string.Equals(Role, "mono", StringComparison.OrdinalIgnoreCase) && IsJustifiedRowText(Text))
        {
            return "";
        }

        return "-";
    }

    private static bool IsJustifiedRowText(string text)
    {
        var value = text.TrimStart();
        if (value.Length < 4)
        {
            return false;
        }

        var index = 0;
        while (index < value.Length && char.IsDigit(value[index]))
        {
            index++;
        }

        return index > 0 && index + 1 < value.Length && value[index] == '.' && char.IsWhiteSpace(value[index + 1]);
    }
}

internal sealed record TextItem(string Prefix, string Text)
{
    public string RenderText => string.IsNullOrWhiteSpace(Prefix) ? Text : $"{Prefix} {Text}";
}
internal sealed record FillCommand(RectI Rect, int ColorIndex);
internal sealed record TextCommand(PsfFont Font, int X, int Y, string Text, int MaxWidth, int ColorIndex);
internal sealed record GlyphCommand(int X, int Y, char Character, int ColorIndex);
internal sealed record GutterCell(int X, int Y, int Row, int LaneColumn, bool Forward);

internal sealed class ToneBatch(int resolutionY, int frameIndex)
{
    private readonly List<float> xs = [];
    private readonly List<float> ys = [];
    private readonly List<CultMathTone> tones = [];
    private readonly List<float> spans = [];

    public int Add(float x, float y, CultMathTone tone, float span)
    {
        var index = xs.Count;
        xs.Add(x);
        ys.Add(y);
        tones.Add(tone);
        spans.Add(span);
        return index;
    }

    public ColorBgra[] Resolve()
    {
        var nativeColors = new Color32[xs.Count];
        Voronoi.SampleTones(
            CollectionsMarshal.AsSpan(xs),
            CollectionsMarshal.AsSpan(ys),
            CollectionsMarshal.AsSpan(tones),
            CollectionsMarshal.AsSpan(spans),
            resolutionY,
            frameIndex,
            nativeColors);
        return nativeColors.Select(color => new ColorBgra(color.r, color.g, color.b)).ToArray();
    }
}

internal readonly record struct ColorBgra(byte R, byte G, byte B)
{
    public static readonly ColorBgra Black = new(0, 0, 0);
}

internal sealed class FramebufferDevice : IDisposable
{
    private const int OReadWrite = 2;
    private const int ProtRead = 1;
    private const int ProtWrite = 2;
    private const int MapShared = 1;
    private readonly FileStream? stream;
    private readonly int fd = -1;
    private readonly IntPtr mapped;
    public int Width { get; }
    public int Height { get; }
    public int BufferBytes => Width * Height * 4;

    private FramebufferDevice(string path, int width, int height)
    {
        Width = width;
        Height = height;
        if (OperatingSystem.IsLinux())
        {
            fd = open(path, OReadWrite);
            if (fd >= 0)
            {
                mapped = mmap(IntPtr.Zero, (UIntPtr)BufferBytes, ProtRead | ProtWrite, MapShared, fd, IntPtr.Zero);
                if (mapped != new IntPtr(-1))
                {
                    return;
                }

                close(fd);
                fd = -1;
            }
        }

        stream = new FileStream(path, FileMode.Open, FileAccess.ReadWrite, FileShare.ReadWrite, BufferBytes);
    }

    public static FramebufferDevice Open(string path, int width, int height)
    {
        if (width <= 0 || height <= 0)
        {
            (width, height) = ReadVirtualSize(path);
        }

        return new FramebufferDevice(path, width, height);
    }

    public void Present(byte[] pixels)
    {
        if (mapped != IntPtr.Zero && mapped != new IntPtr(-1))
        {
            unsafe
            {
                pixels.AsSpan(0, Math.Min(pixels.Length, BufferBytes)).CopyTo(new Span<byte>((void*)mapped, BufferBytes));
            }

            return;
        }

        if (stream is null)
        {
            return;
        }

        stream.Position = 0;
        stream.Write(pixels, 0, Math.Min(pixels.Length, BufferBytes));
    }

    public void WritePpm(string path)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path) ?? ".");
        using var output = File.Create(path);
        output.Write(Encoding.ASCII.GetBytes($"P6\n{Width} {Height}\n255\n"));
        var rgb = new byte[Width * 3];
        for (var y = 0; y < Height; y++)
        {
            var row = new byte[Width * 4];
            ReadRow(y, row);
            for (var x = 0; x < Width; x++)
            {
                rgb[x * 3] = row[x * 4 + 2];
                rgb[x * 3 + 1] = row[x * 4 + 1];
                rgb[x * 3 + 2] = row[x * 4];
            }

            output.Write(rgb);
        }
    }

    private static (int Width, int Height) ReadVirtualSize(string path)
    {
        var name = Path.GetFileName(path);
        var sysfs = $"/sys/class/graphics/{name}/virtual_size";
        if (File.Exists(sysfs))
        {
            var parts = File.ReadAllText(sysfs).Trim().Split(',');
            if (parts.Length == 2 &&
                int.TryParse(parts[0], out var width) &&
                int.TryParse(parts[1], out var height))
            {
                return (width, height);
            }
        }

        return (1920, 1080);
    }

    private void ReadRow(int y, byte[] row)
    {
        if (mapped != IntPtr.Zero && mapped != new IntPtr(-1))
        {
            unsafe
            {
                new ReadOnlySpan<byte>((void*)(mapped + y * Width * 4), row.Length).CopyTo(row);
            }

            return;
        }

        if (stream is null)
        {
            return;
        }

        stream.Position = y * Width * 4;
        _ = stream.Read(row);
    }

    public void Dispose()
    {
        if (mapped != IntPtr.Zero && mapped != new IntPtr(-1))
        {
            munmap(mapped, (UIntPtr)BufferBytes);
        }

        if (fd >= 0)
        {
            close(fd);
        }

        stream?.Dispose();
    }

    [DllImport("libc", SetLastError = true)]
    private static extern int open(string pathname, int flags);

    [DllImport("libc", SetLastError = true)]
    private static extern IntPtr mmap(IntPtr addr, UIntPtr length, int prot, int flags, int fd, IntPtr offset);

    [DllImport("libc", SetLastError = true)]
    private static extern int munmap(IntPtr addr, UIntPtr length);

    [DllImport("libc", SetLastError = true)]
    private static extern int close(int fd);
}

internal sealed class FontAtlas
{
    private readonly PsfFont[] fonts;
    public PsfFont Default => fonts[0];
    public PsfFont Edge => fonts.FirstOrDefault(font => font.Height >= 14 && font.Width >= 8) ?? fonts.FirstOrDefault(font => font.Height >= 10 && font.Width >= 6) ?? fonts.FirstOrDefault(font => font.Height >= 5 && font.Width >= 3) ?? Default;
    public PsfFont this[int index] => fonts[Math.Clamp(index, 0, fonts.Length - 1)];
    public int IndexOf(PsfFont font) => Math.Max(0, Array.IndexOf(fonts, font));

    private FontAtlas(PsfFont[] fonts) => this.fonts = fonts
        .OrderBy(font => font.Width * font.Height)
        .ThenBy(font => font.Height)
        .ThenBy(font => font.Width)
        .ToArray();

    public static FontAtlas Load(string primary)
    {
        var candidates = new[]
        {
            primary,
            "/usr/share/consolefonts/Lat2-Terminus12x6.psf.gz",
            "/usr/share/consolefonts/Lat7-Terminus12x6.psf.gz",
            "/usr/share/consolefonts/Lat2-Terminus14.psf.gz",
            "/usr/share/consolefonts/Uni2-Fixed14.psf.gz",
            "/usr/share/consolefonts/Lat2-TerminusBoldVGA16.psf.gz",
            "/usr/share/consolefonts/UbuntuMono-R-8x16.psf",
            "/usr/share/consolefonts/Uni3-TerminusBold18x10.psf.gz",
            "/usr/share/consolefonts/Lat7-Terminus20x10.psf.gz",
            "/usr/share/consolefonts/Uni3-TerminusBold22x11.psf.gz",
            "/usr/share/consolefonts/Lat2-Terminus24x12.psf.gz",
            "/usr/share/consolefonts/Uni2-Terminus28x14.psf.gz",
            "/usr/share/consolefonts/Lat2-Terminus32x16.psf.gz",
        };
        var loaded = candidates.Where(path => !string.IsNullOrWhiteSpace(path) && File.Exists(path)).Select(PsfFont.Load).GroupBy(font => (font.Width, font.Height)).Select(group => group.First()).ToArray();
        if (loaded.Length == 0)
        {
            throw new InvalidOperationException("No PSF console fonts found; pass --font.");
        }

        var source = loaded.OrderBy(font => font.Width * font.Height).First();
        var generated = new[]
        {
            source.ShrinkTo(3, 3),
            source.ShrinkTo(3, 5),
            source.ShrinkTo(4, 5),
            source.ShrinkTo(4, 6),
            source.ShrinkTo(5, 7),
            source.ShrinkTo(6, 8),
            source.ShrinkTo(6, 10),
            source.ShrinkTo(7, 11),
        };
        return new FontAtlas(loaded.Concat(generated).GroupBy(font => (font.Width, font.Height)).Select(group => group.First()).ToArray());
    }

    public PsfFont ForTextBox(int width, int height, IReadOnlyList<TextItem> items, float weight, out int index)
    {
        var selected = fonts[0];
        var weightedPressure = Math.Clamp(weight / 4.0f, 0.0f, 5.0f);
        foreach (var font in fonts.OrderByDescending(font => font.Width * font.Height))
        {
            var columns = Math.Max(1, width / Math.Max(1, font.Width));
            var rows = Math.Max(1, height / Math.Max(1, font.LineHeight));
            var desiredRows = EstimateRows(items, columns) + weightedPressure;
            if (desiredRows <= rows)
            {
                selected = font;
                break;
            }
        }

        index = Array.IndexOf(fonts, selected);
        return selected;
    }

    public PsfFont HeaderFor(int height)
    {
        var target = height < 40 ? Math.Max(3, height / 3) : Math.Max(10, Math.Min(24, height / 7));
        return fonts.LastOrDefault(font => font.Height <= target) ?? Default;
    }

    private static float EstimateRows(IReadOnlyList<TextItem> items, int columns)
    {
        if (items.Count == 0)
        {
            return 1.0f;
        }

        var rows = 0.0f;
        foreach (var item in items.Take(96))
        {
            var length = Math.Max(1, item.Prefix.Length + 1 + item.Text.Length);
            rows += MathF.Ceiling(length / (float)Math.Max(1, columns));
            rows += 0.18f;
        }

        if (items.Count > 96)
        {
            rows += 1.0f;
        }

        return rows;
    }
}

internal sealed class PsfFont
{
    private readonly byte[][] glyphs;
    public int Width { get; }
    public int Height { get; }
    public int BytesPerRow { get; }
    public int LineHeight => Height + (Height <= 5 ? 1 : 2);
    public int BlockSpacing => Height <= 5 ? 0 : Math.Max(1, Height / 3);

    private PsfFont(int width, int height, byte[][] glyphs)
    {
        Width = width;
        Height = height;
        BytesPerRow = (width + 7) / 8;
        this.glyphs = glyphs;
    }

    public byte[] Glyph(char ch) => glyphs[Math.Clamp((int)ch, 0, glyphs.Length - 1)];

    public PsfFont ShrinkTo(int width, int height)
    {
        var shrunk = new byte[glyphs.Length][];
        for (var glyphIndex = 0; glyphIndex < glyphs.Length; glyphIndex++)
        {
            shrunk[glyphIndex] = ShrinkGlyph(glyphs[glyphIndex], width, height);
        }

        return new PsfFont(width, height, shrunk);
    }

    public static PsfFont Load(string path)
    {
        var bytes = ReadBytes(path);
        if (bytes.Length > 4 && bytes[0] == 0x36 && bytes[1] == 0x04)
        {
            var glyphCount = (bytes[2] & 0x01) != 0 ? 512 : 256;
            var height = bytes[3];
            return new PsfFont(8, height, ReadGlyphs(bytes, 4, glyphCount, height, height));
        }

        if (bytes.Length > 32 && BitConverter.ToUInt32(bytes, 0) == 0x864ab572)
        {
            var headerSize = (int)BitConverter.ToUInt32(bytes, 8);
            var glyphCount = (int)BitConverter.ToUInt32(bytes, 16);
            var charSize = (int)BitConverter.ToUInt32(bytes, 20);
            var height = (int)BitConverter.ToUInt32(bytes, 24);
            var width = (int)BitConverter.ToUInt32(bytes, 28);
            return new PsfFont(width, height, ReadGlyphs(bytes, headerSize, glyphCount, charSize, charSize));
        }

        throw new InvalidOperationException($"Unsupported PSF font: {path}");
    }

    private static byte[] ReadBytes(string path)
    {
        var bytes = File.ReadAllBytes(path);
        if (!path.EndsWith(".gz", StringComparison.OrdinalIgnoreCase))
        {
            return bytes;
        }

        using var input = new GZipStream(new MemoryStream(bytes), CompressionMode.Decompress);
        using var output = new MemoryStream();
        input.CopyTo(output);
        return output.ToArray();
    }

    private static byte[][] ReadGlyphs(byte[] bytes, int offset, int count, int charSize, int stride)
    {
        var glyphs = new byte[count][];
        for (var i = 0; i < count; i++)
        {
            glyphs[i] = bytes.Skip(offset + i * stride).Take(charSize).ToArray();
        }

        return glyphs;
    }

    private byte[] ShrinkGlyph(byte[] source, int width, int height)
    {
        var bytesPerRow = (width + 7) / 8;
        var target = new byte[height * bytesPerRow];
        for (var y = 0; y < height; y++)
        {
            var sy0 = y * Height / height;
            var sy1 = Math.Max(sy0 + 1, (y + 1) * Height / height);
            for (var x = 0; x < width; x++)
            {
                var sx0 = x * Width / width;
                var sx1 = Math.Max(sx0 + 1, (x + 1) * Width / width);
                var lit = 0;
                var total = 0;
                for (var sy = sy0; sy < sy1; sy++)
                {
                    for (var sx = sx0; sx < sx1; sx++)
                    {
                        total++;
                        if (IsSet(source, sx, sy))
                        {
                            lit++;
                        }
                    }
                }

                if (lit * 2 >= Math.Max(1, total))
                {
                    target[y * bytesPerRow + x / 8] |= (byte)(0x80 >> (x % 8));
                }
            }
        }

        return target;
    }

    private bool IsSet(byte[] glyph, int x, int y)
    {
        if ((uint)x >= (uint)Width || (uint)y >= (uint)Height)
        {
            return false;
        }

        var offset = y * BytesPerRow + x / 8;
        return offset < glyph.Length && (glyph[offset] & (0x80 >> (x % 8))) != 0;
    }
}

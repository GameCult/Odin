param(
    [string]$Output = "-",
    [string]$Device = "",
    [double]$Seconds = 0,
    [int]$SampleRate = 48000,
    [int]$Channels = 2,
    [ValidateSet("Render", "Capture")]
    [string]$DataFlow = "Render",
    [switch]$Loopback,
    [ValidateSet("Console", "Multimedia", "Communications")]
    [string]$Role = "Console"
)

$ErrorActionPreference = "Stop"

$source = @"
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

namespace Mimir {
    public static class WasapiLoopbackCapture {
        const int eRender = 0;
        const int eCapture = 1;
        const int eConsole = 0;
        const int eMultimedia = 1;
        const int eCommunications = 2;
        const int CLSCTX_ALL = 23;
        const uint DEVICE_STATE_ACTIVE = 0x00000001;
        const uint AUDCLNT_SHAREMODE_SHARED = 0;
        const uint AUDCLNT_STREAMFLAGS_LOOPBACK = 0x00020000;
        const int AUDCLNT_BUFFERFLAGS_SILENT = 0x2;
        static readonly Guid IID_IAudioClient = new Guid("1CB9AD4C-DBFA-4c32-B178-C2F568A703B2");
        static readonly Guid IID_IAudioCaptureClient = new Guid("C8ADBD64-E71E-48a0-A4DE-185C395CD317");
        static readonly Guid PcmSubformat = new Guid("00000001-0000-0010-8000-00aa00389b71");
        static readonly Guid FloatSubformat = new Guid("00000003-0000-0010-8000-00aa00389b71");
        static readonly PROPERTYKEY DeviceFriendlyNameKey = new PROPERTYKEY(
            new Guid("A45C254E-DF1C-4EFD-8020-67D146A850E0"),
            14);

        [StructLayout(LayoutKind.Sequential, Pack = 2)]
        struct WAVEFORMATEX {
            public ushort wFormatTag;
            public ushort nChannels;
            public uint nSamplesPerSec;
            public uint nAvgBytesPerSec;
            public ushort nBlockAlign;
            public ushort wBitsPerSample;
            public ushort cbSize;
        }

        [StructLayout(LayoutKind.Sequential, Pack = 2)]
        struct WAVEFORMATEXTENSIBLE {
            public WAVEFORMATEX Format;
            public ushort wValidBitsPerSample;
            public uint dwChannelMask;
            public Guid SubFormat;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct PROPERTYKEY {
            public Guid fmtid;
            public uint pid;

            public PROPERTYKEY(Guid formatId, uint propertyId) {
                fmtid = formatId;
                pid = propertyId;
            }
        }

        [StructLayout(LayoutKind.Sequential)]
        struct PROPVARIANT {
            public ushort vt;
            public ushort wReserved1;
            public ushort wReserved2;
            public ushort wReserved3;
            public IntPtr pointerValue;
            public int intValue;
        }

        [ComImport]
        [Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
        class MMDeviceEnumerator { }

        [ComImport]
        [Guid("A95664D2-9614-4F35-A746-DE8DB63617E6")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IMMDeviceEnumerator {
            [PreserveSig]
            int EnumAudioEndpoints(int dataFlow, uint stateMask, out IMMDeviceCollection devices);
            [PreserveSig]
            int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice endpoint);
        }

        [ComImport]
        [Guid("0BD7A1BE-7A1A-44DB-8397-CC5392387B5E")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IMMDeviceCollection {
            [PreserveSig]
            int GetCount(out uint count);
            [PreserveSig]
            int Item(uint index, out IMMDevice device);
        }

        [ComImport]
        [Guid("D666063F-1587-4E43-81F1-B948E807363F")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IMMDevice {
            [PreserveSig]
            int Activate(ref Guid iid, int dwClsCtx, IntPtr pActivationParams, out IAudioClient audioClient);
            [PreserveSig]
            int OpenPropertyStore(uint access, out IPropertyStore properties);
        }

        [ComImport]
        [Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IPropertyStore {
            [PreserveSig]
            int GetCount(out uint propertyCount);
            [PreserveSig]
            int GetAt(uint propertyIndex, out PROPERTYKEY key);
            [PreserveSig]
            int GetValue(ref PROPERTYKEY key, out PROPVARIANT value);
            [PreserveSig]
            int SetValue(ref PROPERTYKEY key, ref PROPVARIANT value);
            [PreserveSig]
            int Commit();
        }

        [ComImport]
        [Guid("1CB9AD4C-DBFA-4c32-B178-C2F568A703B2")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IAudioClient {
            [PreserveSig]
            int Initialize(uint shareMode, uint streamFlags, long hnsBufferDuration, long hnsPeriodicity, IntPtr pFormat, IntPtr audioSessionGuid);
            [PreserveSig]
            int GetBufferSize(out uint bufferSize);
            [PreserveSig]
            int GetStreamLatency(out long latency);
            [PreserveSig]
            int GetCurrentPadding(out uint currentPadding);
            [PreserveSig]
            int IsFormatSupported(uint shareMode, IntPtr pFormat, out IntPtr closestMatch);
            [PreserveSig]
            int GetMixFormat(out IntPtr deviceFormat);
            [PreserveSig]
            int GetDevicePeriod(out long defaultDevicePeriod, out long minimumDevicePeriod);
            [PreserveSig]
            int Start();
            [PreserveSig]
            int Stop();
            [PreserveSig]
            int Reset();
            [PreserveSig]
            int SetEventHandle(IntPtr eventHandle);
            [PreserveSig]
            int GetService(ref Guid iid, out IAudioCaptureClient captureClient);
        }

        [ComImport]
        [Guid("C8ADBD64-E71E-48a0-A4DE-185C395CD317")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IAudioCaptureClient {
            [PreserveSig]
            int GetBuffer(out IntPtr data, out uint frames, out uint flags, out ulong devicePosition, out ulong qpcPosition);
            [PreserveSig]
            int ReleaseBuffer(uint frames);
            [PreserveSig]
            int GetNextPacketSize(out uint frames);
        }

        [DllImport("ole32.dll")]
        static extern int PropVariantClear(ref PROPVARIANT variant);

        public static void Capture(string outputPath, string deviceSubstring, double seconds, int targetRate, int targetChannels, string dataFlowName, bool loopback, string roleName) {
            Stream output = IsStdoutPath(outputPath) ? Console.OpenStandardOutput() : new FileStream(outputPath, FileMode.Create, FileAccess.Write, FileShare.Read);
            try {
                IMMDeviceEnumerator enumerator = (IMMDeviceEnumerator)(new MMDeviceEnumerator());
                int dataFlow = DataFlowFromName(dataFlowName);
                uint streamFlags = loopback ? AUDCLNT_STREAMFLAGS_LOOPBACK : 0u;
                IMMDevice device = ResolveDevice(enumerator, dataFlow, roleName, deviceSubstring);
                try {
                    IAudioClient audioClient;
                    Guid audioClientId = IID_IAudioClient;
                    Check(device.Activate(ref audioClientId, CLSCTX_ALL, IntPtr.Zero, out audioClient), "Activate IAudioClient");
                    IntPtr mixFormatPtr;
                    Check(audioClient.GetMixFormat(out mixFormatPtr), "GetMixFormat");
                    WAVEFORMATEX fmt = Marshal.PtrToStructure<WAVEFORMATEX>(mixFormatPtr);
                    Console.Error.WriteLine("MixFormat tag={0} channels={1} rate={2} bits={3} blockAlign={4} cbSize={5}", fmt.wFormatTag, fmt.nChannels, fmt.nSamplesPerSec, fmt.wBitsPerSample, fmt.nBlockAlign, fmt.cbSize);
                    IntPtr activeFormatPtr = mixFormatPtr;
                    IntPtr fallbackFormatPtr = IntPtr.Zero;
                    IntPtr closest = IntPtr.Zero;
                    int support = audioClient.IsFormatSupported(AUDCLNT_SHAREMODE_SHARED, activeFormatPtr, out closest);
                    Console.Error.WriteLine("MixFormatSupport hr=0x{0:X8} closest={1}", support, closest != IntPtr.Zero);
                    if (support != 0 && closest != IntPtr.Zero) {
                        activeFormatPtr = closest;
                        fmt = Marshal.PtrToStructure<WAVEFORMATEX>(activeFormatPtr);
                        Console.Error.WriteLine("ClosestFormat tag={0} channels={1} rate={2} bits={3} blockAlign={4} cbSize={5}", fmt.wFormatTag, fmt.nChannels, fmt.nSamplesPerSec, fmt.wBitsPerSample, fmt.nBlockAlign, fmt.cbSize);
                    }
                    int init = audioClient.Initialize(AUDCLNT_SHAREMODE_SHARED, streamFlags, 0, 0, activeFormatPtr, IntPtr.Zero);
                    if (init < 0) {
                        fallbackFormatPtr = MakeWaveFormatPcm((ushort)targetChannels, (uint)targetRate, 16);
                        activeFormatPtr = fallbackFormatPtr;
                        fmt = Marshal.PtrToStructure<WAVEFORMATEX>(activeFormatPtr);
                        Console.Error.WriteLine("RetryFormat tag={0} channels={1} rate={2} bits={3} blockAlign={4} cbSize={5}", fmt.wFormatTag, fmt.nChannels, fmt.nSamplesPerSec, fmt.wBitsPerSample, fmt.nBlockAlign, fmt.cbSize);
                        IntPtr retryClosest = IntPtr.Zero;
                        int retrySupport = audioClient.IsFormatSupported(AUDCLNT_SHAREMODE_SHARED, activeFormatPtr, out retryClosest);
                        Console.Error.WriteLine("RetryFormatSupport hr=0x{0:X8} closest={1}", retrySupport, retryClosest != IntPtr.Zero);
                        if (retrySupport != 0 && retryClosest != IntPtr.Zero) {
                            Marshal.FreeHGlobal(fallbackFormatPtr);
                            fallbackFormatPtr = IntPtr.Zero;
                            activeFormatPtr = retryClosest;
                            fmt = Marshal.PtrToStructure<WAVEFORMATEX>(activeFormatPtr);
                            Console.Error.WriteLine("RetryClosestFormat tag={0} channels={1} rate={2} bits={3} blockAlign={4} cbSize={5}", fmt.wFormatTag, fmt.nChannels, fmt.nSamplesPerSec, fmt.wBitsPerSample, fmt.nBlockAlign, fmt.cbSize);
                        }
                        init = audioClient.Initialize(AUDCLNT_SHAREMODE_SHARED, streamFlags, 0, 0, activeFormatPtr, IntPtr.Zero);
                    }
                    Check(init, loopback ? "Initialize loopback" : "Initialize capture");
                    IAudioCaptureClient captureClient;
                    Guid captureClientId = IID_IAudioCaptureClient;
                    Check(audioClient.GetService(ref captureClientId, out captureClient), "GetService IAudioCaptureClient");
                    Check(audioClient.Start(), "Start");
                    DateTime end = seconds > 0 ? DateTime.UtcNow.AddSeconds(seconds) : DateTime.MaxValue;
                    byte[] pcm = new byte[8192];
                    byte[] silence = new byte[0];
                    int sourceRate = Math.Max(1, (int)fmt.nSamplesPerSec);
                    double sourceFramesPerTargetFrame = sourceRate / (double)Math.Max(1, targetRate);
                    long sourceFramesSeen = 0;
                    double nextOutputSourceFrame = 0.0;
                    float[] previousFrame = new float[Math.Max(1, targetChannels)];
                    bool hasPreviousFrame = false;
                    int idleFramesPerChunk = Math.Max(1, (int)Math.Round(Math.Max(1, targetRate) * 0.005));
                    int idleBytes = checked(idleFramesPerChunk * Math.Max(1, targetChannels) * 4);
                    try {
                        while (DateTime.UtcNow < end) {
                            uint packetFrames;
                            Check(captureClient.GetNextPacketSize(out packetFrames), "GetNextPacketSize");
                            if (packetFrames == 0) {
                                Thread.Sleep(5);
                                continue;
                            }
                            IntPtr data;
                            uint frames;
                            uint flags;
                            ulong devicePosition;
                            ulong qpcPosition;
                            Check(captureClient.GetBuffer(out data, out frames, out flags, out devicePosition, out qpcPosition), "GetBuffer");
                            try {
                                int sourceChannels = Math.Max(1, (int)fmt.nChannels);
                                int sourceBytesPerFrame = (int)fmt.nBlockAlign;
                                int sourceBits = (int)fmt.wBitsPerSample;
                                int sourceBytesPerSample = Math.Max(1, sourceBits / 8);
                                int frameBytes = targetChannels * 4;
                                long packetStart = sourceFramesSeen;
                                long packetEnd = packetStart + frames;
                                int outputFrames = 0;
                                double countCursor = nextOutputSourceFrame;
                                while (countCursor < packetEnd) {
                                    outputFrames++;
                                    countCursor += sourceFramesPerTargetFrame;
                                }
                                int needed = checked(outputFrames * frameBytes);
                                if (pcm.Length < needed) pcm = new byte[needed];
                                int dst = 0;
                                float[] left = new float[targetChannels];
                                float[] right = new float[targetChannels];
                                while (nextOutputSourceFrame < packetEnd) {
                                    long leftAbsolute = (long)Math.Floor(nextOutputSourceFrame);
                                    long rightAbsolute = Math.Min(packetEnd - 1, leftAbsolute + 1);
                                    double fraction = nextOutputSourceFrame - leftAbsolute;
                                    ReadFrame(leftAbsolute, packetStart, data, flags, sourceChannels, sourceBytesPerFrame, sourceBytesPerSample, sourceBits, fmt.wFormatTag, activeFormatPtr, previousFrame, hasPreviousFrame, left);
                                    ReadFrame(rightAbsolute, packetStart, data, flags, sourceChannels, sourceBytesPerFrame, sourceBytesPerSample, sourceBits, fmt.wFormatTag, activeFormatPtr, previousFrame, hasPreviousFrame, right);
                                    for (int ch = 0; ch < targetChannels; ch++) {
                                        float sample = (float)(left[ch] + (right[ch] - left[ch]) * fraction);
                                        byte[] bytes = BitConverter.GetBytes(sample);
                                        Buffer.BlockCopy(bytes, 0, pcm, dst, 4);
                                        dst += 4;
                                    }
                                    nextOutputSourceFrame += sourceFramesPerTargetFrame;
                                }
                                if (frames > 0) {
                                    ReadFrame(packetEnd - 1, packetStart, data, flags, sourceChannels, sourceBytesPerFrame, sourceBytesPerSample, sourceBits, fmt.wFormatTag, activeFormatPtr, previousFrame, false, previousFrame);
                                    hasPreviousFrame = true;
                                }
                                output.Write(pcm, 0, needed);
                                output.Flush();
                                sourceFramesSeen = packetEnd;
                            }
                            finally {
                                Check(captureClient.ReleaseBuffer(frames), "ReleaseBuffer");
                            }
                        }
                    }
                    finally {
                        audioClient.Stop();
                        Marshal.FreeCoTaskMem(mixFormatPtr);
                        if (fallbackFormatPtr != IntPtr.Zero) Marshal.FreeHGlobal(fallbackFormatPtr);
                        if (closest != IntPtr.Zero) Marshal.FreeCoTaskMem(closest);
                        Marshal.ReleaseComObject(captureClient);
                        Marshal.ReleaseComObject(audioClient);
                    }
                }
                finally {
                    Marshal.ReleaseComObject(device);
                    Marshal.ReleaseComObject(enumerator);
                }
            }
            finally {
                if (!IsStdoutPath(outputPath)) output.Dispose();
            }
        }

        static IMMDevice ResolveDevice(IMMDeviceEnumerator enumerator, int dataFlow, string roleName, string deviceSubstring) {
            if (String.IsNullOrWhiteSpace(deviceSubstring)) {
                IMMDevice defaultDevice;
                Check(enumerator.GetDefaultAudioEndpoint(dataFlow, RoleFromName(roleName), out defaultDevice), "GetDefaultAudioEndpoint");
                Console.Error.WriteLine("SelectedAudioDevice dataFlow={0} default role={1} name=\"{2}\"", dataFlow == eCapture ? "capture" : "render", roleName, DeviceFriendlyName(defaultDevice));
                return defaultDevice;
            }

            IMMDeviceCollection devices;
            Check(enumerator.EnumAudioEndpoints(dataFlow, DEVICE_STATE_ACTIVE, out devices), "EnumAudioEndpoints");
            try {
                uint count;
                Check(devices.GetCount(out count), "GetCount");
                for (uint index = 0; index < count; index++) {
                    IMMDevice device;
                    Check(devices.Item(index, out device), "Item");
                    string friendlyName = "";
                    try {
                        friendlyName = DeviceFriendlyName(device);
                        if (friendlyName.IndexOf(deviceSubstring, StringComparison.OrdinalIgnoreCase) >= 0) {
                            Console.Error.WriteLine("SelectedAudioDevice dataFlow={0} match=\"{1}\" name=\"{2}\"", dataFlow == eCapture ? "capture" : "render", deviceSubstring, friendlyName);
                            return device;
                        }
                    }
                    catch {
                        Marshal.ReleaseComObject(device);
                        throw;
                    }
                    Marshal.ReleaseComObject(device);
                }
            }
            finally {
                Marshal.ReleaseComObject(devices);
            }

            throw new InvalidOperationException("No active " + (dataFlow == eCapture ? "capture" : "render") + " endpoint matched --device \"" + deviceSubstring + "\"");
        }

        static string DeviceFriendlyName(IMMDevice device) {
            IPropertyStore store;
            Check(device.OpenPropertyStore(0, out store), "OpenPropertyStore");
            try {
                PROPERTYKEY key = DeviceFriendlyNameKey;
                PROPVARIANT value;
                Check(store.GetValue(ref key, out value), "GetValue FriendlyName");
                try {
                    if (value.vt == 31 && value.pointerValue != IntPtr.Zero) {
                        return Marshal.PtrToStringUni(value.pointerValue) ?? "";
                    }
                    return "";
                }
                finally {
                    PropVariantClear(ref value);
                }
            }
            finally {
                Marshal.ReleaseComObject(store);
            }
        }

        static bool IsStdoutPath(string outputPath) {
            return String.Equals(outputPath, "-", StringComparison.Ordinal) ||
                String.Equals(outputPath, "stdout", StringComparison.OrdinalIgnoreCase);
        }

        static void ReadFrame(long absoluteFrame, long packetStart, IntPtr data, uint flags, int sourceChannels, int sourceBytesPerFrame, int sourceBytesPerSample, int sourceBits, ushort tag, IntPtr formatPtr, float[] previousFrame, bool hasPreviousFrame, float[] target) {
            if (absoluteFrame < packetStart) {
                for (int ch = 0; ch < target.Length; ch++) target[ch] = hasPreviousFrame ? previousFrame[ch] : 0f;
                return;
            }

            int packetFrame = checked((int)(absoluteFrame - packetStart));
            for (int ch = 0; ch < target.Length; ch++) {
                float sample = 0f;
                if ((flags & AUDCLNT_BUFFERFLAGS_SILENT) == 0) {
                    int srcCh = Math.Min(ch, sourceChannels - 1);
                    IntPtr src = IntPtr.Add(data, packetFrame * sourceBytesPerFrame + srcCh * sourceBytesPerSample);
                    sample = ReadSample(src, sourceBits, tag, formatPtr);
                }
                target[ch] = sample;
            }
        }

        static void Check(int hr, string stage) {
            if (hr < 0) {
                Exception inner = Marshal.GetExceptionForHR(hr);
                throw new InvalidOperationException(stage + " failed: 0x" + hr.ToString("X8") + " " + (inner == null ? "" : inner.Message), inner);
            }
        }

        static int RoleFromName(string roleName) {
            if (String.Equals(roleName, "Multimedia", StringComparison.OrdinalIgnoreCase)) return eMultimedia;
            if (String.Equals(roleName, "Communications", StringComparison.OrdinalIgnoreCase)) return eCommunications;
            return eConsole;
        }

        static int DataFlowFromName(string dataFlowName) {
            if (String.Equals(dataFlowName, "Capture", StringComparison.OrdinalIgnoreCase)) return eCapture;
            return eRender;
        }

        static IntPtr MakeWaveFormatPcm(ushort channels, uint sampleRate, ushort bits) {
            WAVEFORMATEX fmt = new WAVEFORMATEX();
            fmt.wFormatTag = 1;
            fmt.nChannels = channels;
            fmt.nSamplesPerSec = sampleRate;
            fmt.wBitsPerSample = bits;
            fmt.nBlockAlign = (ushort)(channels * bits / 8);
            fmt.nAvgBytesPerSec = sampleRate * fmt.nBlockAlign;
            fmt.cbSize = 0;
            IntPtr ptr = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(WAVEFORMATEX)));
            Marshal.StructureToPtr(fmt, ptr, false);
            return ptr;
        }

        static float ReadSample(IntPtr source, int bits, ushort tag, IntPtr formatPtr) {
            bool isFloat = tag == 3;
            if (tag == 65534) {
                WAVEFORMATEXTENSIBLE ext = Marshal.PtrToStructure<WAVEFORMATEXTENSIBLE>(formatPtr);
                isFloat = ext.SubFormat == FloatSubformat;
            }
            if (isFloat && bits == 32) {
                return Math.Max(-1f, Math.Min(1f, (float)Marshal.PtrToStructure(source, typeof(float))));
            }
            if (bits == 16) {
                return Marshal.ReadInt16(source) / 32768f;
            }
            if (bits == 24) {
                int b0 = Marshal.ReadByte(source, 0);
                int b1 = Marshal.ReadByte(source, 1);
                int b2 = Marshal.ReadByte(source, 2);
                int value = b0 | (b1 << 8) | (b2 << 16);
                if ((value & 0x800000) != 0) value |= unchecked((int)0xff000000);
                return Math.Max(-1f, Math.Min(1f, value / 8388608f));
            }
            if (bits == 32) {
                if (tag == 65534) {
                    WAVEFORMATEXTENSIBLE ext = Marshal.PtrToStructure<WAVEFORMATEXTENSIBLE>(formatPtr);
                    if (ext.SubFormat == PcmSubformat && ext.wValidBitsPerSample > 0 && ext.wValidBitsPerSample < 32) {
                        int value = Marshal.ReadInt32(source);
                        int shifted = value >> (32 - ext.wValidBitsPerSample);
                        return Math.Max(-1f, Math.Min(1f, shifted / (float)(1 << (ext.wValidBitsPerSample - 1))));
                    }
                }
                return Math.Max(-1f, Math.Min(1f, Marshal.ReadInt32(source) / 2147483648f));
            }
            return 0f;
        }
    }
}
"@

Add-Type -TypeDefinition $source
$effectiveOutput = if ($Output -eq "stdout") { "-" } else { $Output }
[Mimir.WasapiLoopbackCapture]::Capture($effectiveOutput, $Device, $Seconds, $SampleRate, $Channels, $DataFlow, $Loopback, $Role)

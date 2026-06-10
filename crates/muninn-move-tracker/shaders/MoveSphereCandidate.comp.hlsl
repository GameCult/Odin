// Muninn PS Move sphere candidate extraction.
// One thread group owns one image tile. The shader emits at most one bright
// marker candidate per 16px tile; Rust owns dispatch sizing and downstream ranking.

struct MoveSphereCandidate
{
    uint2 sourceIdHash;
    uint2 frameSequence;
    uint tileX;
    uint tileY;
    float centerXPx;
    float centerYPx;
    float radiusPx;
    uint areaPx;
    float meanLuma;
    uint peakLuma;
    float score;
};

cbuffer MoveTrackerConfig : register(b0)
{
    uint width;
    uint height;
    uint strideBytes;
    uint tileSize;
    uint thresholdMin;
    uint minAreaPx;
    uint maxCandidates;
    uint _pad0;
    uint2 sourceIdHash;
    uint2 frameSequence;
};

ByteAddressBuffer frameY8 : register(t0);
RWStructuredBuffer<MoveSphereCandidate> candidates : register(u0);
RWByteAddressBuffer candidateCounter : register(u1);

groupshared uint gArea;
groupshared uint gSumX;
groupshared uint gSumY;
groupshared uint gSumW;
groupshared uint gPeak;

[numthreads(16, 16, 1)]
void main(uint3 groupId : SV_GroupID, uint3 groupThreadId : SV_GroupThreadID)
{
    if (groupThreadId.x == 0 && groupThreadId.y == 0)
    {
        gArea = 0;
        gSumX = 0;
        gSumY = 0;
        gSumW = 0;
        gPeak = 0;
    }
    GroupMemoryBarrierWithGroupSync();

    uint x = groupId.x * tileSize + groupThreadId.x;
    uint y = groupId.y * tileSize + groupThreadId.y;
    if (groupThreadId.x < tileSize && groupThreadId.y < tileSize && x < width && y < height)
    {
        uint byteOffset = y * strideBytes + x;
        uint packed = frameY8.Load(byteOffset & ~3u);
        uint shift = (byteOffset & 3u) * 8u;
        uint luma = (packed >> shift) & 0xffu;
        if (luma >= thresholdMin)
        {
            uint weight = luma - thresholdMin + 1u;
            InterlockedAdd(gArea, 1u);
            InterlockedAdd(gSumX, x * weight);
            InterlockedAdd(gSumY, y * weight);
            InterlockedAdd(gSumW, weight);
            InterlockedMax(gPeak, luma);
        }
    }

    GroupMemoryBarrierWithGroupSync();

    if (groupThreadId.x == 0 && groupThreadId.y == 0 && gArea >= minAreaPx && gSumW > 0)
    {
        uint index;
        candidateCounter.InterlockedAdd(0, 1, index);
        if (index < maxCandidates)
        {
            float area = (float)gArea;
            float tilePixels = (float)(tileSize * tileSize);
            float areaScore = min(1.0, area / tilePixels);
            float brightnessScore = (float)gPeak / 255.0;

            MoveSphereCandidate candidate;
            candidate.sourceIdHash = sourceIdHash;
            candidate.frameSequence = frameSequence;
            candidate.tileX = groupId.x;
            candidate.tileY = groupId.y;
            candidate.centerXPx = (float)gSumX / (float)gSumW;
            candidate.centerYPx = (float)gSumY / (float)gSumW;
            candidate.radiusPx = sqrt(area / 3.14159265358979323846);
            candidate.areaPx = gArea;
            candidate.meanLuma = (float)thresholdMin + (((float)gSumW / area) - 1.0);
            candidate.peakLuma = gPeak;
            candidate.score = brightnessScore * (0.65 + areaScore * 0.35);
            candidates[index] = candidate;
        }
    }
}

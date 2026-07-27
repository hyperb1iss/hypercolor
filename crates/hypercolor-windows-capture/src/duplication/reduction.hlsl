Texture2D<float4> Desktop : register(t0);
Texture2D<uint4> Pointer : register(t1);
RWTexture2D<float4> Target : register(u0);

cbuffer ReductionParams : register(b0)
{
    uint SourceWidth;
    uint SourceHeight;
    uint OutputWidth;
    uint OutputHeight;
    uint Stride;
    uint Rotation;
    uint PointerKind;
    uint PointerVisible;
    int PointerX;
    int PointerY;
    uint PointerWidth;
    uint PointerHeight;
};

int2 logical_to_scanout(int2 logical)
{
    if (Rotation == 1) {
        return int2(logical.y, int(SourceHeight) - 1 - logical.x);
    }
    if (Rotation == 2) {
        return int2(int(SourceWidth) - 1 - logical.x,
                    int(SourceHeight) - 1 - logical.y);
    }
    if (Rotation == 3) {
        return int2(int(SourceWidth) - 1 - logical.y, logical.x);
    }
    return logical;
}

[numthreads(8, 8, 1)]
void composite_cursor(uint3 thread_id : SV_DispatchThreadID)
{
    if (PointerVisible == 0 || thread_id.x >= PointerWidth ||
        thread_id.y >= PointerHeight) {
        return;
    }

    int2 scanout = logical_to_scanout(
        int2(PointerX + int(thread_id.x), PointerY + int(thread_id.y)));
    if (scanout.x < 0 || scanout.y < 0 ||
        scanout.x >= int(SourceWidth) || scanout.y >= int(SourceHeight)) {
        return;
    }

    uint4 desktop = uint4(round(saturate(Desktop.Load(int3(scanout, 0))) * 255.0));
    uint4 pointer = Pointer.Load(int3(thread_id.xy, 0));
    uint3 color;

    if (PointerKind == 0) {
        uint alpha = pointer.a;
        color = (pointer.rgb * alpha + desktop.rgb * (255 - alpha) + 127) / 255;
    } else if (PointerKind == 1) {
        color = pointer.a == 0 ? pointer.rgb : desktop.rgb ^ pointer.rgb;
    } else {
        uint and_mask = pointer.r;
        uint xor_mask = pointer.g;
        color = (desktop.rgb & and_mask) ^ xor_mask;
    }

    Target[scanout] = float4(float3(color) / 255.0, 1.0);
}

[numthreads(8, 8, 1)]
void reduce_desktop(uint3 thread_id : SV_DispatchThreadID)
{
    if (thread_id.x >= OutputWidth || thread_id.y >= OutputHeight) {
        return;
    }

    uint2 begin = thread_id.xy * Stride;
    uint2 end = min(begin + Stride, uint2(SourceWidth, SourceHeight));
    float3 sum = 0.0;
    uint samples = 0;
    for (uint y = begin.y; y < end.y; ++y) {
        for (uint x = begin.x; x < end.x; ++x) {
            sum += Desktop.Load(int3(x, y, 0)).rgb;
            ++samples;
        }
    }

    Target[thread_id.xy] = float4(sum / max(samples, 1), 1.0);
}

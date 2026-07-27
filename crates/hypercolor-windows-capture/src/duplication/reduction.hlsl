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
    uint RegionX;
    uint RegionY;
    uint RegionWidth;
    uint RegionHeight;
};

int2 scanout_to_logical(int2 scanout)
{
    if (Rotation == 1) {
        return int2(int(SourceHeight) - 1 - scanout.y, scanout.x);
    }
    if (Rotation == 2) {
        return int2(int(SourceWidth) - 1 - scanout.x,
                    int(SourceHeight) - 1 - scanout.y);
    }
    if (Rotation == 3) {
        return int2(scanout.y, int(SourceWidth) - 1 - scanout.x);
    }
    return scanout;
}

float4 compose_cursor(uint2 scanout, float4 desktop_sample)
{
    if (PointerVisible == 0) {
        return desktop_sample;
    }

    int2 logical = scanout_to_logical(int2(scanout));
    int2 pointer_position = logical - int2(PointerX, PointerY);
    if (pointer_position.x < 0 || pointer_position.y < 0 ||
        pointer_position.x >= int(PointerWidth) ||
        pointer_position.y >= int(PointerHeight)) {
        return desktop_sample;
    }

    uint4 desktop = uint4(round(saturate(desktop_sample) * 255.0));
    uint4 pointer = Pointer.Load(int3(pointer_position, 0));
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

    return float4(float3(color) / 255.0, 1.0);
}

[numthreads(8, 8, 1)]
void reduce_desktop(uint3 thread_id : SV_DispatchThreadID)
{
    if (thread_id.x >= OutputWidth || thread_id.y >= OutputHeight) {
        return;
    }

    uint2 region_begin = uint2(RegionX, RegionY);
    uint2 region_end = region_begin + uint2(RegionWidth, RegionHeight);
    uint2 begin = region_begin + thread_id.xy * Stride;
    uint2 end = min(begin + Stride, region_end);
    float3 sum = 0.0;
    uint samples = 0;
    for (uint y = begin.y; y < end.y; ++y) {
        for (uint x = begin.x; x < end.x; ++x) {
            float4 desktop = Desktop.Load(int3(x, y, 0));
            sum += compose_cursor(uint2(x, y), desktop).rgb;
            ++samples;
        }
    }

    Target[thread_id.xy] = float4(sum / max(samples, 1), 1.0);
}

#include "common.h"

// Диагностический узор: compute пишет текстуру, scene читает её во fragment.
kernel void pattern_main(texture2d<float, access::write> out [[texture(0)]],
                         constant FrameUniforms& frame [[buffer(0)]],
                         uint2 gid [[thread_position_in_grid]]) {
    uint w = out.get_width();
    uint h = out.get_height();
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    out.write(float4(pattern_color(gid, w, frame.pattern_time), 1.0), gid);
}

// Записывает размеры и смещения общих структур для сверки с Rust.
// Порядок совпадает с layout::expected_probe.
kernel void layout_probe(device uint* out [[buffer(0)]],
                         uint gid [[thread_position_in_grid]]) {
    if (gid != 0) {
        return;
    }
    FrameUniforms f;
    InstanceData i;
    Vertex v;
    thread char* fb = (thread char*)&f;
    thread char* ib = (thread char*)&i;
    thread char* vb = (thread char*)&v;
    out[0] = sizeof(FrameUniforms);
    out[1] = alignof(FrameUniforms);
    out[2] = uint((thread char*)&f.camera_pos - fb);
    out[3] = uint((thread char*)&f.light_dir - fb);
    out[4] = uint((thread char*)&f.time - fb);
    out[5] = uint((thread char*)&f.exposure - fb);
    out[6] = uint((thread char*)&f.pattern_time - fb);
    out[7] = uint((thread char*)&f.frame_index - fb);
    out[8] = sizeof(InstanceData);
    out[9] = uint((thread char*)&i.tint - ib);
    out[10] = sizeof(Vertex);
    out[11] = uint((thread char*)&v.normal - vb);
    out[12] = uint((thread char*)&v.uv - vb);
}

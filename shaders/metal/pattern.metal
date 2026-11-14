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
    TerrainVertex tv;
    thread char* tb = (thread char*)&tv;
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
    out[8] = uint((thread char*)&f.view_mode - fb);
    out[9] = uint((thread char*)&f.map_size_m - fb);
    out[10] = uint((thread char*)&f.underwater_m - fb);
    out[11] = uint((thread char*)&f.view_proj_rel - fb);
    out[12] = uint((thread char*)&f.cam_lo - fb);
    out[13] = sizeof(InstanceData);
    out[14] = uint((thread char*)&i.tint - ib);
    out[15] = sizeof(Vertex);
    out[16] = uint((thread char*)&v.normal - vb);
    out[17] = uint((thread char*)&v.uv - vb);
    out[18] = sizeof(TerrainVertex);
    out[19] = uint((thread char*)&tv.normal - tb);
    out[20] = uint((thread char*)&tv.weights - tb);
    out[21] = uint((thread char*)&tv.material - tb);
    SeaParams sp;
    thread char* sb = (thread char*)&sp;
    out[22] = sizeof(GerstnerWave);
    out[23] = sizeof(SeaParams);
    out[24] = uint((thread char*)&sp.waves - sb);
    out[25] = uint((thread char*)&sp.probe - sb);
    out[26] = sizeof(WaterLevel);
    SkinnedVertex sv;
    HandsUniforms hu;
    thread char* svb = (thread char*)&sv;
    thread char* hub = (thread char*)&hu;
    out[27] = sizeof(SkinnedVertex);
    out[28] = uint((thread char*)&sv.normal - svb);
    out[29] = uint((thread char*)&sv.uv - svb);
    out[30] = uint((thread char*)&sv.joints - svb);
    out[31] = uint((thread char*)&sv.weights - svb);
    out[32] = sizeof(HandsUniforms);
    out[33] = uint((thread char*)&hu.vertex_count - hub);
    FeatureSegment fs;
    thread char* fsb = (thread char*)&fs;
    out[34] = sizeof(FeatureSegment);
    out[35] = uint((thread char*)&fs.half_width - fsb);
    out[36] = uint((thread char*)&fs.kind - fsb);
    TerrainCluster tc;
    out[37] = sizeof(TerrainCluster);
    out[38] = uint((thread char*)&tc.depth - (thread char*)&tc);
    SkyInputs si;
    thread char* sib = (thread char*)&si;
    out[39] = sizeof(SkyInputs);
    out[40] = uint((thread char*)&si.params - sib);
    out[41] = uint((thread char*)&si.inv_view_proj_rel - sib);
    out[42] = sizeof(TerrainZone);
}

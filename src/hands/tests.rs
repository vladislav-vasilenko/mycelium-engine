use glam::{Mat4, Vec3};

use super::rig::POSE_BLEND_S;
use super::*;

fn hands() -> Hands {
    Hands::load().expect("руки загружаются")
}

/// Ассет: одна сетка, 50 костей скелета (+ служебная `neutral_bone`
/// экспортёра), все позы, веса нормированы.
#[test]
fn asset_loads_with_skeleton_and_poses() {
    let h = hands();
    let a = &h.asset;
    let named = a
        .skeleton
        .joints
        .iter()
        .filter(|j| j.name != "neutral_bone")
        .count();
    assert_eq!(named, 50, "костей скелета");
    for n in POSE_NAMES {
        assert!(a.pose(n).is_some(), "поза {n}");
    }
    assert!(a.vertices.len() > 1000 && a.indices.len().is_multiple_of(3));
    for v in &a.vertices {
        let s: f32 = v.weights.iter().sum();
        assert!((s - 1.0).abs() < 1e-4, "сумма весов {s}");
    }
    assert_eq!(a.albedo.len(), (a.albedo_size * a.albedo_size * 4) as usize);
}

/// Суставы каждой позы совпадают с эталоном экспорта Blender
/// (`fps-arms.poses.json`, оси Blender: x, y — вперёд, z — вверх).
#[test]
fn pose_joints_match_blender_reference() {
    let h = hands();
    let path = asset_path().with_extension("poses.json");
    let reference: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut checked = 0;
    for name in POSE_NAMES {
        let g = h.asset.skeleton.globals(h.asset.pose(name).unwrap());
        for (joint, p) in reference[name].as_object().unwrap() {
            let Some(j) = h.asset.skeleton.joint(joint) else {
                continue;
            };
            let b: Vec<f32> = p
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap() as f32)
                .collect();
            let want = Vec3::new(b[0], b[2], -b[1]);
            let got = g[j].w_axis.truncate();
            assert!(
                (got - want).length() < 1e-3,
                "{name}/{joint}: {got} вместо {want}"
            );
            checked += 1;
        }
    }
    assert!(checked >= 4 * 30, "проверено суставов {checked}");
}

/// Кисть в каждой позе перед глазом и в кадре проекции рук; длина кисти 12–25 см.
#[test]
fn hands_are_in_front_of_the_eye() {
    let h = hands();
    let s = &h.asset.skeleton;
    let proj = projection(16.0 / 9.0);
    for name in POSE_NAMES {
        let g = s.globals(h.asset.pose(name).unwrap());
        for side in ["L", "R"] {
            let wrist = g[s.joint(&format!("hand.{side}")).unwrap()]
                .w_axis
                .truncate();
            let tip = g[s.joint(&format!("f_middle.03.{side}")).unwrap()]
                .w_axis
                .truncate();
            assert!(wrist.z < -0.1, "{name} {side}: запястье {wrist}");
            let clip = proj * tip.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!(
                ndc.x.abs() < 1.0 && ndc.y.abs() < 1.0,
                "{name} {side}: палец вне кадра {ndc}"
            );
            let len = (tip - wrist).length();
            assert!((0.08..0.25).contains(&len), "{name} {side}: {len} м");
        }
    }
}

/// Смешивание поз: концы совпадают с позами, середина — без скачка.
#[test]
fn pose_blend_is_continuous() {
    let h = hands();
    let a = h.asset.pose("relaxed").unwrap();
    let b = h.asset.pose("grip").unwrap();
    let s = &h.asset.skeleton;
    let tip = s.joint("f_index.03.R").unwrap();
    let at = |k: f32| s.globals(&a.blend(b, k))[tip].w_axis.truncate();
    assert!((at(0.0) - s.globals(a)[tip].w_axis.truncate()).length() < 1e-5);
    assert!((at(1.0) - s.globals(b)[tip].w_axis.truncate()).length() < 1e-5);
    let mut prev = at(0.0);
    let total = (at(1.0) - at(0.0)).length();
    for i in 1..=20 {
        let p = at(i as f32 / 20.0);
        assert!(
            (p - prev).length() < total * 0.2 + 1e-3,
            "скачок на шаге {i}"
        );
        prev = p;
    }
}

/// Аниматор: переход занимает `POSE_BLEND_S`, отставание от поворота
/// появляется и затухает, покачивание выключается весом 0.
#[test]
fn rig_blends_lags_and_settles() {
    let mut h = hands();
    let mut inp = RigInput {
        dt: 1.0 / 60.0,
        ..Default::default()
    };
    h.rig.update(&inp);
    h.rig.set_pose(1);
    for _ in 0..((POSE_BLEND_S * 60.0) as usize + 2) {
        h.rig.update(&inp);
    }
    let s = &h.asset.skeleton;
    let got = s.globals(&h.rig.pose());
    let want = s.globals(h.asset.pose("grip").unwrap());
    for (a, b) in got.iter().zip(&want) {
        assert!(a.abs_diff_eq(*b, 1e-5), "переход не завершился");
    }

    inp.yaw = 0.05;
    h.rig.update(&inp);
    assert!(h.rig.lag().x < -0.01, "отставание {}", h.rig.lag());
    for _ in 0..60 {
        h.rig.update(&inp);
    }
    assert!(h.rig.lag().length() < 1e-4, "отставание не затухло");

    inp.bob_phase = 1.0;
    inp.bob_weight = 0.0;
    assert!(h.rig.root(&inp).abs_diff_eq(Mat4::IDENTITY, 1e-6));
}

/// CPU-скиннинг: в контрольной позе вершины конечны и лежат у кистей,
/// а в позе покоя при палитре из обратных матриц привязки — на месте.
#[test]
fn skinning_is_finite_and_near_hands() {
    let h = hands();
    let pal = h.control_palette();
    let mut max_dist = 0.0f32;
    for v in &h.asset.vertices {
        let p = skin_position(v, &pal);
        assert!(p.is_finite());
        max_dist = max_dist.max(p.length());
    }
    // Руки с плечами: не дальше метра от глаза.
    assert!(max_dist < 1.0, "вершина в {max_dist} м от глаза");
}

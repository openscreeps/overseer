//! Frames drawn through wgpu's no-op backend: pipelines, shaders and buffer validation.

use glam::Vec2;
use overseer::map::{RoomCoord, RoomRect, RoomStatus};
use serde_json::json;

use super::{
    ME, creep, energy, minimap, object, place, room, shard, snapshot_of, stat, structure, terrain,
    visuals,
};
use crate::gpu::{Draw, Gpu};
use crate::text::Atlas;
use crate::view::View;
use crate::world::Limits;
use crate::{DETAIL_PX_PER_ROOM, Scope, ViewMode};

/// Runs a future that completes without waiting (the no-op backend's).
fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}

#[test]
fn frames_render_through_the_noop_backend() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: wgpu::BackendOptions {
            noop: wgpu::NoopBackendOptions {
                enable: true,
                ..wgpu::NoopBackendOptions::default()
            },
            ..wgpu::BackendOptions::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = ready(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = ready(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
        ..wgpu::DeviceDescriptor::default()
    }))
    .unwrap();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let output = device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 320,
                height: 200,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let limits = Limits::of(&device.limits());
    assert_eq!(limits, Limits::WEBGL2);
    for samples in [1, 4] {
        let mut gpu = Gpu::new(device.clone(), queue.clone(), format, samples);
        gpu.resize(320, 200);
        let mut view = View::new(limits);
        view.set_scope(Scope::World(shard()));
        view.resize(320.0, 200.0);
        view.world.set_user(Some(ME.to_owned()));
        let objects = (0..300_u16)
            .map(|index| {
                let (x, y) = (
                    u8::try_from(index % 40).unwrap() + 5,
                    u8::try_from(index / 40).unwrap(),
                );
                match index % 4 {
                    0 => creep(&format!("c{index}"), x, y, ME),
                    1 => structure("road", &format!("r{index}"), x, y),
                    2 => structure("rampart", &format!("p{index}"), x, y),
                    _ => energy(&format!("e{index}"), x, y),
                }
            })
            .collect::<Vec<_>>();
        let terrain = terrain();
        for name in ["W1N1", "W0N1", "W1N0", "W0N0"] {
            let room = room(name);
            view.world.set_terrain(room, &terrain);
            view.world
                .set_stat(room, Some(&stat(RoomStatus::Normal, Some((ME, 8)))));
            view.world.set_minimap(room, Some(&minimap(ME)));
        }
        view.world
            .set_room(&snapshot_of("W1N1", &terrain, objects.clone()), 0.0);
        // The next tick: actions (effects), bot visuals with text (a fake rasterizer).
        view.world.atlas = Atlas::new(Some(Box::new(visuals::Boxes::default())));
        let mut next = snapshot_of("W1N1", &terrain, objects.clone());
        next.objects.push(object(json!({
            "_id": "worker", "type": "creep", "room": "W1N1", "x": 6, "y": 9, "user": ME,
            "name": "worker", "hits": 100, "hitsMax": 100, "store": {},
            "body": [{"type": "work", "hits": 100}],
            "actionLog": {
                "build": {"x": 7, "y": 7}, "rangedAttack": {"x": 8, "y": 8},
                "attack": {"x": 6, "y": 8}, "attacked": {}, "rangedMassAttack": {},
                "say": {"message": "hi", "isPublic": true},
            },
        })));
        next.visuals = serde_json::from_value(json!([
            {"t": "l", "x1": 1, "y1": 1, "x2": 9, "y2": 9, "s": {"lineStyle": "dashed"}},
            {"t": "c", "x": 5, "y": 5, "s": {"stroke": "#ff0000", "radius": 2}},
            {"t": "r", "x": 2, "y": 2, "w": 3, "h": 2, "s": {"stroke": "red", "lineStyle": "dotted"}},
            {"t": "p", "points": [[0, 0], [4, 0], [2, 3]], "s": {"fill": "blue"}},
            {"t": "t", "text": "Overseer", "x": 25, "y": 25, "s": {"backgroundColor": "#000"}},
        ]))
        .unwrap();
        view.world.set_room(&next, 500.0);
        view.world
            .set_room(&snapshot_of("W0N1", &terrain, objects), 0.0);
        view.world.mark_no_terrain(room("W2N2"));
        view.select(Some((room("W1N1"), 7, 7)));
        // The overlay: badges (more than the atlas first holds), portals, the hover outline.
        for user in 0..150 {
            let id = if user == 0 {
                ME.to_owned()
            } else {
                format!("u{user}")
            };
            view.world.overlay.set_badge(&id, &[200; 64 * 64 * 4], 64);
        }
        let portal = |at: &str, to: &str, shard: Option<&str>| overseer::map::Portal {
            room: room(at),
            x: 25,
            y: 25,
            destination: Some(overseer::map::PortalDestination {
                room: room(to),
                shard: shard.map(str::to_owned),
                x: Some(25),
                y: Some(25),
            }),
            expires_at: None,
            decay_time: None,
        };
        view.world
            .overlay
            .set_portals(room("W5N5"), &[portal("W5N5", "E5S5", None)]);
        view.world.overlay.set_portals(
            room("W10N10"),
            &[portal("W10N10", "W10N10", Some("shard1"))],
        );
        // A wall of portals along W1N1's north edge, linked to W1N11; hovering W1N1 draws
        // its link as the focus.
        let wall: Vec<_> = (1..=48_u8)
            .map(|x| overseer::map::Portal {
                x,
                y: 1,
                ..portal("W1N1", "W1N11", None)
            })
            .collect();
        view.world.overlay.set_portals(room("W1N1"), &wall);
        view.set_hover(Some(room("W1N1")));
        for (mode, px_per_room) in [
            (ViewMode::Perspective, 2.0),
            (ViewMode::TopDown, 2.0),
            (ViewMode::TopDown, 30.0),
            (ViewMode::TopDown, 120.0),
            (ViewMode::Perspective, 500.0),
            (ViewMode::TopDown, 500.0),
        ] {
            view.set_mode(mode);
            place(&mut view, Vec2::new(-50.0, -50.0), px_per_room);
            let (_, animating) = view.world.frame(600.0, &view.eye());
            assert!(animating, "effects play");
            if mode == ViewMode::Perspective && px_per_room > DETAIL_PX_PER_ROOM {
                let detail = view.world.detail(room("W1N1")).unwrap();
                assert!(!detail.scene.sides().is_empty(), "prisms in perspective");
            }
            view.update_overlay();
            gpu.sync(&mut view.world);
            gpu.draw(&output, &Draw::of(&view, 600.0), &view.world);
        }
        let overlay = &view.world.overlay;
        assert!(
            !overlay.overview_glyphs.items.is_empty(),
            "sector labels drawn"
        );
        assert!(!overlay.room_glyphs.items.is_empty(), "room labels drawn");
        assert_eq!(overlay.badges.layers(), 4, "the badge atlas grew");
        assert!(!overlay.marks.items.is_empty() && !overlay.arcs.items.is_empty());
        let detail = view.world.detail(room("W1N1")).unwrap();
        assert!(!detail.scene.effects().is_empty(), "effects drawn");
        assert!(!detail.scene.visuals().glyphs.is_empty(), "text drawn");
        // A texture that grows (more rooms than reserved) and new bounds: new textures,
        // everything uploaded again.
        let layers = view.world.terrain.layers();
        for x in 0..i32::try_from(layers * 1640).unwrap() {
            view.world
                .set_terrain(RoomCoord { x: 1000 + x, y: 0 }, &terrain);
        }
        assert!(view.world.terrain.layers() > layers);
        view.set_scope(Scope::World(RoomRect::new(room("W5N5"), room("E5S5"))));
        gpu.sync(&mut view.world);
        gpu.draw(&output, &Draw::of(&view, 1000.0), &view.world);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
}

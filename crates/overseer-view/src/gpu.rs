//! The wgpu side: buffers and textures mirroring the [`World`], and the render pass of a
//! frame. Layers are drawn in order: the rooms (terrain from the overview texture and the
//! packed terrain texture, status and owner, borders; one instanced draw) and minimap points
//! (faded out under their room's detail scene) flat on the ground without depth; then each
//! detail scene: raised walls and object prisms (perspective only) with depth, shadows and
//! object tokens (blended, far to near), action effects (added), bot visuals (polygon fills,
//! shapes, text); the overlay (portal arcs and marks, badges, the hover outline, labels:
//! see `gpu/overlay.rs`); the selection frame over everything.

mod overlay;
mod pipelines;

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::{BufferInitDescriptor, DeviceExt};

use crate::MINIMAP_ROOMS;
use crate::camera::room_origin;
use crate::mesh::{
    self, EffectInstance, FillVertex, GlyphInstance, Instance, PointInstance, RoomInstance, Token,
    VisualInstance,
};
use crate::text::ATLAS_SIDE;
use crate::view::{Fades, View};
use crate::world::terrain::{LAYER_HEIGHT, LAYER_WIDTH};
use crate::world::{DETAIL_SLOTS, Detail, POINTS, Upload, World};
use overlay::{OverlayDraw, OverlayGpu};
use pipelines::Pipelines;

/// Depth buffer format (supported, with multisampling, by WebGPU and WebGL2).
pub const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Background colour (linear); also what rooms outside the world look like.
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.002,
    g: 0.002,
    b: 0.003,
    a: 1.0,
};

/// The per-frame uniform block (`Globals` in `common.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Globals {
    view_projection: [[f32; 4]; 4],
    light: [f32; 4],
    params: [f32; 4],
    plain: [f32; 4],
    swamp: [f32; 4],
    wall: [f32; 4],
    pending: [f32; 4],
    overview: [f32; 4],
    eye: [f32; 4],
}

/// The per-room uniform block of detail scenes (`Locals` in `shader.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
struct Locals {
    offset: [f32; 4],
    tint: [f32; 4],
}

/// What a frame draws besides the world's own data.
#[derive(Clone, Copy, Debug)]
pub struct Draw {
    /// World to clip space.
    pub view_projection: Mat4,
    /// The camera's eye (world).
    pub eye: Vec3,
    /// Whether the camera is in perspective (prisms and raised walls show).
    pub perspective: bool,
    /// Layer opacities.
    pub fades: Fades,
    /// The selection frame.
    pub selection: Option<Instance>,
    /// The overlay's camera, fades and hover outline.
    pub overlay: OverlayDraw,
    /// Frame time (ms), for fades and animations.
    pub now: f64,
}

impl Draw {
    /// The frame of `view` at `now`.
    pub fn of(view: &View, now: f64) -> Self {
        let eye = view.eye();
        Self {
            view_projection: view.camera.view_projection(),
            eye: eye.position,
            perspective: eye.perspective,
            fades: view.fades(),
            selection: view.selection(),
            overlay: OverlayDraw {
                px_per_room: view.camera.px_per_room(),
                tiles_per_px: view.camera.tiles_per_px(),
                size: view.camera.size().to_array(),
                fades: view.overlay_fades(),
                hover: view.world.overlay.hover(view.world.bounds()),
                target_w: {
                    let target = view.camera.target();
                    (view.camera.view_projection() * glam::Vec4::new(target.x, 0.0, target.y, 1.0))
                        .w
                },
            },
            now,
        }
    }
}

/// Render targets sized to the output.
#[derive(Debug)]
struct Targets {
    size: (u32, u32),
    depth: wgpu::TextureView,
    /// Multisampled colour, resolved into the output (`None` without multisampling).
    msaa: Option<wgpu::TextureView>,
}

/// A vertex buffer of `T`s that grows (to the next power of two) when a scene needs more.
#[derive(Debug)]
struct Slab {
    label: &'static str,
    stride: usize,
    buffer: wgpu::Buffer,
    /// Items the buffer holds.
    capacity: usize,
    /// Items written.
    len: u32,
}

impl Slab {
    fn new(device: &wgpu::Device, label: &'static str, stride: usize, capacity: usize) -> Self {
        Self {
            label,
            stride,
            buffer: buffer(device, label, capacity * stride, wgpu::BufferUsages::VERTEX),
            capacity,
            len: 0,
        }
    }

    /// Makes room for `items` (outside drawing: allocates only when growing).
    fn reserve(&mut self, device: &wgpu::Device, items: usize) {
        if items > self.capacity {
            self.capacity = items.next_power_of_two();
            self.buffer = buffer(
                device,
                self.label,
                self.capacity * self.stride,
                wgpu::BufferUsages::VERTEX,
            );
        }
    }

    /// Writes `items` from the start; nothing when they do not fit (see `reserve`).
    fn write<T: Pod>(&mut self, queue: &wgpu::Queue, items: &[T]) {
        if items.len() > self.capacity {
            self.len = 0;
            return;
        }
        if !items.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(items));
        }
        self.len = u32::try_from(items.len()).unwrap_or(0);
    }

    /// The bytes of items `range`.
    fn slice(&self, range: Range<u32>) -> wgpu::BufferSlice<'_> {
        let stride = self.stride as u64;
        self.buffer
            .slice(u64::from(range.start) * stride..u64::from(range.end) * stride)
    }
}

/// GPU resources of one detail slot.
#[derive(Debug)]
struct DetailGpu {
    /// [`Detail::id`] of the scene uploaded (0: none).
    id: u64,
    terrain_version: u64,
    /// Wall vertex and index buffers and index count.
    terrain: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    tokens: Slab,
    sides: Slab,
    effects: Slab,
    /// Visuals uploaded: the scene's visuals version.
    visuals_version: u64,
    prims: Slab,
    fills: Slab,
    glyphs: Slab,
    locals: wgpu::Buffer,
    group: wgpu::BindGroup,
}

/// A texture created for one generation of its world data.
#[derive(Debug)]
struct Generation {
    generation: u64,
    texture: wgpu::Texture,
}

/// GPU resources of the viewer.
#[derive(Debug)]
pub struct Gpu {
    pub device: wgpu::Device,
    queue: wgpu::Queue,
    /// Format of the output view (sRGB).
    pub format: wgpu::TextureFormat,
    samples: u32,
    pipelines: Pipelines,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    /// Every room's packed terrain (`R8Uint` array, see `world/terrain.rs`).
    terrain: Option<Generation>,
    /// The overview of the bounds (`Rgba8Unorm` with mips, see `world/overview.rs`).
    overview: Option<Generation>,
    overview_sampler: wgpu::Sampler,
    /// Group 1 of the rooms: both textures (`None` until they exist or after either changed).
    rooms_group: Option<wgpu::BindGroup>,
    /// The glyph atlas (`R8Unorm`, see `text.rs`) and its group.
    atlas: wgpu::Texture,
    atlas_group: wgpu::BindGroup,
    shape_vertices: wgpu::Buffer,
    shape_indices: wgpu::Buffer,
    shape_ranges: [Range<u32>; 8],
    rooms: wgpu::Buffer,
    room_capacity: usize,
    room_count: u32,
    points: wgpu::Buffer,
    details: Vec<DetailGpu>,
    selection: wgpu::Buffer,
    /// Locals of the selection: no offset, opaque.
    world_group: wgpu::BindGroup,
    /// Locals of minimaps without a detail scene: nothing covers them.
    no_detail_group: wgpu::BindGroup,
    targets: Option<Targets>,
    overlay: OverlayGpu,
}

impl Gpu {
    /// Creates the pipelines and buffers for an output of `format` (sRGB) drawn with
    /// `samples` samples per pixel; textures follow the world ([`Self::sync`]).
    #[cfg_attr(
        not(any(target_arch = "wasm32", test)),
        expect(
            dead_code,
            reason = "only the wasm canvas constructor and the tests create a renderer"
        )
    )]
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let pipelines = Pipelines::new(&device, format, samples);
        let globals = buffer(
            &device,
            "globals",
            size_of::<Globals>(),
            wgpu::BufferUsages::UNIFORM,
        );
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &pipelines.globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let linear = |label| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..wgpu::SamplerDescriptor::default()
            })
        };
        let overview_sampler = linear("overview");
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIDE,
                height: ATLAS_SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = linear("glyph atlas");
        let atlas_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph atlas"),
            layout: &pipelines.atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });
        let (shapes, shape_ranges) = mesh::shapes();
        let shape_vertices = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("shape vertices"),
            contents: bytemuck::cast_slice(&shapes.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let shape_indices = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("shape indices"),
            contents: bytemuck::cast_slice(&shapes.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let vertex = wgpu::BufferUsages::VERTEX;
        let points = buffer(
            &device,
            "minimap points",
            MINIMAP_ROOMS * POINTS * size_of::<PointInstance>(),
            vertex,
        );
        let room_capacity = 1;
        let rooms = buffer(&device, "rooms", size_of::<RoomInstance>(), vertex);
        let details = (0..DETAIL_SLOTS)
            .map(|_| detail_gpu(&device, &pipelines))
            .collect();
        let selection = buffer(&device, "selection", size_of::<Instance>(), vertex);
        let locals = |label, opacity| {
            device.create_buffer_init(&BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&Locals {
                    offset: [0.0, 0.0, 0.0, opacity],
                    tint: [0.0; 4],
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let world_group = locals_group(&device, &pipelines, &locals("world locals", 1.0));
        let no_detail_group = locals_group(&device, &pipelines, &locals("no detail", 0.0));
        let overlay = OverlayGpu::new(&device, &pipelines);
        Self {
            device,
            queue,
            format,
            samples,
            pipelines,
            globals,
            globals_group,
            terrain: None,
            overview: None,
            overview_sampler,
            rooms_group: None,
            atlas,
            atlas_group,
            shape_vertices,
            shape_indices,
            shape_ranges,
            rooms,
            room_capacity,
            room_count: 0,
            points,
            details,
            selection,
            world_group,
            no_detail_group,
            targets: None,
            overlay,
        }
    }

    /// Presents a drawn canvas texture.
    pub fn present(&self, texture: wgpu::SurfaceTexture) {
        self.queue.present(texture);
    }

    /// Uploads what changed in the world: room instances, terrain and overview texels,
    /// minimap slots, glyphs, detail walls and visuals; grows buffers and textures to the
    /// world's needs.
    pub fn sync(&mut self, world: &mut World) {
        self.sync_rooms(world);
        self.overlay.sync(
            &self.device,
            &self.queue,
            &self.pipelines,
            &mut world.overlay,
        );
        self.sync_textures(world);
        let stride = size_of::<PointInstance>();
        let (queue, points) = (&self.queue, &self.points);
        let minimaps = &mut world.minimaps;
        let mut dirty = std::mem::take(&mut minimaps.dirty);
        dirty.drain(MINIMAP_ROOMS, |slots| {
            for slot in slots {
                let data = minimaps.points(slot);
                if !data.is_empty() {
                    let offset = (slot * POINTS * stride) as u64;
                    queue.write_buffer(points, offset, bytemuck::cast_slice(data));
                }
            }
        });
        minimaps.dirty = dirty;
        let atlas = &self.atlas;
        world.atlas.flush(|origin, field| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: origin.0,
                        y: origin.1,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                field,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(crate::text::CELL),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: crate::text::CELL,
                    height: crate::text::CELL,
                    depth_or_array_layers: 1,
                },
            );
        });
        for (gpu, detail) in self.details.iter_mut().zip(world.details()) {
            if let Some(detail) = detail {
                sync_detail(&self.device, &self.queue, gpu, detail);
            } else {
                gpu.id = 0;
                gpu.terrain = None;
                gpu.visuals_version = 0;
            }
        }
    }

    fn sync_rooms(&mut self, world: &mut World) {
        let cells = &mut world.cells;
        let len = cells.instances().len();
        if len > self.room_capacity {
            self.room_capacity = len;
            self.rooms = buffer(
                &self.device,
                "rooms",
                self.room_capacity * size_of::<RoomInstance>(),
                wgpu::BufferUsages::VERTEX,
            );
            cells.dirty.mark_all();
        }
        self.room_count = u32::try_from(len).unwrap_or(u32::MAX);
        let stride = size_of::<RoomInstance>();
        let (queue, buffer) = (&self.queue, &self.rooms);
        let mut dirty = std::mem::take(&mut cells.dirty);
        dirty.drain(len, |range| {
            let offset = (range.start * stride) as u64;
            queue.write_buffer(
                buffer,
                offset,
                bytemuck::cast_slice(&cells.instances()[range]),
            );
        });
        cells.dirty = dirty;
    }

    /// (Re)creates the terrain and overview textures when their world data asks for it,
    /// then uploads the texels that changed.
    fn sync_textures(&mut self, world: &mut World) {
        let generation = world.terrain.generation();
        if self
            .terrain
            .as_ref()
            .is_none_or(|terrain| terrain.generation != generation)
        {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrain"),
                size: wgpu::Extent3d {
                    width: LAYER_WIDTH,
                    height: LAYER_HEIGHT,
                    depth_or_array_layers: world.terrain.layers(),
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.terrain = Some(Generation {
                generation,
                texture,
            });
            self.rooms_group = None;
            world.terrain.mark_all();
        }
        let generation = world.overview.generation();
        if self
            .overview
            .as_ref()
            .is_none_or(|overview| overview.generation != generation)
        {
            let (width, height) = world.overview.size();
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("overview"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: world.overview.level_count(),
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.overview = Some(Generation {
                generation,
                texture,
            });
            self.rooms_group = None;
            world.overview.mark_all();
        }
        let (Some(terrain), Some(overview)) = (&self.terrain, &self.overview) else {
            return;
        };
        let queue = &self.queue;
        world
            .terrain
            .flush(|upload| write(queue, &terrain.texture, upload));
        world
            .overview
            .flush(|upload| write(queue, &overview.texture, upload));
        if self.rooms_group.is_none() {
            let terrain = terrain.texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..wgpu::TextureViewDescriptor::default()
            });
            let overview = overview
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            self.rooms_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rooms"),
                layout: &self.pipelines.rooms_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&terrain),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&overview),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.overview_sampler),
                    },
                ],
            }));
        }
    }

    /// Sizes the depth and multisample targets to the output (recreated only on change).
    pub fn resize(&mut self, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self
            .targets
            .as_ref()
            .is_some_and(|targets| targets.size == size)
        {
            return;
        }
        let texture = |label, format, samples| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let depth = texture("depth", DEPTH, self.samples);
        let msaa = (self.samples > 1).then(|| texture("msaa", self.format, self.samples));
        self.targets = Some(Targets { size, depth, msaa });
    }

    /// Writes the uniforms and instances of a frame.
    fn prepare(&mut self, draw: &Draw, world: &World) {
        let terrain = |rgb| mesh::linear(rgb);
        // Seconds, wrapping every hour (f32 keeps sub-millisecond steps).
        #[expect(clippy::cast_possible_truncation, reason = "below 3600 after the wrap")]
        let time = (draw.now / 1000.0).rem_euclid(3600.0) as f32;
        let globals = Globals {
            view_projection: draw.view_projection.to_cols_array_2d(),
            light: light().extend(0.0).to_array(),
            params: [
                draw.fades.terrain,
                draw.fades.minimap,
                draw.fades.borders,
                draw.fades.accent,
            ],
            plain: terrain(mesh::PLAIN),
            swamp: terrain(mesh::SWAMP),
            wall: terrain(mesh::WALL_TOP),
            pending: terrain(mesh::PENDING),
            overview: world.overview.placement(),
            eye: draw.eye.extend(time).to_array(),
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        for (gpu, detail) in self.details.iter_mut().zip(world.details()) {
            let Some(detail) = detail else {
                continue;
            };
            gpu.tokens.write(&self.queue, detail.scene.tokens());
            gpu.sides.write(&self.queue, detail.scene.sides());
            gpu.effects.write(&self.queue, detail.scene.effects());
            let origin = room_origin(detail.room);
            let tint = world.tint(detail.room);
            let [r, g, b, _] = mesh::linear(u32::from_be_bytes([0, tint[0], tint[1], tint[2]]));
            let locals = Locals {
                offset: [
                    origin.x,
                    0.0,
                    origin.y,
                    detail.opacity(draw.fades.detail, draw.now),
                ],
                tint: [r, g, b, f32::from(tint[3]) / 255.0 * draw.fades.accent],
            };
            self.queue
                .write_buffer(&gpu.locals, 0, bytemuck::bytes_of(&locals));
        }
        self.overlay.prepare(&self.queue, draw, world);
        if let Some(selection) = &draw.selection {
            self.queue
                .write_buffer(&self.selection, 0, bytemuck::bytes_of(selection));
        }
    }

    /// Draws a frame of the world into `output` (a view of [`Self::format`], sized by the
    /// latest [`Self::resize`]).
    pub fn draw(&mut self, output: &wgpu::TextureView, draw: &Draw, world: &World) {
        if self.targets.is_none() {
            return;
        }
        self.prepare(draw, world);
        let Some(targets) = &self.targets else {
            return;
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let (view, resolve_target) = targets
                .msaa
                .as_ref()
                .map_or((output, None), |msaa| (msaa, Some(output)));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_group, &[]);
            self.draw_flat(&mut pass, draw, world);
            self.draw_details(&mut pass, draw, world);
            self.overlay
                .draw(&mut pass, &self.pipelines, &self.atlas_group);
            if draw.selection.is_some() {
                pass.set_pipeline(&self.pipelines.overlay);
                pass.set_bind_group(1, &self.world_group, &[]);
                pass.set_vertex_buffer(0, self.shape_vertices.slice(..));
                pass.set_vertex_buffer(1, self.selection.slice(..));
                pass.set_index_buffer(self.shape_indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(self.shape_ranges[7].clone(), 0, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// The rooms and minimap points.
    fn draw_flat(&self, pass: &mut wgpu::RenderPass<'_>, draw: &Draw, world: &World) {
        if self.room_count > 0
            && let Some(group) = &self.rooms_group
        {
            pass.set_pipeline(&self.pipelines.rooms);
            pass.set_bind_group(1, group, &[]);
            pass.set_vertex_buffer(0, self.rooms.slice(..));
            pass.draw(0..6, 0..self.room_count);
        }
        if draw.fades.minimap > 0.0 {
            pass.set_pipeline(&self.pipelines.points);
            let stride = size_of::<PointInstance>() as u64;
            let bounds = world.bounds();
            for (slot, room, count) in world.minimaps.slots() {
                if count == 0 || !bounds.contains(room) {
                    continue;
                }
                // Under the room's detail scene (as uploaded), the points fade out with it.
                let group = world
                    .detail_slot(room)
                    .and_then(|index| {
                        let detail = world.details().get(index)?.as_ref()?;
                        Some((self.details.get(index)?, detail))
                    })
                    .filter(|(gpu, detail)| gpu.id == detail.id)
                    .map_or(&self.no_detail_group, |(gpu, _)| &gpu.group);
                pass.set_bind_group(1, group, &[]);
                let start = (slot * POINTS) as u64 * stride;
                pass.set_vertex_buffer(
                    0,
                    self.points.slice(start..start + u64::from(count) * stride),
                );
                pass.draw(0..6, 0..count);
            }
        }
    }

    /// The detail scenes: walls and prisms (perspective), shadows, tokens, effects, visuals.
    fn draw_details(&self, pass: &mut wgpu::RenderPass<'_>, draw: &Draw, world: &World) {
        let bounds = world.bounds();
        for (gpu, detail) in self.details.iter().zip(world.details()) {
            let Some(detail) = detail else {
                continue;
            };
            if gpu.id != detail.id
                || !bounds.contains(detail.room)
                || detail.opacity(draw.fades.detail, draw.now) <= 0.0
            {
                continue;
            }
            pass.set_bind_group(1, &gpu.group, &[]);
            if draw.perspective {
                if let Some((vertices, indices, count)) = &gpu.terrain {
                    pass.set_pipeline(&self.pipelines.detail_terrain);
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*count, 0, 0..1);
                }
                if gpu.sides.len > 0 {
                    pass.set_pipeline(&self.pipelines.sides);
                    pass.set_vertex_buffer(0, self.shape_vertices.slice(..));
                    pass.set_index_buffer(self.shape_indices.slice(..), wgpu::IndexFormat::Uint32);
                    for (shape, range) in self.shape_ranges.iter().zip(&detail.frame.sides) {
                        if range.is_empty() || range.end > gpu.sides.len {
                            continue;
                        }
                        // WebGL2 has no first-instance offset: bind at the first instance.
                        pass.set_vertex_buffer(1, gpu.sides.slice(range.clone()));
                        pass.draw_indexed(shape.clone(), 0, 0..range.end - range.start);
                    }
                }
            }
            if gpu.tokens.len > 0 {
                pass.set_vertex_buffer(0, gpu.tokens.slice(0..gpu.tokens.len));
                pass.set_pipeline(&self.pipelines.shadows);
                pass.draw(0..6, 0..gpu.tokens.len);
                pass.set_pipeline(&self.pipelines.tokens);
                pass.draw(0..6, 0..gpu.tokens.len);
            }
            if gpu.effects.len > 0 {
                pass.set_pipeline(&self.pipelines.effects);
                pass.set_vertex_buffer(0, gpu.effects.slice(0..gpu.effects.len));
                pass.draw(0..6, 0..gpu.effects.len);
            }
            if gpu.fills.len > 0 {
                pass.set_pipeline(&self.pipelines.fills);
                pass.set_vertex_buffer(0, gpu.fills.slice(0..gpu.fills.len));
                pass.draw(0..gpu.fills.len, 0..1);
            }
            if gpu.prims.len > 0 {
                pass.set_pipeline(&self.pipelines.prims);
                pass.set_vertex_buffer(0, gpu.prims.slice(0..gpu.prims.len));
                pass.draw(0..6, 0..gpu.prims.len);
            }
            if gpu.glyphs.len > 0 {
                pass.set_pipeline(&self.pipelines.glyphs);
                pass.set_bind_group(2, &self.atlas_group, &[]);
                pass.set_vertex_buffer(0, gpu.glyphs.slice(0..gpu.glyphs.len));
                pass.draw(0..6, 0..gpu.glyphs.len);
            }
        }
    }
}

/// The direction towards the light.
fn light() -> Vec3 {
    Vec3::new(-0.4, 1.0, -0.3).normalize()
}

/// Uploads a detail scene's walls and visuals when they changed and grows its instance
/// buffers to what its frames need.
fn sync_detail(device: &wgpu::Device, queue: &wgpu::Queue, gpu: &mut DetailGpu, detail: &Detail) {
    let (terrain, version) = detail.scene.terrain();
    if gpu.id != detail.id || gpu.terrain_version != version {
        gpu.id = detail.id;
        gpu.terrain_version = version;
        gpu.visuals_version = 0;
        gpu.terrain = (!terrain.indices.is_empty()).then(|| {
            let vertices = device.create_buffer_init(&BufferInitDescriptor {
                label: Some("wall vertices"),
                contents: bytemuck::cast_slice(&terrain.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let indices = device.create_buffer_init(&BufferInitDescriptor {
                label: Some("wall indices"),
                contents: bytemuck::cast_slice(&terrain.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let count = u32::try_from(terrain.indices.len()).unwrap_or(u32::MAX);
            (vertices, indices, count)
        });
    }
    let (objects, effects) = detail.scene.capacity();
    gpu.tokens.reserve(device, objects);
    gpu.sides.reserve(device, objects);
    gpu.effects.reserve(device, effects);
    let visuals = detail.scene.visuals();
    if gpu.visuals_version != visuals.version {
        gpu.visuals_version = visuals.version;
        gpu.prims.reserve(device, visuals.prims.len());
        gpu.fills.reserve(device, visuals.fills.len());
        gpu.glyphs.reserve(device, visuals.glyphs.len());
        gpu.prims.write(queue, &visuals.prims);
        gpu.fills.write(queue, &visuals.fills);
        gpu.glyphs.write(queue, &visuals.glyphs);
    }
}

/// An empty detail slot.
fn detail_gpu(device: &wgpu::Device, pipelines: &Pipelines) -> DetailGpu {
    let locals = buffer(
        device,
        "locals",
        size_of::<Locals>(),
        wgpu::BufferUsages::UNIFORM,
    );
    let group = locals_group(device, pipelines, &locals);
    DetailGpu {
        id: 0,
        terrain_version: 0,
        terrain: None,
        tokens: Slab::new(device, "tokens", size_of::<Token>(), 64),
        sides: Slab::new(device, "prisms", size_of::<Instance>(), 64),
        effects: Slab::new(device, "effects", size_of::<EffectInstance>(), 16),
        visuals_version: 0,
        prims: Slab::new(device, "visual shapes", size_of::<VisualInstance>(), 16),
        fills: Slab::new(device, "visual fills", size_of::<FillVertex>(), 16),
        glyphs: Slab::new(device, "visual text", size_of::<GlyphInstance>(), 16),
        locals,
        group,
    }
}

fn locals_group(
    device: &wgpu::Device,
    pipelines: &Pipelines,
    locals: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("locals"),
        layout: &pipelines.locals_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: locals.as_entire_binding(),
        }],
    })
}

/// Writes an upload of the world's texels into `texture`.
fn write(queue: &wgpu::Queue, texture: &wgpu::Texture, upload: Upload<'_>) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: upload.level,
            origin: wgpu::Origin3d {
                x: upload.origin.0,
                y: upload.origin.1,
                z: upload.layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        upload.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(upload.bytes_per_row),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: upload.size.0,
            height: upload.size.1,
            depth_or_array_layers: 1,
        },
    );
}

/// A buffer of `size` bytes, writable from the queue.
fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: usize,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16) as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

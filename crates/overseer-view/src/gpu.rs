//! The wgpu side: buffers and textures mirroring the [`World`], and the render pass of a
//! frame. Layers are drawn in order: overview cells, atlas terrain and minimap points flat on
//! the ground without depth, then the detail scenes with depth (blended by their opacity) and
//! the selection frame over everything.

mod pipelines;

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::{BufferInitDescriptor, DeviceExt};

use crate::MINIMAP_ROOMS;
use crate::camera::room_origin;
use crate::mesh::{self, CellInstance, Instance, PointInstance, TerrainInstance};
use crate::view::{Fades, View};
use crate::world::{DETAIL_SLOTS, Detail, LEVEL0, LEVEL1, POINTS, World};
use pipelines::Pipelines;

/// Depth buffer format (supported, with multisampling, by WebGPU and WebGL2).
pub const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Background colour (linear).
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.002,
    g: 0.002,
    b: 0.003,
    a: 1.0,
};
/// Mip levels of the terrain atlas: 50 and 25 texels per room.
const ATLAS_LEVELS: u32 = 2;

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
    /// Layer opacities.
    pub fades: Fades,
    /// The selection frame.
    pub selection: Option<Instance>,
    /// Frame time (ms), for the detail scenes' fade-in.
    pub now: f64,
}

impl Draw {
    /// The frame of `view` at `now`.
    pub fn of(view: &View, now: f64) -> Self {
        Self {
            view_projection: view.camera.view_projection(),
            fades: view.fades(),
            selection: view.selection(),
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

/// GPU resources of one detail slot.
#[derive(Debug)]
struct DetailGpu {
    /// [`Detail::id`] of the scene uploaded (0: none).
    id: u64,
    terrain_version: u64,
    /// Terrain vertex and index buffers and index count.
    terrain: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    instances: wgpu::Buffer,
    capacity: usize,
    locals: wgpu::Buffer,
    group: wgpu::BindGroup,
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
    atlas: wgpu::Texture,
    atlas_side: u32,
    atlas_per_row: u32,
    atlas_group: wgpu::BindGroup,
    shape_vertices: wgpu::Buffer,
    shape_indices: wgpu::Buffer,
    shape_ranges: [Range<u32>; 3],
    cells: wgpu::Buffer,
    cell_capacity: usize,
    cell_count: u32,
    terrain_instances: wgpu::Buffer,
    terrain_slots: u32,
    points: wgpu::Buffer,
    details: Vec<DetailGpu>,
    selection: wgpu::Buffer,
    /// Locals of the selection: no offset, opaque.
    world_group: wgpu::BindGroup,
    /// Texel scratch of atlas uploads.
    level0: Box<[u8; LEVEL0]>,
    level1: Box<[u8; LEVEL1]>,
    targets: Option<Targets>,
}

impl Gpu {
    /// Creates the pipelines and buffers for an output of `format` (sRGB) drawn with
    /// `samples` samples per pixel and a terrain atlas of `atlas_per_row²` rooms.
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
        atlas_per_row: u32,
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
        let atlas_side = atlas_per_row * 50;
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain atlas"),
            size: wgpu::Extent3d {
                width: atlas_side,
                height: atlas_side,
                depth_or_array_layers: 1,
            },
            mip_level_count: ATLAS_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let filter = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain atlas"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain atlas"),
            layout: &pipelines.atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&filter),
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
        let terrain_slots = atlas_per_row * atlas_per_row;
        let vertex = wgpu::BufferUsages::VERTEX;
        let terrain_instances = buffer(
            &device,
            "terrain instances",
            terrain_slots as usize * size_of::<TerrainInstance>(),
            vertex,
        );
        let points = buffer(
            &device,
            "minimap points",
            MINIMAP_ROOMS * POINTS * size_of::<PointInstance>(),
            vertex,
        );
        let cell_capacity = 1;
        let cells = buffer(&device, "cells", size_of::<CellInstance>(), vertex);
        let details = (0..DETAIL_SLOTS)
            .map(|_| detail_gpu(&device, &pipelines, 64))
            .collect();
        let selection = buffer(&device, "selection", size_of::<Instance>(), vertex);
        let world_locals = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("world locals"),
            contents: bytemuck::bytes_of(&Locals {
                offset: [0.0, 0.0, 0.0, 1.0],
                tint: [0.0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let world_group = locals_group(&device, &pipelines, &world_locals);
        Self {
            device,
            queue,
            format,
            samples,
            pipelines,
            globals,
            globals_group,
            atlas,
            atlas_side,
            atlas_per_row,
            atlas_group,
            shape_vertices,
            shape_indices,
            shape_ranges,
            cells,
            cell_capacity,
            cell_count: 0,
            terrain_instances,
            terrain_slots,
            points,
            details,
            selection,
            world_group,
            level0: Box::new([0; LEVEL0]),
            level1: Box::new([0; LEVEL1]),
            targets: None,
        }
    }

    /// Presents a drawn canvas texture.
    pub fn present(&self, texture: wgpu::SurfaceTexture) {
        self.queue.present(texture);
    }

    /// Uploads what changed in the world: cells, atlas texels and instances, minimap slots
    /// and detail terrain; grows buffers to the world's needs.
    pub fn sync(&mut self, world: &mut World) {
        self.sync_cells(world);
        self.sync_atlas(world);
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
        for (gpu, detail) in self.details.iter_mut().zip(world.details()) {
            if let Some(detail) = detail {
                sync_detail(&self.device, gpu, detail);
            } else {
                gpu.id = 0;
                gpu.terrain = None;
            }
        }
    }

    fn sync_cells(&mut self, world: &mut World) {
        let cells = &mut world.cells;
        let len = cells.instances().len();
        if len > self.cell_capacity {
            self.cell_capacity = len.next_power_of_two();
            self.cells = buffer(
                &self.device,
                "cells",
                self.cell_capacity * size_of::<CellInstance>(),
                wgpu::BufferUsages::VERTEX,
            );
            cells.dirty.mark_all();
        }
        self.cell_count = u32::try_from(len).unwrap_or(u32::MAX);
        let stride = size_of::<CellInstance>();
        let (queue, buffer) = (&self.queue, &self.cells);
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

    fn sync_atlas(&mut self, world: &mut World) {
        let atlas = &mut world.atlas;
        let mut uploads = std::mem::take(&mut atlas.uploads);
        uploads.sort_unstable();
        uploads.dedup();
        for &slot in &uploads {
            if !atlas.texels(slot, &mut self.level0, &mut self.level1) {
                continue;
            }
            let (column, row) = (slot % self.atlas_per_row, slot / self.atlas_per_row);
            for (level, data, side) in [(0, &self.level0[..], 50), (1, &self.level1[..], 25)] {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.atlas,
                        mip_level: level,
                        origin: wgpu::Origin3d {
                            x: column * side,
                            y: row * side,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(side * 2),
                        rows_per_image: Some(side),
                    },
                    wgpu::Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        uploads.clear();
        atlas.uploads = uploads;
        let stride = size_of::<TerrainInstance>();
        let (queue, buffer) = (&self.queue, &self.terrain_instances);
        let mut dirty = std::mem::take(&mut atlas.dirty);
        dirty.drain(atlas.instances().len(), |range| {
            let offset = (range.start * stride) as u64;
            queue.write_buffer(
                buffer,
                offset,
                bytemuck::cast_slice(&atlas.instances()[range]),
            );
        });
        atlas.dirty = dirty;
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
    fn prepare(&self, draw: &Draw, world: &World) {
        let terrain = |rgb| mesh::linear(rgb);
        let globals = Globals {
            view_projection: draw.view_projection.to_cols_array_2d(),
            light: light().extend(0.0).to_array(),
            params: [
                draw.fades.terrain,
                draw.fades.minimap,
                draw.fades.borders,
                1.0 / f32::from(u16::try_from(self.atlas_side).unwrap_or(u16::MAX)),
            ],
            plain: terrain(mesh::PLAIN),
            swamp: terrain(mesh::SWAMP),
            wall: terrain(mesh::WALL_TOP),
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        for (gpu, detail) in self.details.iter().zip(world.details()) {
            let Some(detail) = detail else {
                continue;
            };
            let instances = detail.scene.instances();
            if !instances.is_empty() && instances.len() <= gpu.capacity {
                self.queue
                    .write_buffer(&gpu.instances, 0, bytemuck::cast_slice(instances));
            }
            let origin = room_origin(detail.room);
            let tint = world.tint(detail.room);
            let [r, g, b, _] = mesh::linear(u32::from_be_bytes([0, tint[0], tint[1], tint[2]]));
            let locals = Locals {
                offset: [
                    origin.x,
                    0.0,
                    origin.y,
                    draw.fades.detail * detail.arrival(draw.now),
                ],
                tint: [r, g, b, f32::from(tint[3]) / 255.0],
            };
            self.queue
                .write_buffer(&gpu.locals, 0, bytemuck::bytes_of(&locals));
        }
        if let Some(selection) = &draw.selection {
            self.queue
                .write_buffer(&self.selection, 0, bytemuck::bytes_of(selection));
        }
    }

    /// Draws a frame of the world into `output` (a view of [`Self::format`], sized by the
    /// latest [`Self::resize`]).
    pub fn draw(&self, output: &wgpu::TextureView, draw: &Draw, world: &World) {
        let Some(targets) = &self.targets else {
            return;
        };
        self.prepare(draw, world);
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
            if draw.selection.is_some() {
                pass.set_pipeline(&self.pipelines.overlay);
                pass.set_bind_group(1, &self.world_group, &[]);
                pass.set_vertex_buffer(0, self.shape_vertices.slice(..));
                pass.set_vertex_buffer(1, self.selection.slice(..));
                pass.set_index_buffer(self.shape_indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(self.shape_ranges[2].clone(), 0, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Cells, atlas terrain and minimap points.
    fn draw_flat(&self, pass: &mut wgpu::RenderPass<'_>, draw: &Draw, world: &World) {
        if self.cell_count > 0 {
            pass.set_pipeline(&self.pipelines.cells);
            pass.set_vertex_buffer(0, self.cells.slice(..));
            pass.draw(0..6, 0..self.cell_count);
        }
        if draw.fades.terrain > 0.0 {
            pass.set_pipeline(&self.pipelines.atlas);
            pass.set_bind_group(1, &self.atlas_group, &[]);
            pass.set_vertex_buffer(0, self.terrain_instances.slice(..));
            pass.draw(0..6, 0..self.terrain_slots);
        }
        if draw.fades.minimap > 0.0 {
            pass.set_pipeline(&self.pipelines.points);
            let stride = size_of::<PointInstance>() as u64;
            let bounds = world.bounds();
            for (slot, room, count) in world.minimaps.slots() {
                if count == 0 || !bounds.contains(room) {
                    continue;
                }
                let start = (slot * POINTS) as u64 * stride;
                pass.set_vertex_buffer(
                    0,
                    self.points.slice(start..start + u64::from(count) * stride),
                );
                pass.draw(0..6, 0..count);
            }
        }
    }

    /// The detail scenes: terrain, then objects per shape.
    fn draw_details(&self, pass: &mut wgpu::RenderPass<'_>, draw: &Draw, world: &World) {
        let bounds = world.bounds();
        for (gpu, detail) in self.details.iter().zip(world.details()) {
            let Some(detail) = detail else {
                continue;
            };
            if gpu.id != detail.id
                || !bounds.contains(detail.room)
                || draw.fades.detail * detail.arrival(draw.now) <= 0.0
            {
                continue;
            }
            pass.set_bind_group(1, &gpu.group, &[]);
            if let Some((vertices, indices, count)) = &gpu.terrain {
                pass.set_pipeline(&self.pipelines.detail_terrain);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }
            if detail.scene.instances().len() > gpu.capacity {
                continue;
            }
            pass.set_pipeline(&self.pipelines.objects);
            pass.set_vertex_buffer(0, self.shape_vertices.slice(..));
            pass.set_index_buffer(self.shape_indices.slice(..), wgpu::IndexFormat::Uint32);
            for (shape, range) in self.shape_ranges.iter().zip(&detail.frame.ranges) {
                if range.is_empty() {
                    continue;
                }
                // WebGL2 has no first-instance offset: bind the buffer at the first instance.
                let stride = size_of::<Instance>() as u64;
                let bytes = u64::from(range.start) * stride..u64::from(range.end) * stride;
                pass.set_vertex_buffer(1, gpu.instances.slice(bytes));
                pass.draw_indexed(shape.clone(), 0, 0..range.end - range.start);
            }
        }
    }
}

/// The direction towards the light.
fn light() -> Vec3 {
    Vec3::new(-0.4, 1.0, -0.3).normalize()
}

/// Uploads a detail scene's terrain when it changed and grows its instance buffer.
fn sync_detail(device: &wgpu::Device, gpu: &mut DetailGpu, detail: &Detail) {
    let (terrain, version) = detail.scene.terrain();
    if gpu.id != detail.id || gpu.terrain_version != version {
        gpu.id = detail.id;
        gpu.terrain_version = version;
        gpu.terrain = (!terrain.indices.is_empty()).then(|| {
            let vertices = device.create_buffer_init(&BufferInitDescriptor {
                label: Some("terrain vertices"),
                contents: bytemuck::cast_slice(&terrain.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let indices = device.create_buffer_init(&BufferInitDescriptor {
                label: Some("terrain indices"),
                contents: bytemuck::cast_slice(&terrain.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let count = u32::try_from(terrain.indices.len()).unwrap_or(u32::MAX);
            (vertices, indices, count)
        });
    }
    let needed = detail.scene.instance_capacity();
    if needed > gpu.capacity {
        gpu.capacity = needed.next_power_of_two();
        gpu.instances = instance_buffer(device, gpu.capacity);
    }
}

/// An empty detail slot with room for `capacity` instances.
fn detail_gpu(device: &wgpu::Device, pipelines: &Pipelines, capacity: usize) -> DetailGpu {
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
        instances: instance_buffer(device, capacity),
        capacity,
        locals,
        group,
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    buffer(
        device,
        "instances",
        capacity * size_of::<Instance>(),
        wgpu::BufferUsages::VERTEX,
    )
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

//! The overlay's GPU side: its uniforms, the badge atlas texture and the instance buffers
//! of marks, arcs and labels (uploaded when their version changes), drawn over the world
//! and the detail scenes: arcs, then marks (badges, portals), the hover outline, labels.

use bytemuck::{Pod, Zeroable};

use super::pipelines::Pipelines;
use super::{Draw, Generation, Slab, buffer, write};
use crate::camera::room_centre;
use crate::mesh::{ArcInstance, LabelGlyph, MarkInstance};
use crate::overlay::badges::{LAYER_HEIGHT, LAYER_WIDTH, LEVELS};
use crate::overlay::{Fades, Layer, Overlay};
use crate::world::{DETAIL_SLOTS, World};

/// The overlay's uniform block (`Overlay` in `overlay.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct Uniforms {
    view: [f32; 4],
    labels: [f32; 4],
    marks: [f32; 4],
    links: [f32; 4],
    details: [[f32; 4]; DETAIL_SLOTS],
}

/// What a frame's overlay needs from the camera.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlayDraw {
    /// CSS pixels per room at the camera's target.
    pub px_per_room: f32,
    /// Tiles per CSS pixel at the camera's target.
    pub tiles_per_px: f32,
    /// Canvas size, CSS pixels.
    pub size: [f32; 2],
    /// Opacity of each part.
    pub fades: Fades,
    /// The hovered room's outline.
    pub hover: Option<MarkInstance>,
    /// Clip-space `w` of the camera's target: rooms elsewhere are this many times as
    /// large as at the target divided by their own `w` (perspective).
    pub target_w: f32,
}

/// An instance buffer and the version of the data it holds.
#[derive(Debug)]
struct Instances {
    slab: Slab,
    version: u64,
}

impl Instances {
    fn new<T>(device: &wgpu::Device, label: &'static str) -> Self {
        Self {
            slab: Slab::new(device, label, size_of::<T>(), 16),
            version: 0,
        }
    }

    /// Uploads `layer` when it changed.
    fn sync<T: Pod>(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, layer: &Layer<T>) {
        if self.version != layer.version {
            self.version = layer.version;
            self.slab.reserve(device, layer.items.len());
            self.slab.write(queue, &layer.items);
        }
    }

    /// Draws the instances with `vertices` vertices each (the pipeline is set).
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, vertices: u32) {
        if self.slab.len > 0 {
            pass.set_vertex_buffer(0, self.slab.slice(0..self.slab.len));
            pass.draw(0..vertices, 0..self.slab.len);
        }
    }
}

/// Vertices of an arc: two triangles per segment (`ARC_SEGMENTS` in `overlay.wgsl`).
const ARC_VERTICES: u32 = 24 * 6;

/// The overlay's GPU resources.
#[derive(Debug)]
pub struct OverlayGpu {
    uniforms: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// The badge atlas (`Rgba8UnormSrgb` 2D array with mips, see `overlay/badges.rs`).
    badges: Option<Generation>,
    badges_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    marks: Instances,
    portal_marks: Instances,
    arcs: Instances,
    overview: Instances,
    rooms: Instances,
    hover: wgpu::Buffer,
    hovered: bool,
}

impl OverlayGpu {
    /// Buffers for the overlay; the badge texture follows the world ([`Self::sync`]).
    pub fn new(device: &wgpu::Device, pipelines: &Pipelines) -> Self {
        let uniforms = buffer(
            device,
            "overlay",
            size_of::<Uniforms>(),
            wgpu::BufferUsages::UNIFORM,
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay"),
            layout: &pipelines.overlays.uniforms_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("badges"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let vertex = wgpu::BufferUsages::VERTEX;
        Self {
            uniforms,
            group,
            badges: None,
            badges_group: None,
            sampler,
            marks: Instances::new::<MarkInstance>(device, "overlay badges"),
            portal_marks: Instances::new::<MarkInstance>(device, "portal marks"),
            arcs: Instances::new::<ArcInstance>(device, "overlay arcs"),
            overview: Instances::new::<LabelGlyph>(device, "overlay labels"),
            rooms: Instances::new::<LabelGlyph>(device, "room labels"),
            hover: buffer(device, "hover", size_of::<MarkInstance>(), vertex),
            hovered: false,
        }
    }

    /// Uploads what changed: badges (a new texture when the atlas grew) and instances.
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipelines: &Pipelines,
        overlay: &mut Overlay,
    ) {
        let generation = overlay.badges.generation();
        if self
            .badges
            .as_ref()
            .is_none_or(|badges| badges.generation != generation)
        {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("badges"),
                size: wgpu::Extent3d {
                    width: LAYER_WIDTH,
                    height: LAYER_HEIGHT,
                    depth_or_array_layers: overlay.badges.layers(),
                },
                mip_level_count: LEVELS,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..wgpu::TextureViewDescriptor::default()
            });
            self.badges_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("badges"),
                layout: &pipelines.overlays.badges_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            }));
            self.badges = Some(Generation {
                generation,
                texture,
            });
            overlay.badges.mark_all();
        }
        if let Some(badges) = &self.badges {
            overlay
                .badges
                .flush(|upload| write(queue, &badges.texture, upload));
        }
        self.marks.sync(device, queue, &overlay.marks);
        self.portal_marks.sync(device, queue, &overlay.portal_marks);
        self.arcs.sync(device, queue, &overlay.arcs);
        self.overview.sync(device, queue, &overlay.overview_glyphs);
        self.rooms.sync(device, queue, &overlay.room_glyphs);
    }

    /// Writes the uniforms and the hover outline of a frame.
    pub fn prepare(&mut self, queue: &wgpu::Queue, draw: &Draw, world: &World) {
        let overlay = &draw.overlay;
        let mut details = [[0.0; 4]; DETAIL_SLOTS];
        for (slot, detail) in details.iter_mut().zip(world.details()) {
            if let Some(detail) = detail {
                let centre = room_centre(detail.room);
                let opacity = detail.opacity(draw.fades.detail, draw.now);
                *slot = [centre.x, centre.y, opacity, 0.0];
            }
        }
        let uniforms = Uniforms {
            view: [
                overlay.px_per_room,
                overlay.tiles_per_px,
                overlay.size[0],
                overlay.size[1],
            ],
            labels: [
                overlay.fades.labels[0],
                overlay.fades.labels[1],
                overlay.fades.labels[2],
                overlay.target_w,
            ],
            marks: overlay.fades.marks,
            links: overlay.fades.links,
            details,
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        self.hovered = overlay.hover.is_some();
        if let Some(hover) = &overlay.hover {
            queue.write_buffer(&self.hover, 0, bytemuck::bytes_of(hover));
        }
    }

    /// The badge atlas' group (the tokens of creeps draw their owner's badge from it); `None`
    /// before the first [`Self::sync`].
    pub const fn badges_group(&self) -> Option<&wgpu::BindGroup> {
        self.badges_group.as_ref()
    }

    /// Draws the overlay (group 0 is bound; `atlas` is the glyph atlas' group).
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipelines: &Pipelines,
        atlas: &wgpu::BindGroup,
    ) {
        let Some(badges) = &self.badges_group else {
            return;
        };
        let overlays = &pipelines.overlays;
        pass.set_bind_group(1, &self.group, &[]);
        pass.set_bind_group(2, atlas, &[]);
        pass.set_bind_group(3, badges, &[]);
        pass.set_pipeline(&overlays.arcs);
        self.arcs.draw(pass, ARC_VERTICES);
        pass.set_pipeline(&overlays.marks);
        self.marks.draw(pass, 6);
        self.portal_marks.draw(pass, 6);
        if self.hovered {
            pass.set_vertex_buffer(0, self.hover.slice(..));
            pass.draw(0..6, 0..1);
        }
        pass.set_pipeline(&overlays.labels);
        self.overview.draw(pass, 6);
        self.rooms.draw(pass, 6);
    }
}

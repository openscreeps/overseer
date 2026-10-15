//! The wgpu side: pipelines, buffers and the render pass of a frame.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::{BufferInitDescriptor, DeviceExt};

use crate::mesh::{self, Instance, ShapeVertex, TerrainVertex};
use crate::scene::{Frame, Scene};

/// Depth buffer format (supported, with multisampling, by WebGPU and WebGL2).
pub const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Background colour (linear).
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.002,
    g: 0.002,
    b: 0.003,
    a: 1.0,
};

/// The uniform block of the shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Globals {
    view_projection: [[f32; 4]; 4],
    light: [f32; 4],
}

/// Render targets sized to the output.
#[derive(Debug)]
struct Targets {
    size: (u32, u32),
    depth: wgpu::TextureView,
    /// Multisampled colour, resolved into the output (`None` without multisampling).
    msaa: Option<wgpu::TextureView>,
}

/// GPU resources of the viewer.
#[derive(Debug)]
pub struct Gpu {
    pub device: wgpu::Device,
    queue: wgpu::Queue,
    /// Format of the output view (sRGB).
    pub format: wgpu::TextureFormat,
    samples: u32,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    terrain_pipeline: wgpu::RenderPipeline,
    object_pipeline: wgpu::RenderPipeline,
    /// Object pipeline drawn over everything (the selection frame).
    overlay_pipeline: wgpu::RenderPipeline,
    shape_vertices: wgpu::Buffer,
    shape_indices: wgpu::Buffer,
    shape_ranges: [Range<u32>; 3],
    /// Terrain vertex and index buffers, index count and the scene's terrain version.
    terrain: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    terrain_version: u64,
    instances: wgpu::Buffer,
    instance_capacity: usize,
    targets: Option<Targets>,
}

impl Gpu {
    /// Creates the pipelines and static buffers for an output of `format` (sRGB) drawn with
    /// `samples` samples per pixel.
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overseer-view"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overseer-view"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = Pipelines {
            device: &device,
            layout: &pipeline_layout,
            shader: &shader,
            format,
            samples,
        };
        let terrain_pipeline = pipelines.terrain();
        let object_pipeline = pipelines.objects(false);
        let overlay_pipeline = pipelines.objects(true);
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
        let instance_capacity = 64;
        let instances = instance_buffer(&device, instance_capacity);
        Self {
            device,
            queue,
            format,
            samples,
            globals,
            bind_group,
            terrain_pipeline,
            object_pipeline,
            overlay_pipeline,
            shape_vertices,
            shape_indices,
            shape_ranges,
            terrain: None,
            terrain_version: 0,
            instances,
            instance_capacity,
            targets: None,
        }
    }

    /// Presents a drawn canvas texture.
    pub fn present(&self, texture: wgpu::SurfaceTexture) {
        self.queue.present(texture);
    }

    /// Uploads the scene's terrain when it changed and grows the instance buffer to the
    /// scene's needs (called when the scene changes, so that frames do not allocate).
    pub fn sync(&mut self, scene: &Scene) {
        let (terrain, version) = scene.terrain();
        if version != self.terrain_version {
            self.terrain_version = version;
            self.terrain = (!terrain.indices.is_empty()).then(|| {
                let vertices = self.device.create_buffer_init(&BufferInitDescriptor {
                    label: Some("terrain vertices"),
                    contents: bytemuck::cast_slice(&terrain.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let indices = self.device.create_buffer_init(&BufferInitDescriptor {
                    label: Some("terrain indices"),
                    contents: bytemuck::cast_slice(&terrain.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                let count = u32::try_from(terrain.indices.len()).unwrap_or(u32::MAX);
                (vertices, indices, count)
            });
        }
        let needed = scene.instance_capacity();
        if needed > self.instance_capacity {
            self.instance_capacity = needed.next_power_of_two();
            self.instances = instance_buffer(&self.device, self.instance_capacity);
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

    /// Draws a frame of the scene into `output` (a view of [`Self::format`], sized by the
    /// latest [`Self::resize`]).
    pub fn draw(
        &mut self,
        output: &wgpu::TextureView,
        view_projection: Mat4,
        scene: &Scene,
        frame: &Frame,
    ) {
        let Some(targets) = &self.targets else {
            return;
        };
        let globals = Globals {
            view_projection: view_projection.to_cols_array_2d(),
            light: Vec3::new(-0.4, 1.0, -0.3)
                .normalize()
                .extend(0.0)
                .to_array(),
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        let instances = scene.instances();
        if !instances.is_empty() && instances.len() <= self.instance_capacity {
            self.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let (view, resolve_target) = targets
                .msaa
                .as_ref()
                .map_or((output, None), |msaa| (msaa, Some(output)));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("room"),
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            if let Some((vertices, indices, count)) = &self.terrain {
                pass.set_pipeline(&self.terrain_pipeline);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }
            if instances.len() <= self.instance_capacity {
                pass.set_vertex_buffer(0, self.shape_vertices.slice(..));
                pass.set_index_buffer(self.shape_indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_pipeline(&self.object_pipeline);
                for (shape, range) in self.shape_ranges.iter().zip(&frame.ranges) {
                    self.draw_instances(&mut pass, shape, range.clone());
                }
                if let Some(selection) = frame.selection {
                    pass.set_pipeline(&self.overlay_pipeline);
                    let ring = &self.shape_ranges[2];
                    self.draw_instances(&mut pass, ring, selection..selection + 1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Draws `instances` of a shape. The instance buffer is bound at the first instance:
    /// WebGL2 has no first-instance offset.
    fn draw_instances(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        shape: &Range<u32>,
        instances: Range<u32>,
    ) {
        if instances.is_empty() {
            return;
        }
        let stride = size_of::<Instance>() as u64;
        let bytes = u64::from(instances.start) * stride..u64::from(instances.end) * stride;
        pass.set_vertex_buffer(1, self.instances.slice(bytes));
        pass.draw_indexed(shape.clone(), 0, 0..instances.end - instances.start);
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: (capacity * size_of::<Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// What every pipeline shares.
struct Pipelines<'a> {
    device: &'a wgpu::Device,
    layout: &'a wgpu::PipelineLayout,
    shader: &'a wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
}

impl Pipelines<'_> {
    fn terrain(&self) -> wgpu::RenderPipeline {
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<TerrainVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        })];
        self.build("terrain", "terrain_vertex", &buffers, false)
    }

    /// The object pipeline; `overlay` draws over everything (no depth test or write).
    fn objects(&self, overlay: bool) -> wgpu::RenderPipeline {
        let shape = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let instance = wgpu::vertex_attr_array![2 => Float32x3, 3 => Float32x3, 4 => Float32x4];
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<ShapeVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &shape,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance,
            }),
        ];
        let label = if overlay { "overlay" } else { "objects" };
        self.build(label, "object_vertex", &buffers, overlay)
    }

    fn build(
        &self,
        label: &str,
        entry_point: &str,
        buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
        overlay: bool,
    ) -> wgpu::RenderPipeline {
        self.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(self.layout),
                vertex: wgpu::VertexState {
                    module: self.shader,
                    entry_point: Some(entry_point),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: Some(wgpu::Face::Back),
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(!overlay),
                    depth_compare: Some(if overlay {
                        wgpu::CompareFunction::Always
                    } else {
                        wgpu::CompareFunction::Less
                    }),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: self.samples,
                    ..wgpu::MultisampleState::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: self.shader,
                    entry_point: Some("fragment"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
    }
}

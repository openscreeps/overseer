//! Bind group layouts and render pipelines.

use crate::mesh::{Instance, PointInstance, RoomInstance, ShapeVertex, TerrainVertex};

use super::DEPTH;

/// Every pipeline and the bind group layouts they use.
#[derive(Debug)]
pub struct Pipelines {
    /// Group 0: the per-frame uniforms.
    pub globals_layout: wgpu::BindGroupLayout,
    /// Group 1 of the rooms: packed terrain, overview texture and its sampler.
    pub rooms_layout: wgpu::BindGroupLayout,
    /// Group 1 of detail scenes: the per-room uniforms.
    pub locals_layout: wgpu::BindGroupLayout,
    /// The rooms: terrain (overview and full resolution), status, owner and borders.
    pub rooms: wgpu::RenderPipeline,
    /// Minimap points.
    pub points: wgpu::RenderPipeline,
    /// Detail terrain meshes.
    pub detail_terrain: wgpu::RenderPipeline,
    /// Detail objects.
    pub objects: wgpu::RenderPipeline,
    /// Objects drawn over everything (the selection frame).
    pub overlay: wgpu::RenderPipeline,
}

/// How a pipeline treats depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Depth {
    /// Flat layers: drawn in order, no depth test or write.
    Ignore,
    /// Detail scenes: tested and written.
    Test,
}

/// What the pipelines of one layout share.
struct Builder<'a> {
    device: &'a wgpu::Device,
    format: wgpu::TextureFormat,
    samples: u32,
}

impl Builder<'_> {
    #[expect(
        clippy::too_many_arguments,
        reason = "one call per pipeline; a struct would only rename the arguments"
    )]
    fn build(
        &self,
        label: &str,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        entry: (&str, &str),
        buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
        depth: Depth,
        blend: Option<wgpu::BlendState>,
        cull: Option<wgpu::Face>,
    ) -> wgpu::RenderPipeline {
        let test = depth == Depth::Test;
        self.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(entry.0),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: cull,
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(test),
                    depth_compare: Some(if test {
                        wgpu::CompareFunction::Less
                    } else {
                        wgpu::CompareFunction::Always
                    }),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: self.samples,
                    ..wgpu::MultisampleState::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry.1),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
    }
}

fn instances(stride: usize, attributes: &[wgpu::VertexAttribute]) -> wgpu::VertexBufferLayout<'_> {
    wgpu::VertexBufferLayout {
        array_stride: stride as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes,
    }
}

fn layout_entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty,
        count: None,
    }
}

const UNIFORM: wgpu::BindingType = wgpu::BindingType::Buffer {
    ty: wgpu::BufferBindingType::Uniform,
    has_dynamic_offset: false,
    min_binding_size: None,
};

impl Pipelines {
    /// The pipelines for an output of `format` drawn with `samples` samples per pixel.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let module = |label, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(
                    [include_str!("../common.wgsl"), source].concat().into(),
                ),
            })
        };
        let world_shader = module("world", include_str!("../world.wgsl"));
        let detail_shader = module("detail", include_str!("../shader.wgsl"));
        let bind_layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let globals_layout = bind_layout("globals", &[layout_entry(0, UNIFORM)]);
        let rooms_layout = bind_layout(
            "rooms",
            &[
                layout_entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    2,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        );
        let locals_layout = bind_layout("locals", &[layout_entry(0, UNIFORM)]);
        let pipeline_layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let flat = pipeline_layout("flat", &[Some(&globals_layout)]);
        let rooms_pipeline_layout =
            pipeline_layout("rooms", &[Some(&globals_layout), Some(&rooms_layout)]);
        let detail = pipeline_layout("detail", &[Some(&globals_layout), Some(&locals_layout)]);
        let builder = Builder {
            device,
            format,
            samples,
        };
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let room_attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Uint32, 2 => Unorm8x4, 3 => Unorm8x4
        ];
        let rooms = builder.build(
            "rooms",
            &rooms_pipeline_layout,
            &world_shader,
            ("room_vertex", "room_fragment"),
            &[Some(instances(size_of::<RoomInstance>(), &room_attributes))],
            Depth::Ignore,
            None,
            None,
        );
        let point_attributes =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Unorm8x4];
        let points = builder.build(
            "minimap points",
            &flat,
            &world_shader,
            ("point_vertex", "point_fragment"),
            &[Some(instances(
                size_of::<PointInstance>(),
                &point_attributes,
            ))],
            Depth::Ignore,
            blend,
            None,
        );
        let terrain_vertex =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
        let detail_terrain = builder.build(
            "detail terrain",
            &detail,
            &detail_shader,
            ("terrain_vertex", "fragment"),
            &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<TerrainVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &terrain_vertex,
            })],
            Depth::Test,
            blend,
            Some(wgpu::Face::Back),
        );
        let shape = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let instance = wgpu::vertex_attr_array![2 => Float32x3, 3 => Float32x3, 4 => Float32x4];
        let object_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<ShapeVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &shape,
            }),
            Some(instances(size_of::<Instance>(), &instance)),
        ];
        let objects = builder.build(
            "objects",
            &detail,
            &detail_shader,
            ("object_vertex", "fragment"),
            &object_buffers,
            Depth::Test,
            blend,
            Some(wgpu::Face::Back),
        );
        let overlay = builder.build(
            "overlay",
            &detail,
            &detail_shader,
            ("object_vertex", "fragment"),
            &object_buffers,
            Depth::Ignore,
            None,
            Some(wgpu::Face::Back),
        );
        Self {
            globals_layout,
            rooms_layout,
            locals_layout,
            rooms,
            points,
            detail_terrain,
            objects,
            overlay,
        }
    }
}

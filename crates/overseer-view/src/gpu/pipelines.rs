//! Bind group layouts and render pipelines.

use crate::mesh::{
    ArcInstance, EffectInstance, FillVertex, GlyphInstance, Instance, LabelGlyph, MarkInstance,
    PointInstance, RoomInstance, ShapeVertex, TerrainVertex, Token, VisualInstance,
};
use crate::overlay::badges::{CELLS_PER_LAYER, COLUMNS, ROWS};

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
    /// Group 2 of text: the glyph atlas and its sampler.
    pub atlas_layout: wgpu::BindGroupLayout,
    /// The rooms: terrain (overview and full resolution), status, owner and borders.
    pub rooms: wgpu::RenderPipeline,
    /// Minimap points.
    pub points: wgpu::RenderPipeline,
    /// Raised walls of detail scenes (perspective).
    pub detail_terrain: wgpu::RenderPipeline,
    /// Object prisms (perspective).
    pub sides: wgpu::RenderPipeline,
    /// Shadows of objects with a prism.
    pub shadows: wgpu::RenderPipeline,
    /// Object tokens.
    pub tokens: wgpu::RenderPipeline,
    /// Action effects.
    pub effects: wgpu::RenderPipeline,
    /// Filled visual polygons.
    pub fills: wgpu::RenderPipeline,
    /// Visual lines, circles and rectangles.
    pub prims: wgpu::RenderPipeline,
    /// Visual text.
    pub glyphs: wgpu::RenderPipeline,
    /// Objects drawn over everything (the selection frame).
    pub overlay: wgpu::RenderPipeline,
    /// The overlay over the world: badges, portals, labels, the hover outline.
    pub overlays: Overlays,
}

/// The overlay's pipelines and the layouts of its groups 1 (the overlay's uniforms) and
/// 3 (the badge atlas); group 2 is the glyph atlas.
#[derive(Debug)]
pub struct Overlays {
    /// Group 1: the overlay's uniforms.
    pub uniforms_layout: wgpu::BindGroupLayout,
    /// Group 3: the badge atlas (2D array) and its sampler.
    pub badges_layout: wgpu::BindGroupLayout,
    /// Marks: badges, portals, the hover outline.
    pub marks: wgpu::RenderPipeline,
    /// Arcs between linked portals.
    pub arcs: wgpu::RenderPipeline,
    /// Labels.
    pub labels: wgpu::RenderPipeline,
}

impl Overlays {
    fn new(
        builder: &Builder<'_>,
        shader: &wgpu::ShaderModule,
        globals_layout: &wgpu::BindGroupLayout,
        atlas_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let device = builder.device;
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay"),
            entries: &[layout_entry(0, UNIFORM)],
        });
        let badges_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("badges"),
            entries: &[
                layout_entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                layout_entry(1, SAMPLER),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[
                Some(globals_layout),
                Some(&uniforms_layout),
                Some(atlas_layout),
                Some(&badges_layout),
            ],
            immediate_size: 0,
        });
        let premultiplied = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let build = |label, entry, buffer: wgpu::VertexBufferLayout<'_>| {
            builder.build(
                &Stage {
                    label,
                    layout: &layout,
                    shader,
                    entry,
                    buffers: &[Some(buffer)],
                },
                Depth::Ignore,
                premultiplied,
                None,
            )
        };
        let mark_attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x3, 2 => Float32x4, 3 => Unorm8x4, 4 => Uint32
        ];
        let marks = build(
            "overlay marks",
            ("mark_vertex", "mark_fragment"),
            instances(size_of::<MarkInstance>(), &mark_attributes),
        );
        let arc_attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4, 3 => Float32, 4 => Uint32
        ];
        let arcs = build(
            "overlay arcs",
            ("arc_vertex", "arc_fragment"),
            instances(size_of::<ArcInstance>(), &arc_attributes),
        );
        let label_attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Unorm8x4, 4 => Unorm8x4,
            5 => Float32, 6 => Float32, 7 => Uint32
        ];
        let labels = build(
            "overlay labels",
            ("label_vertex", "label_fragment"),
            instances(size_of::<LabelGlyph>(), &label_attributes),
        );
        Self {
            uniforms_layout,
            badges_layout,
            marks,
            arcs,
            labels,
        }
    }
}

/// How a pipeline treats depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Depth {
    /// Flat layers: drawn in order, no depth test or write.
    Ignore,
    /// Solid geometry of detail scenes: tested and written.
    Write,
    /// Blended layers of detail scenes: tested, not written.
    Test,
}

/// What a pipeline draws with.
struct Stage<'a> {
    label: &'a str,
    layout: &'a wgpu::PipelineLayout,
    shader: &'a wgpu::ShaderModule,
    entry: (&'a str, &'a str),
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
}

/// What the pipelines of one layout share.
struct Builder<'a> {
    device: &'a wgpu::Device,
    format: wgpu::TextureFormat,
    samples: u32,
}

impl Builder<'_> {
    fn build(
        &self,
        stage: &Stage<'_>,
        depth: Depth,
        blend: Option<wgpu::BlendState>,
        cull: Option<wgpu::Face>,
    ) -> wgpu::RenderPipeline {
        self.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(stage.label),
                layout: Some(stage.layout),
                vertex: wgpu::VertexState {
                    module: stage.shader,
                    entry_point: Some(stage.entry.0),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: stage.buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: cull,
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(depth == Depth::Write),
                    depth_compare: Some(if depth == Depth::Ignore {
                        wgpu::CompareFunction::Always
                    } else {
                        wgpu::CompareFunction::LessEqual
                    }),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: self.samples,
                    ..wgpu::MultisampleState::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: stage.shader,
                    entry_point: Some(stage.entry.1),
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

fn vertices(stride: usize, attributes: &[wgpu::VertexAttribute]) -> wgpu::VertexBufferLayout<'_> {
    wgpu::VertexBufferLayout {
        array_stride: stride as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
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

const FLOAT_TEXTURE: wgpu::BindingType = wgpu::BindingType::Texture {
    sample_type: wgpu::TextureSampleType::Float { filterable: true },
    view_dimension: wgpu::TextureViewDimension::D2,
    multisampled: false,
};

const SAMPLER: wgpu::BindingType = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);

/// Premultiplied colour added to what is behind (glow).
const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

impl Pipelines {
    /// The pipelines for an output of `format` drawn with `samples` samples per pixel.
    #[expect(
        clippy::too_many_lines,
        reason = "one block per pipeline, read top to bottom"
    )]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let module = |label, sources: &[&str]| {
            let mut source = String::from(include_str!("../common.wgsl"));
            for part in sources {
                source.push_str(part);
            }
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let world_shader = module("world", &[include_str!("../world.wgsl")]);
        // The token shader finds a badge's cell in the atlas as `overlay/badges.rs` places it.
        let badge_cells = format!(
            "const BADGE_COLUMNS: u32 = {COLUMNS}u;\nconst BADGE_ROWS: u32 = {ROWS}u;\n\
             const BADGE_CELLS_PER_LAYER: u32 = {CELLS_PER_LAYER}u;\n"
        );
        let detail_shader = module(
            "detail",
            &[
                include_str!("../shader.wgsl"),
                badge_cells.as_str(),
                include_str!("../objects.wgsl"),
                include_str!("../effects.wgsl"),
                include_str!("../visuals.wgsl"),
            ],
        );
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
                layout_entry(1, FLOAT_TEXTURE),
                layout_entry(2, SAMPLER),
            ],
        );
        let locals_layout = bind_layout("locals", &[layout_entry(0, UNIFORM)]);
        let atlas_layout = bind_layout(
            "atlas",
            &[layout_entry(0, FLOAT_TEXTURE), layout_entry(1, SAMPLER)],
        );
        let pipeline_layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let rooms_pipeline_layout =
            pipeline_layout("rooms", &[Some(&globals_layout), Some(&rooms_layout)]);
        let detail = pipeline_layout("detail", &[Some(&globals_layout), Some(&locals_layout)]);
        let text = pipeline_layout(
            "text",
            &[
                Some(&globals_layout),
                Some(&locals_layout),
                Some(&atlas_layout),
            ],
        );
        let builder = Builder {
            device,
            format,
            samples,
        };
        let overlay_shader = module("overlay", &[include_str!("../overlay.wgsl")]);
        let overlays = Overlays::new(&builder, &overlay_shader, &globals_layout, &atlas_layout);
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let premultiplied = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);

        let room_attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Uint32, 2 => Unorm8x4, 3 => Unorm8x4
        ];
        let rooms = builder.build(
            &Stage {
                label: "rooms",
                layout: &rooms_pipeline_layout,
                shader: &world_shader,
                entry: ("room_vertex", "room_fragment"),
                buffers: &[Some(instances(size_of::<RoomInstance>(), &room_attributes))],
            },
            Depth::Ignore,
            None,
            None,
        );
        let point_attributes =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Unorm8x4];
        let points = builder.build(
            &Stage {
                label: "minimap points",
                layout: &detail,
                shader: &detail_shader,
                entry: ("point_vertex", "fragment"),
                buffers: &[Some(instances(
                    size_of::<PointInstance>(),
                    &point_attributes,
                ))],
            },
            Depth::Ignore,
            blend,
            None,
        );
        let terrain_vertex =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
        let detail_terrain = builder.build(
            &Stage {
                label: "detail terrain",
                layout: &detail,
                shader: &detail_shader,
                entry: ("terrain_vertex", "fragment"),
                buffers: &[Some(vertices(size_of::<TerrainVertex>(), &terrain_vertex))],
            },
            Depth::Write,
            blend,
            Some(wgpu::Face::Back),
        );
        let shape = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let instance = wgpu::vertex_attr_array![2 => Float32x3, 3 => Float32x3, 4 => Float32x4];
        let prism_buffers = [
            Some(vertices(size_of::<ShapeVertex>(), &shape)),
            Some(instances(size_of::<Instance>(), &instance)),
        ];
        let prism = |label, depth, blend| {
            builder.build(
                &Stage {
                    label,
                    layout: &detail,
                    shader: &detail_shader,
                    entry: ("object_vertex", "fragment"),
                    buffers: &prism_buffers,
                },
                depth,
                blend,
                Some(wgpu::Face::Back),
            )
        };
        let sides = prism("sides", Depth::Write, blend);
        let overlay = prism("overlay", Depth::Ignore, None);
        let token_attributes = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32, 2 => Uint32, 3 => Unorm8x4, 4 => Unorm8x4,
            5 => Uint8x4, 6 => Uint8x4, 7 => Uint8x4, 8 => Float32, 9 => Float32, 10 => Uint32
        ];
        let token_buffers = [Some(instances(size_of::<Token>(), &token_attributes))];
        // Group 2 is the glyph atlas of the same shader module's text; tokens sample the
        // badge atlas of group 3.
        let tokens_layout = pipeline_layout(
            "tokens",
            &[
                Some(&globals_layout),
                Some(&locals_layout),
                Some(&atlas_layout),
                Some(&overlays.badges_layout),
            ],
        );
        let token = |label, entry| {
            builder.build(
                &Stage {
                    label,
                    layout: &tokens_layout,
                    shader: &detail_shader,
                    entry,
                    buffers: &token_buffers,
                },
                Depth::Test,
                premultiplied,
                None,
            )
        };
        let shadows = token("shadows", ("shadow_vertex", "shadow_fragment"));
        let tokens = token("tokens", ("token_vertex", "token_fragment"));
        let effect_attributes = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32, 2 => Float32x3, 3 => Float32, 4 => Unorm8x4,
            5 => Uint32
        ];
        let effects = builder.build(
            &Stage {
                label: "effects",
                layout: &detail,
                shader: &detail_shader,
                entry: ("effect_vertex", "effect_fragment"),
                buffers: &[Some(instances(
                    size_of::<EffectInstance>(),
                    &effect_attributes,
                ))],
            },
            Depth::Ignore,
            Some(ADDITIVE),
            None,
        );
        let fill_attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Unorm8x4];
        let fills = builder.build(
            &Stage {
                label: "visual fills",
                layout: &detail,
                shader: &detail_shader,
                entry: ("fill_vertex", "fill_fragment"),
                buffers: &[Some(vertices(size_of::<FillVertex>(), &fill_attributes))],
            },
            Depth::Ignore,
            premultiplied,
            None,
        );
        let prim_attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32, 4 => Unorm8x4,
            5 => Unorm8x4, 6 => Uint32
        ];
        let prims = builder.build(
            &Stage {
                label: "visual shapes",
                layout: &detail,
                shader: &detail_shader,
                entry: ("prim_vertex", "prim_fragment"),
                buffers: &[Some(instances(
                    size_of::<VisualInstance>(),
                    &prim_attributes,
                ))],
            },
            Depth::Ignore,
            premultiplied,
            None,
        );
        let glyph_attributes = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Unorm8x4, 3 => Unorm8x4, 4 => Float32
        ];
        let glyphs = builder.build(
            &Stage {
                label: "visual text",
                layout: &text,
                shader: &detail_shader,
                entry: ("glyph_vertex", "glyph_fragment"),
                buffers: &[Some(instances(
                    size_of::<GlyphInstance>(),
                    &glyph_attributes,
                ))],
            },
            Depth::Ignore,
            premultiplied,
            None,
        );
        Self {
            globals_layout,
            rooms_layout,
            locals_layout,
            atlas_layout,
            rooms,
            points,
            detail_terrain,
            sides,
            shadows,
            tokens,
            effects,
            fills,
            prims,
            glyphs,
            overlay,
            overlays,
        }
    }
}

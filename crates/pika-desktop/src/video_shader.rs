use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use iced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::Rectangle;

use crate::video::DecodedFrame;

/// Shader-based video renderer that maintains a persistent GPU texture.
///
/// Unlike `iced::widget::image` which recreates GPU textures on every
/// `Handle::from_rgba()` call (causing flicker), this renders by updating
/// the texture in-place via `queue.write_texture()`.
pub struct VideoShaderProgram {
    frame_data: Arc<Mutex<Option<DecodedFrame>>>,
    generation: Arc<AtomicU64>,
}

impl VideoShaderProgram {
    pub fn new(frame_data: Arc<Mutex<Option<DecodedFrame>>>, generation: Arc<AtomicU64>) -> Self {
        Self {
            frame_data,
            generation,
        }
    }
}

impl<Message: Send + 'static> shader::Program<Message> for VideoShaderProgram {
    type State = ();
    type Primitive = VideoFramePrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        let gen = self.generation.load(Ordering::Relaxed);
        let (width, height) = self
            .frame_data
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|f| (f.width, f.height)))
            .unwrap_or((1, 1));

        VideoFramePrimitive {
            frame_data: self.frame_data.clone(),
            generation: gen,
            video_width: width,
            video_height: height,
        }
    }
}

/// Carries per-frame data for the GPU upload.
pub struct VideoFramePrimitive {
    frame_data: Arc<Mutex<Option<DecodedFrame>>>,
    generation: u64,
    video_width: u32,
    video_height: u32,
}

impl std::fmt::Debug for VideoFramePrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFramePrimitive")
            .field("generation", &self.generation)
            .field("video_width", &self.video_width)
            .field("video_height", &self.video_height)
            .finish()
    }
}

impl shader::Primitive for VideoFramePrimitive {
    type Pipeline = VideoFramePipeline;

    fn prepare(
        &self,
        pipeline: &mut VideoFramePipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        // Always update aspect-ratio uniforms (widget may have resized).
        pipeline.update_uniforms(
            queue,
            self.video_width,
            self.video_height,
            bounds.width,
            bounds.height,
        );

        // Only upload texture data when a new frame arrives.
        if self.generation == pipeline.last_generation && pipeline.last_generation != 0 {
            return;
        }

        let Ok(slot) = self.frame_data.lock() else {
            return;
        };
        let Some(frame) = slot.as_ref() else { return };

        // Recreate texture if video dimensions changed.
        if frame.width != pipeline.texture_width || frame.height != pipeline.texture_height {
            pipeline.recreate_texture(device, frame.width, frame.height);
        }

        // Upload RGBA data to the persistent GPU texture.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &pipeline.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * frame.width),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );

        pipeline.last_generation = self.generation;
    }

    fn render(
        &self,
        pipeline: &VideoFramePipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("video_frame_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_viewport(
            clip_bounds.x as f32,
            clip_bounds.y as f32,
            clip_bounds.width as f32,
            clip_bounds.height as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(&pipeline.render_pipeline);
        pass.set_bind_group(0, &pipeline.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Persistent GPU resources for video rendering — created once by iced.
pub struct VideoFramePipeline {
    render_pipeline: wgpu::RenderPipeline,
    texture: wgpu::Texture,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    texture_width: u32,
    texture_height: u32,
    last_generation: u64,
}

impl VideoFramePipeline {
    fn recreate_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video_frame_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video_frame_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });
        self.texture_width = width;
        self.texture_height = height;
    }

    fn update_uniforms(
        &self,
        queue: &wgpu::Queue,
        video_width: u32,
        video_height: u32,
        widget_width: f32,
        widget_height: f32,
    ) {
        let video_aspect = video_width as f32 / video_height.max(1) as f32;
        let widget_aspect = widget_width / widget_height.max(1.0);

        let (scale_x, scale_y, offset_x, offset_y) = if video_aspect > widget_aspect {
            // Video is wider — letterbox (black bars top/bottom).
            let sy = widget_aspect / video_aspect;
            (1.0, sy, 0.0, (1.0 - sy) / 2.0)
        } else {
            // Video is taller — pillarbox (black bars left/right).
            let sx = video_aspect / widget_aspect;
            (sx, 1.0, (1.0 - sx) / 2.0, 0.0)
        };

        let data: [f32; 4] = [scale_x, scale_y, offset_x, offset_y];
        let bytes = unsafe {
            std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(&data))
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytes);
    }
}

impl shader::Pipeline for VideoFramePipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        // Initial 1x1 texture (will be resized on first frame).
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video_frame_texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video_frame_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("video_uniform_buffer"),
            size: 16, // 4 x f32
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video_frame_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video_frame_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video_frame_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("video_frame_shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "shaders/video.wgsl"
            ))),
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video_frame_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        Self {
            render_pipeline,
            texture,
            sampler,
            bind_group_layout,
            bind_group,
            uniform_buffer,
            texture_width: 1,
            texture_height: 1,
            last_generation: 0,
        }
    }
}

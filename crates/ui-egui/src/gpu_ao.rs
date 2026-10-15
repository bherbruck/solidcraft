//! The optional AO depth prepass. Only opaque model/preview triangles participate.
use super::{Batches, color_layout, planned_layout, tri_layout, wgpu};

pub(super) struct Resources {
    pub layout: wgpu::BindGroupLayout,
    pub bind: wgpu::BindGroup,
    depth: wgpu::TextureView,
    size: [u32; 2],
    pipeline: wgpu::RenderPipeline,
    planned_pipeline: wgpu::RenderPipeline,
}

fn texture(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, size: [u32; 2]) -> (wgpu::TextureView, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sc_ao_depth"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let depth = texture.create_view(&Default::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sc_ao_depth"),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth) }],
    });
    (depth, bind)
}

impl Resources {
    pub fn new(device: &wgpu::Device, uniform: &wgpu::BindGroupLayout, module: &wgpu::ShaderModule) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sc_ao_depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc_ao_prepass"),
            bind_group_layouts: &[Some(uniform)],
            immediate_size: 0,
        });
        let built = [Some(tri_layout())];
        let evaluated = [Some(planned_layout()), Some(color_layout())];
        let pipeline = |planned: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sc_ao_prepass"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_tri"),
                    buffers: if planned { &evaluated } else { &built },
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_ao_depth"),
                    targets: &[],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let planned_pipeline = pipeline(true);
        let pipeline = pipeline(false);
        let (depth, bind) = texture(device, &layout, [1, 1]);
        Self { layout, bind, depth, size: [1, 1], pipeline, planned_pipeline }
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        uniform: &wgpu::BindGroup,
        radius: f32,
        size_px: [f32; 2],
        batches: [&Batches; 2],
    ) {
        let enabled = radius.is_finite() && radius > 0.0;
        // Bound memory at large window sizes; normalized coordinates also handle downsampling.
        let size = target_size(enabled, size_px, device.limits().max_texture_dimension_2d);
        if size != self.size {
            (self.depth, self.bind) = texture(device, &self.layout, size);
            self.size = size;
        }
        if !enabled {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sc_ao_prepass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, uniform, &[]);
        for batch in batches {
            pass.set_pipeline(&self.pipeline);
            if let Some(tris) = &batch.tris {
                for (buffer, count) in &tris.parts {
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
            pass.set_pipeline(&self.planned_pipeline);
            for draw in &batch.planned {
                pass.set_vertex_buffer(0, draw.mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, draw.colors.slice(..));
                pass.set_index_buffer(draw.mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..draw.mesh.index_count, 0, 0..1);
            }
        }
    }
}

fn target_size(enabled: bool, pixels: [f32; 2], device_limit: u32) -> [u32; 2] {
    if !enabled {
        return [1, 1];
    }
    let limit = device_limit.clamp(1, 2048);
    let pixels = pixels.map(|s| if s.is_finite() { s.max(1.0) } else { 1.0 });
    let scale = (limit as f32 / pixels[0].max(pixels[1])).min(1.0);
    pixels.map(|s| (s * scale).ceil().clamp(1.0, limit as f32) as u32)
}

#[cfg(test)]
mod tests {
    use super::target_size;
    #[test]
    fn depth_map_is_bounded_and_released_when_disabled() {
        assert_eq!(target_size(true, [6000.0, 3000.0], 8192), [2048, 1024]);
        assert_eq!(target_size(true, [6000.0, 3000.0], 1024), [1024, 512]);
        assert_eq!(target_size(false, [6000.0, 3000.0], 8192), [1, 1]);
        assert_eq!(target_size(true, [f32::NAN, f32::INFINITY], 8192), [1, 1]);
        assert_eq!(target_size(true, [0.0, -1.0], 8192), [1, 1]);
    }
}

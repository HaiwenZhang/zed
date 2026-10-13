#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{Context as _, Result};
    use block2::RcBlock;
    use foreign_types::ForeignTypeRef;
    use gpui::{
        Bounds, Context, GpuBackend, GpuCapabilities, GpuDrawMode, GpuPaintContext, GpuPaintTarget,
        GpuPainter, GpuPainterHandle, GpuPassContext, GpuPrepareContext, GpuResetReason,
        IntoElement, Render, Styled, Window, WindowBounds, WindowOptions, canvas, div, prelude::*,
        px, rgb, size,
    };
    #[cfg(test)]
    use gpui::{MetalDrawContext, MetalPaintContext, MetalPrepareContext};
    use gpui_util::ResultExt;
    use metal::{
        MTLBlendFactor, MTLClearColor, MTLCompareFunction, MTLLoadAction, MTLPixelFormat,
        MTLPrimitiveType, MTLScissorRect, MTLStorageMode, MTLStoreAction, MTLTextureType,
        MTLTextureUsage, MTLViewport,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };

    #[derive(Clone, Copy, PartialEq)]
    enum RenderMode {
        Direct,
        Offscreen,
    }

    #[derive(Clone, Copy, PartialEq)]
    struct Snapshot {
        mode: RenderMode,
        angle: f32,
    }

    #[derive(Default)]
    struct CadPainter {
        resources: Option<CadResources>,
    }

    struct CadResources {
        color_format: MTLPixelFormat,
        window_samples: u32,
        offscreen_samples: u32,
        direct_pipeline: metal::RenderPipelineState,
        offscreen_pipeline: metal::RenderPipelineState,
        composite_pipeline: metal::RenderPipelineState,
        depth_state: metal::DepthStencilState,
        direct_depth: Option<metal::Texture>,
        offscreen: Option<OffscreenTarget>,
    }

    struct OffscreenTarget {
        color: metal::Texture,
        multisample_color: Option<metal::Texture>,
        depth: metal::Texture,
        snapshot: Option<Snapshot>,
        completed: Arc<AtomicBool>,
    }

    fn texture(
        device: &metal::DeviceRef,
        size: [u32; 2],
        format: MTLPixelFormat,
        samples: u32,
        usage: MTLTextureUsage,
    ) -> metal::Texture {
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_width(size[0] as u64);
        descriptor.set_height(size[1] as u64);
        descriptor.set_pixel_format(format);
        descriptor.set_sample_count(samples as u64);
        descriptor.set_texture_type(if samples > 1 {
            MTLTextureType::D2Multisample
        } else {
            MTLTextureType::D2
        });
        descriptor.set_storage_mode(MTLStorageMode::Private);
        descriptor.set_usage(usage);
        device.new_texture(&descriptor)
    }

    fn pipeline(
        device: &metal::DeviceRef,
        library: &metal::LibraryRef,
        vertex: &str,
        fragment: &str,
        color_format: MTLPixelFormat,
        samples: u32,
        depth: bool,
    ) -> Result<metal::RenderPipelineState> {
        let descriptor = metal::RenderPipelineDescriptor::new();
        let vertex = library
            .get_function(vertex, None)
            .map_err(anyhow::Error::msg)?;
        let fragment = library
            .get_function(fragment, None)
            .map_err(anyhow::Error::msg)?;
        descriptor.set_vertex_function(Some(&vertex));
        descriptor.set_fragment_function(Some(&fragment));
        descriptor.set_sample_count(samples as u64);
        if depth {
            descriptor.set_depth_attachment_pixel_format(MTLPixelFormat::Depth32Float);
        }
        let color = descriptor
            .color_attachments()
            .object_at(0)
            .context("color attachment")?;
        color.set_pixel_format(color_format);
        color.set_blending_enabled(true);
        color.set_source_rgb_blend_factor(MTLBlendFactor::One);
        color.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        color.set_source_alpha_blend_factor(MTLBlendFactor::One);
        color.set_destination_alpha_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        device
            .new_render_pipeline_state(&descriptor)
            .map_err(anyhow::Error::msg)
    }

    impl CadPainter {
        fn resources(
            &mut self,
            device: &metal::DeviceRef,
            color_format: MTLPixelFormat,
            window_samples: u32,
        ) -> Result<&mut CadResources> {
            if self.resources.as_ref().is_none_or(|resources| {
                resources.color_format != color_format || resources.window_samples != window_samples
            }) {
                let library = device
                    .new_library_with_source(
                        include_str!("cad_viewport.metal"),
                        &metal::CompileOptions::new(),
                    )
                    .map_err(anyhow::Error::msg)?;
                let offscreen_samples = if device.supports_texture_sample_count(4) {
                    4
                } else {
                    1
                };
                let depth_descriptor = metal::DepthStencilDescriptor::new();
                depth_descriptor.set_depth_compare_function(MTLCompareFunction::Less);
                depth_descriptor.set_depth_write_enabled(true);
                self.resources = Some(CadResources {
                    color_format,
                    window_samples,
                    offscreen_samples,
                    direct_pipeline: pipeline(
                        device,
                        &library,
                        "cube_vertex",
                        "cube_fragment",
                        color_format,
                        window_samples,
                        true,
                    )?,
                    offscreen_pipeline: pipeline(
                        device,
                        &library,
                        "cube_vertex",
                        "cube_fragment",
                        color_format,
                        offscreen_samples,
                        true,
                    )?,
                    composite_pipeline: pipeline(
                        device,
                        &library,
                        "composite_vertex",
                        "composite_fragment",
                        color_format,
                        window_samples,
                        false,
                    )?,
                    depth_state: device.new_depth_stencil_state(&depth_descriptor),
                    direct_depth: None,
                    offscreen: None,
                });
            }
            self.resources.as_mut().context("CAD resources missing")
        }
    }

    fn encode_cube(
        encoder: &metal::RenderCommandEncoderRef,
        pipeline: &metal::RenderPipelineStateRef,
        depth_state: &metal::DepthStencilStateRef,
        snapshot: Snapshot,
        aspect: f32,
        opacity: f32,
    ) {
        let parameters = [snapshot.angle, aspect, opacity, 0.0];
        encoder.set_render_pipeline_state(pipeline);
        encoder.set_depth_stencil_state(depth_state);
        encoder.set_vertex_bytes(
            0,
            std::mem::size_of_val(&parameters) as u64,
            parameters.as_ptr().cast(),
        );
        encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 36);
    }

    fn viewport(target: GpuPaintTarget) -> MTLViewport {
        MTLViewport {
            originX: target.bounds.origin.x.0 as f64,
            originY: target.bounds.origin.y.0 as f64,
            width: target.bounds.size.width.0 as f64,
            height: target.bounds.size.height.0 as f64,
            znear: 0.0,
            zfar: 1.0,
        }
    }

    fn scissor(target: GpuPaintTarget) -> MTLScissorRect {
        let [left, top, width, height] = target.scissor_rect();
        MTLScissorRect {
            x: left as u64,
            y: top as u64,
            width: width as u64,
            height: height as u64,
        }
    }

    impl GpuPainter for CadPainter {
        type Snapshot = Snapshot;
        fn supports_backend(&self, backend: GpuBackend) -> bool {
            backend == GpuBackend::Metal
        }
        fn prepare(
            &mut self,
            context: &mut GpuPrepareContext<'_>,
            snapshot: &Snapshot,
        ) -> Result<()> {
            let context = context.metal()?;
            let snapshot = *snapshot;
            if snapshot.mode == RenderMode::Direct {
                return Ok(());
            }
            let resources = self.resources(
                context.device,
                context.color_format,
                context.target.sample_count,
            )?;
            let size = [
                context.target.bounds.size.width.0.ceil().max(1.0) as u32,
                context.target.bounds.size.height.0.ceil().max(1.0) as u32,
            ];
            if resources.offscreen.as_ref().is_none_or(|target| {
                target.color.width() != size[0] as u64 || target.color.height() != size[1] as u64
            }) {
                resources.offscreen = Some(OffscreenTarget {
                    color: texture(
                        context.device,
                        size,
                        context.color_format,
                        1,
                        MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
                    ),
                    multisample_color: (resources.offscreen_samples > 1).then(|| {
                        texture(
                            context.device,
                            size,
                            context.color_format,
                            resources.offscreen_samples,
                            MTLTextureUsage::RenderTarget,
                        )
                    }),
                    depth: texture(
                        context.device,
                        size,
                        MTLPixelFormat::Depth32Float,
                        resources.offscreen_samples,
                        MTLTextureUsage::RenderTarget,
                    ),
                    snapshot: None,
                    completed: Arc::new(AtomicBool::new(false)),
                });
            }
            let target = resources
                .offscreen
                .as_mut()
                .context("offscreen target missing")?;
            if target.snapshot == Some(snapshot) && target.completed.load(Ordering::Acquire) {
                return Ok(());
            }
            let descriptor = metal::RenderPassDescriptor::new();
            let color = descriptor
                .color_attachments()
                .object_at(0)
                .context("color attachment")?;
            color.set_load_action(MTLLoadAction::Clear);
            color.set_clear_color(MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
            if let Some(multisample_color) = &target.multisample_color {
                color.set_texture(Some(multisample_color));
                color.set_resolve_texture(Some(&target.color));
                color.set_store_action(MTLStoreAction::MultisampleResolve);
            } else {
                color.set_texture(Some(&target.color));
                color.set_store_action(MTLStoreAction::Store);
            }
            let depth = descriptor.depth_attachment().context("depth attachment")?;
            depth.set_texture(Some(&target.depth));
            depth.set_load_action(MTLLoadAction::Clear);
            depth.set_clear_depth(1.0);
            depth.set_store_action(MTLStoreAction::DontCare);
            let encoder = context
                .command_buffer
                .new_render_command_encoder(&descriptor);
            encode_cube(
                encoder,
                &resources.offscreen_pipeline,
                &resources.depth_state,
                snapshot,
                size[0] as f32 / size[1] as f32,
                1.0,
            );
            encoder.end_encoding();
            target.snapshot = Some(snapshot);
            // Encoding may be retried or abandoned before submission, so the cache becomes valid only after GPU completion.
            target.completed = Arc::new(AtomicBool::new(false));
            let completed = target.completed.clone();
            let handler = RcBlock::new(
                move |command_buffer: std::ptr::NonNull<objc2::runtime::AnyObject>| {
                    // Metal supplies a live command buffer for the duration of this completion callback.
                    let command_buffer = unsafe {
                        metal::CommandBufferRef::from_ptr(command_buffer.as_ptr().cast())
                    };
                    completed.store(
                        command_buffer.status() == metal::MTLCommandBufferStatus::Completed,
                        Ordering::Release,
                    );
                },
            );
            // Both block types are opaque views of the same Objective-C block ABI.
            unsafe {
                context
                    .command_buffer
                    .add_completed_handler(&*RcBlock::as_ptr(&handler).cast());
            }
            Ok(())
        }

        fn draw_mode(&self, _: GpuCapabilities, snapshot: &Snapshot) -> GpuDrawMode {
            match snapshot.mode {
                RenderMode::Offscreen => GpuDrawMode::SharedPass,
                RenderMode::Direct => GpuDrawMode::OwnPass,
            }
        }

        fn draw_in_pass(
            &mut self,
            context: &mut GpuPassContext<'_, '_>,
            _snapshot: &Snapshot,
        ) -> Result<()> {
            let context = context.metal()?;
            let resources = self.resources(
                context.device,
                context.color_format,
                context.target.sample_count,
            )?;
            let offscreen = resources
                .offscreen
                .as_ref()
                .context("offscreen target was not prepared")?;
            context
                .encoder
                .set_render_pipeline_state(&resources.composite_pipeline);
            context
                .encoder
                .set_fragment_texture(0, Some(&offscreen.color));
            let opacity = context.target.opacity;
            context.encoder.set_fragment_bytes(
                0,
                std::mem::size_of_val(&opacity) as u64,
                (&opacity as *const f32).cast(),
            );
            context
                .encoder
                .draw_primitives(MTLPrimitiveType::Triangle, 0, 6);
            Ok(())
        }

        fn paint(&mut self, context: &mut GpuPaintContext<'_>, snapshot: &Snapshot) -> Result<()> {
            let context = context.metal()?;
            let snapshot = *snapshot;
            anyhow::ensure!(
                snapshot.mode == RenderMode::Direct,
                "offscreen composition requires shared-pass access"
            );
            let resources = self.resources(
                context.device,
                context.color_format,
                context.target.sample_count,
            )?;
            if resources.direct_depth.as_ref().is_none_or(|depth| {
                depth.width() != context.target.size[0] as u64
                    || depth.height() != context.target.size[1] as u64
            }) {
                resources.direct_depth = Some(texture(
                    context.device,
                    context.target.size,
                    MTLPixelFormat::Depth32Float,
                    context.target.sample_count,
                    MTLTextureUsage::RenderTarget,
                ));
            }
            let descriptor = metal::RenderPassDescriptor::new();
            let color = descriptor
                .color_attachments()
                .object_at(0)
                .context("color attachment")?;
            color.set_texture(Some(context.color_target));
            color.set_load_action(MTLLoadAction::Load);
            color.set_store_action(MTLStoreAction::Store);
            let depth = descriptor.depth_attachment().context("depth attachment")?;
            depth.set_texture(resources.direct_depth.as_deref());
            depth.set_load_action(MTLLoadAction::Clear);
            depth.set_clear_depth(1.0);
            depth.set_store_action(MTLStoreAction::DontCare);
            let clip = scissor(context.target);
            if clip.width == 0 || clip.height == 0 {
                return Ok(());
            }
            let encoder = context
                .command_buffer
                .new_render_command_encoder(&descriptor);
            encoder.set_viewport(viewport(context.target));
            encoder.set_scissor_rect(clip);
            encode_cube(
                encoder,
                &resources.direct_pipeline,
                &resources.depth_state,
                snapshot,
                context.target.bounds.size.width.0 / context.target.bounds.size.height.0,
                context.target.opacity,
            );
            encoder.end_encoding();
            Ok(())
        }

        fn reset(&mut self, _reason: GpuResetReason) {
            self.resources = None;
        }
    }

    struct CadView {
        painter: Option<GpuPainterHandle<Snapshot>>,
        error: Option<String>,
        snapshot: Snapshot,
    }

    struct CadViewport {
        views: [CadView; 2],
        animate: bool,
        last_frame: Instant,
    }

    impl Render for CadViewport {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let now = Instant::now();
            if self.animate {
                let angle_change = now.duration_since(self.last_frame).as_secs_f32() * 0.7;
                for view in &mut self.views {
                    view.snapshot.angle += angle_change;
                }
                window.request_animation_frame();
            }
            self.last_frame = now;
            div().size_full().flex().flex_col().gap_3().p_4().bg(rgb(0x20252c)).text_color(rgb(0xffffff))
                .child(div().id("animation").cursor_pointer().child(if self.animate { "Pause" } else { "Animate" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.animate = !this.animate;
                        this.last_frame = Instant::now();
                        cx.notify();
                    })))
                .child("Click each viewport's mode to switch. Paused offscreen frames reuse the cached texture.")
                .child(div().flex().flex_1().gap_4().children(self.views.iter().enumerate().map(|(index, view)| {
                    let painter = view.painter.clone();
                    let snapshot = view.snapshot;
                    div().flex().flex_col().flex_1().gap_2().overflow_hidden()
                        .child(div().id(("mode", index)).cursor_pointer()
                            .child(match snapshot.mode { RenderMode::Direct => "Direct rendering", RenderMode::Offscreen => "Offscreen + shared-pass composition" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(view) = this.views.get_mut(index) {
                                    view.snapshot.mode = match view.snapshot.mode { RenderMode::Direct => RenderMode::Offscreen, RenderMode::Offscreen => RenderMode::Direct };
                                    cx.notify();
                                }
                            })))
                        .child(view.error.clone().or_else(|| view.painter.as_ref().and_then(|painter| painter.status().last_error)).unwrap_or_default())
                        .child(div().flex_1().overflow_hidden().bg(rgb(0x303945))
                            .child(canvas(|_, _, _| (), move |bounds, _, window, _| {
                                if let Some(painter) = &painter {
                                    window.paint_gpu(bounds, painter, Arc::new(snapshot)).log_err();
                                }
                            }).size_full()))
                })))
        }
    }

    pub fn run() {
        gpui_platform::application().run(|cx| {
            let bounds = Bounds::centered(None, size(px(1000.0), px(600.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let views = [RenderMode::Direct, RenderMode::Offscreen].map(|mode| {
                        let painter = window.register_gpu_painter(CadPainter::default());
                        let error = painter.as_ref().err().map(|error| format!("{error:#}"));
                        CadView {
                            painter: painter.log_err(),
                            error,
                            snapshot: Snapshot {
                                mode,
                                angle: if mode == RenderMode::Direct { 0.0 } else { 0.9 },
                            },
                        }
                    });
                    cx.new(|_| CadViewport {
                        views,
                        animate: true,
                        last_frame: Instant::now(),
                    })
                },
            )
            .log_err();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use gpui::{ScaledPixels, point};

        fn test_target(edge: u32, opacity: f32) -> GpuPaintTarget {
            GpuPaintTarget {
                size: [edge, edge],
                bounds: Bounds::new(
                    point(ScaledPixels(0.0), ScaledPixels(0.0)),
                    gpui::size(ScaledPixels(edge as f32), ScaledPixels(edge as f32)),
                ),
                clip: Bounds::new(
                    point(ScaledPixels(16.0), ScaledPixels(16.0)),
                    gpui::size(
                        ScaledPixels((edge - 32) as f32),
                        ScaledPixels((edge - 32) as f32),
                    ),
                ),
                scale_factor: 1.0,
                sample_count: 1,
                opacity,
            }
        }

        fn begin_pass<'frame>(
            command_buffer: &'frame metal::CommandBufferRef,
            color: &'frame metal::TextureRef,
            clear: bool,
        ) -> Result<&'frame metal::RenderCommandEncoderRef> {
            let descriptor = metal::RenderPassDescriptor::new();
            let attachment = descriptor
                .color_attachments()
                .object_at(0)
                .context("color attachment")?;
            attachment.set_texture(Some(color));
            attachment.set_load_action(if clear {
                MTLLoadAction::Clear
            } else {
                MTLLoadAction::Load
            });
            attachment.set_clear_color(MTLClearColor::new(0.1, 0.1, 0.1, 1.0));
            attachment.set_store_action(MTLStoreAction::Store);
            Ok(command_buffer.new_render_command_encoder(&descriptor))
        }

        fn render_views(
            device: &metal::DeviceRef,
            queue: &metal::CommandQueueRef,
            views: &mut [(&mut CadPainter, Snapshot, GpuPaintTarget)],
            dimensions: [u32; 2],
        ) -> Result<Vec<u8>> {
            let color = texture(
                device,
                dimensions,
                MTLPixelFormat::BGRA8Unorm,
                1,
                MTLTextureUsage::RenderTarget,
            );
            let command_buffer = queue.new_command_buffer();
            for (painter, snapshot, target) in views.iter_mut() {
                painter.prepare(
                    &mut GpuPrepareContext::Metal(MetalPrepareContext {
                        target: *target,
                        device,
                        command_buffer,
                        color_format: color.pixel_format(),
                    }),
                    snapshot,
                )?;
            }
            for (painter, snapshot, target) in views.iter_mut() {
                painter.finish_prepare(
                    &mut GpuPrepareContext::Metal(MetalPrepareContext {
                        target: *target,
                        device,
                        command_buffer,
                        color_format: color.pixel_format(),
                    }),
                    snapshot,
                )?;
            }
            let mut encoder = begin_pass(command_buffer, &color, true)?;
            for (painter, snapshot, target) in views.iter_mut() {
                let result = match painter.draw_mode(GpuBackend::Metal.capabilities(), snapshot) {
                    GpuDrawMode::SharedPass => {
                        encoder.set_viewport(viewport(*target));
                        encoder.set_scissor_rect(scissor(*target));
                        painter.draw_in_pass(
                            &mut GpuPassContext::Metal(&mut MetalDrawContext {
                                target: *target,
                                device,
                                encoder,
                                color_format: color.pixel_format(),
                            }),
                            snapshot,
                        )
                    }
                    GpuDrawMode::OwnPass => {
                        encoder.end_encoding();
                        let result = painter.paint(
                            &mut GpuPaintContext::Metal(MetalPaintContext {
                                target: *target,
                                device,
                                command_buffer,
                                color_target: &color,
                                color_format: color.pixel_format(),
                            }),
                            snapshot,
                        );
                        encoder = begin_pass(command_buffer, &color, false)?;
                        result
                    }
                };
                if let Err(error) = result {
                    encoder.end_encoding();
                    return Err(error);
                }
            }
            encoder.end_encoding();
            let row_bytes = dimensions[0] as u64 * 4;
            let byte_count = row_bytes * dimensions[1] as u64;
            let buffer =
                device.new_buffer(byte_count, metal::MTLResourceOptions::StorageModeShared);
            let blit = command_buffer.new_blit_command_encoder();
            blit.copy_from_texture_to_buffer(
                &color,
                0,
                0,
                metal::MTLOrigin { x: 0, y: 0, z: 0 },
                metal::MTLSize {
                    width: dimensions[0] as u64,
                    height: dimensions[1] as u64,
                    depth: 1,
                },
                &buffer,
                0,
                row_bytes,
                byte_count,
                metal::MTLBlitOption::None,
            );
            blit.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed();
            anyhow::ensure!(
                command_buffer.status() == metal::MTLCommandBufferStatus::Completed,
                "GPU rendering failed: {:?}",
                command_buffer.status()
            );
            // The completed copy initialized the whole buffer, and the GPU no longer writes to it.
            Ok(unsafe {
                std::slice::from_raw_parts(buffer.contents().cast::<u8>(), byte_count as usize)
            }
            .to_vec())
        }

        fn render(
            device: &metal::DeviceRef,
            queue: &metal::CommandQueueRef,
            painter: &mut CadPainter,
            snapshot: Snapshot,
            edge: u32,
            opacity: f32,
        ) -> Result<Vec<u8>> {
            render_views(
                device,
                queue,
                &mut [(painter, snapshot, test_target(edge, opacity))],
                [edge, edge],
            )
        }

        #[test]
        fn independent_viewports_keep_their_cameras_targets_and_render_modes() -> Result<()> {
            let device = metal::Device::system_default().context("Metal device unavailable")?;
            let queue = device.new_command_queue();
            let mut left = CadPainter::default();
            let mut right = CadPainter::default();
            let mut reference = CadPainter::default();
            let first = Snapshot {
                mode: RenderMode::Direct,
                angle: 0.0,
            };
            let second = Snapshot {
                mode: RenderMode::Offscreen,
                angle: 0.9,
            };
            let mut left_target = test_target(64, 1.0);
            left_target.size = [128, 64];
            let mut right_target = left_target;
            right_target.bounds.origin.x.0 += 64.0;
            right_target.clip.origin.x.0 += 64.0;
            let pixels = render_views(
                &device,
                &queue,
                &mut [
                    (&mut left, first, left_target),
                    (&mut right, second, right_target),
                ],
                [128, 64],
            )?;
            let expected = render(&device, &queue, &mut reference, second, 64, 1.0)?;
            let center = |pixels: &[u8], width: usize, column: usize| -> Result<[u8; 4]> {
                pixels
                    .get((32 * width + column) * 4..(32 * width + column) * 4 + 4)
                    .context("center pixel")?
                    .try_into()
                    .map_err(anyhow::Error::from)
            };
            assert_eq!(center(&pixels, 128, 96)?, center(&expected, 64, 32)?);
            assert_ne!(center(&pixels, 128, 32)?, center(&pixels, 128, 96)?);
            let changed = render_views(
                &device,
                &queue,
                &mut [
                    (
                        &mut left,
                        Snapshot {
                            mode: RenderMode::Offscreen,
                            angle: 1.4,
                        },
                        left_target,
                    ),
                    (&mut right, second, right_target),
                ],
                [128, 64],
            )?;
            assert_eq!(center(&changed, 128, 96)?, center(&pixels, 128, 96)?);
            assert_ne!(center(&changed, 128, 32)?, center(&pixels, 128, 32)?);
            Ok(())
        }

        #[test]
        fn direct_and_offscreen_rendering_preserve_depth_clipping_and_opacity() -> Result<()> {
            let device = metal::Device::system_default().context("Metal device unavailable")?;
            let queue = device.new_command_queue();
            let mut painter = CadPainter::default();
            for mode in [RenderMode::Direct, RenderMode::Offscreen] {
                let pixels = render(
                    &device,
                    &queue,
                    &mut painter,
                    Snapshot { mode, angle: 0.0 },
                    64,
                    0.5,
                )?;
                let center = pixels
                    .get((32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4)
                    .context("center pixel")?;
                // The blue near face is drawn before the red far face, so this checks depth occlusion as well as blending.
                for (actual, expected) in center.iter().zip([134, 96, 38, 255]) {
                    assert!(
                        (i32::from(*actual) - expected).abs() <= 2,
                        "unexpected center pixel: {center:?}"
                    );
                }
                for (x, y) in [(0, 0), (32, 10), (10, 32), (54, 32), (32, 54)] {
                    let pixel = pixels
                        .get((y * 64 + x) * 4..(y * 64 + x) * 4 + 4)
                        .context("background pixel")?;
                    assert_eq!(pixel, &[26, 26, 26, 255]);
                }
            }
            Ok(())
        }

        #[test]
        fn cached_offscreen_rendering_survives_abandoned_frames_resize_mode_switch_and_reset()
        -> Result<()> {
            let device = metal::Device::system_default().context("Metal device unavailable")?;
            let queue = device.new_command_queue();
            let mut painter = CadPainter::default();
            let snapshot = Snapshot {
                mode: RenderMode::Offscreen,
                angle: 0.0,
            };
            let abandoned = queue.new_command_buffer();
            painter.prepare(
                &mut GpuPrepareContext::Metal(MetalPrepareContext {
                    target: test_target(64, 1.0),
                    device: &device,
                    command_buffer: abandoned,
                    color_format: MTLPixelFormat::BGRA8Unorm,
                }),
                &snapshot,
            )?;
            let first = render(&device, &queue, &mut painter, snapshot, 64, 1.0)?;
            let center = first
                .get((32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4)
                .context("center pixel")?;
            assert!(
                center.first().copied().unwrap_or(0) > 200,
                "abandoned preparation must not be reused: {center:?}"
            );
            assert_eq!(
                first,
                render(&device, &queue, &mut painter, snapshot, 64, 1.0)?
            );
            render(
                &device,
                &queue,
                &mut painter,
                Snapshot {
                    mode: RenderMode::Direct,
                    ..snapshot
                },
                64,
                1.0,
            )?;
            assert_eq!(
                first,
                render(&device, &queue, &mut painter, snapshot, 64, 1.0)?
            );
            let rotated = render(
                &device,
                &queue,
                &mut painter,
                Snapshot {
                    angle: 0.9,
                    ..snapshot
                },
                64,
                1.0,
            )?;
            assert_ne!(first, rotated);
            assert_eq!(
                render(&device, &queue, &mut painter, snapshot, 96, 1.0)?.len(),
                96 * 96 * 4
            );
            painter.reset(GpuResetReason::DeviceReplaced);
            assert!(painter.resources.is_none());
            assert_eq!(
                first,
                render(&device, &queue, &mut painter, snapshot, 64, 1.0)?
            );
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The cad_viewport example uses the macOS Metal backend.");
}

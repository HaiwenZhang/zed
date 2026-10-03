use anyhow::Result;
use gpui::{GpuPaintTarget, GpuPainter, GpuResetReason};
use std::any::Any;

/// Native Metal resources borrowed for one drawing callback.
pub struct MetalPaintContext<'a> {
    /// Target geometry and inherited opacity for this drawing operation.
    pub target: GpuPaintTarget,
    /// Resources must be created on this device.
    pub device: &'a metal::DeviceRef,
    /// Begin and end application encoders here; never commit it.
    pub command_buffer: &'a metal::CommandBufferRef,
    /// Use Load/Store to preserve GPUI's existing color.
    pub color_target: &'a metal::TextureRef,
    /// Color interpretation used by the target.
    pub color_format: metal::MTLPixelFormat,
}

/// Application drawing using the window's Metal device and color target.
pub trait MetalPainter: Send + 'static {
    /// Encodes drawing into the current frame.
    ///
    /// Preserve existing color, restrict writes to `context.target.clip`, and apply
    /// `context.target.opacity`. End application encoders before returning, including
    /// on error; errors do not undo commands already encoded.
    ///
    /// Do not submit or present the frame, retain frame targets, use the context on
    /// another thread, or reenter GPUI. Success indicates encoding, not GPU completion.
    fn paint(
        &mut self,
        context: &mut MetalPaintContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()>;

    /// Releases resources before the window's device or renderer is retired.
    /// After device replacement, recreate resources in the next drawing callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// Adapts a Metal painter for [`gpui::Window::register_gpu_painter`].
pub fn metal_painter(painter: impl MetalPainter) -> impl GpuPainter {
    MetalPainterAdapter(Box::new(painter))
}

pub(crate) struct MetalPainterAdapter(pub(crate) Box<dyn MetalPainter>);

impl GpuPainter for MetalPainterAdapter {
    fn reset(&mut self, reason: GpuResetReason) {
        self.0.reset(reason);
    }
}

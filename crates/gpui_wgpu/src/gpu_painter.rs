use anyhow::Result;
use gpui::{GpuPaintTarget, GpuPainter, GpuResetReason};
use std::any::Any;

/// Native Wgpu resources borrowed before scene drawing begins.
///
/// Use application-owned targets for offscreen drawing. Do not submit or present,
/// retain frame resources, reenter GPUI, or leave an encoder active on return.
pub struct WgpuPrepareContext<'a> {
    /// Geometry and opacity of the surface that will be drawn later.
    pub target: GpuPaintTarget,
    /// Resources must be created on this device.
    pub device: &'a wgpu::Device,
    /// Queue for uploads. Do not submit command buffers or present the frame.
    pub queue: &'a wgpu::Queue,
    /// Encoder for uploads, compute work and offscreen passes; never submit it.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Color format of the eventual drawing target.
    pub color_format: wgpu::TextureFormat,
}

/// Native Wgpu resources borrowed for one drawing callback.
pub struct WgpuPaintContext<'a> {
    /// Target geometry and inherited opacity for this drawing operation.
    pub target: GpuPaintTarget,
    /// Resources must be created on this device.
    pub device: &'a wgpu::Device,
    /// Encode uploads and passes without submitting a separate buffer.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Use Load/Store to preserve GPUI's existing color.
    pub color_target: &'a wgpu::TextureView,
    /// Color interpretation used by the target.
    pub color_format: wgpu::TextureFormat,
}

/// Application drawing using the window's Wgpu device and color target.
pub trait WgpuPainter: Send + 'static {
    /// Prepares resources before any ordinary scene drawing.
    ///
    /// Called once per visible surface and frame encoding attempt, including scene
    /// replays. All surfaces prepare before any surface finishes preparation.
    /// Surfaces sharing this painter may have different snapshots: retain separate
    /// buffer ranges or resources when their draws need different prepared data.
    /// An error skips this surface's remaining stages and is recorded in its handle.
    fn prepare(
        &mut self,
        _context: &mut WgpuPrepareContext<'_>,
        _data: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        Ok(())
    }

    /// Runs after all surfaces have prepared, before ordinary scene drawing.
    ///
    /// Called once per surface whose preparation succeeded. An error skips its draw.
    fn finish_prepare(
        &mut self,
        _context: &mut WgpuPrepareContext<'_>,
        _data: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        Ok(())
    }

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
        context: &mut WgpuPaintContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()>;

    /// Releases resources before the window's device or renderer is retired.
    /// After device replacement, recreate resources in the next preparation or drawing callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// Adapts a Wgpu painter for [`gpui::Window::register_gpu_painter`].
pub fn wgpu_painter(painter: impl WgpuPainter) -> impl GpuPainter {
    WgpuPainterAdapter(Box::new(painter))
}

pub(crate) struct WgpuPainterAdapter(pub(crate) Box<dyn WgpuPainter>);

impl GpuPainter for WgpuPainterAdapter {
    fn reset(&mut self, reason: GpuResetReason) {
        self.0.reset(reason);
    }
}

use anyhow::Result;
use gpui::{GpuPaintTarget, GpuPainter, GpuResetReason};
use std::any::Any;

/// Native D3D11 resources borrowed for one drawing callback.
pub struct D3D11PaintContext<'a> {
    /// Target geometry and inherited opacity for this drawing operation.
    pub target: GpuPaintTarget,
    /// Resources must be created on this device.
    pub device: &'a windows::Win32::Graphics::Direct3D11::ID3D11Device,
    /// GPUI restores pipeline state after drawing.
    pub context: &'a windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    /// Bind with an application-owned depth/stencil view if needed.
    pub color_target: &'a windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView,
    /// Color interpretation used by the target.
    pub color_format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
}

/// Application drawing using the window's D3D11 device and color target.
pub trait D3D11Painter: Send + 'static {
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
        context: &mut D3D11PaintContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()>;

    /// Releases resources before the window's device or renderer is retired.
    /// After device replacement, recreate resources in the next drawing callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// Adapts a D3D11 painter for [`gpui::Window::register_gpu_painter`].
pub fn d3d11_painter(painter: impl D3D11Painter) -> impl GpuPainter {
    D3D11PainterAdapter(Box::new(painter))
}

pub(crate) struct D3D11PainterAdapter(pub(crate) Box<dyn D3D11Painter>);

impl GpuPainter for D3D11PainterAdapter {
    fn reset(&mut self, reason: GpuResetReason) {
        self.0.reset(reason);
    }
}

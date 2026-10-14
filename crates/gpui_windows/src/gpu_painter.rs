use anyhow::Result;
use gpui::{GpuPaintTarget, GpuPainter, GpuPainterBackend, GpuResetReason};
use std::any::Any;

/// Native D3D11 resources borrowed before scene drawing begins.
///
/// Use application-owned targets for offscreen drawing. Do not submit or present,
/// retain frame resources, reenter GPUI, or leave an encoder active on return.
pub struct D3D11PrepareContext<'a> {
    /// Geometry and opacity of the surface that will be drawn later.
    pub target: GpuPaintTarget,
    /// Resources must be created on this device.
    pub device: &'a windows::Win32::Graphics::Direct3D11::ID3D11Device,
    /// Commands recorded here execute before ordinary scene drawing.
    pub context: &'a windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    /// Color format of the eventual drawing target.
    pub color_format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
}

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

/// Independent-pass drawing using the window's native D3D11 resources.
pub trait D3D11Painter: Send + 'static {
    /// Immutable application data retained by the scene, including scene replays.
    type Snapshot: Send + Sync + 'static;

    /// Prepares resources before ordinary scene drawing, once per visible surface.
    /// All surfaces prepare before drawing. Keep separate resources or buffer ranges
    /// for different snapshots sharing this painter. Errors skip the remaining stages.
    fn prepare(
        &mut self,
        _context: &mut D3D11PrepareContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        Ok(())
    }

    /// Runs after all surfaces have prepared, before ordinary scene drawing.
    /// Called once per successfully prepared surface; an error skips its drawing.
    fn finish_prepare(
        &mut self,
        _context: &mut D3D11PrepareContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        Ok(())
    }

    /// Draws at this surface's position in the scene.
    /// Preserve existing color, obey the inherited clip and apply target opacity.
    /// Do not submit, present, retain frame resources or reenter GPUI. End any
    /// application encoders before returning, including on error. Encoded commands
    /// are not rolled back by an error; success does not mean GPU completion.
    fn paint(
        &mut self,
        context: &mut D3D11PaintContext<'_>,
        snapshot: &Self::Snapshot,
    ) -> Result<()>;

    /// Releases resources before device replacement or window destruction.
    /// Resize does not reset the painter. Recreate resources on the next callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// Registers independent-pass drawing with typed application snapshots.
pub fn d3d11_painter<P: D3D11Painter>(painter: P) -> GpuPainter<P::Snapshot> {
    GpuPainter::new(D3D11PainterAdapter(Box::new(IndependentPainter(painter))))
}

pub(crate) struct D3D11PainterAdapter(pub(crate) Box<dyn AnyD3D11Painter>);

impl GpuPainterBackend for D3D11PainterAdapter {
    fn reset(&mut self, reason: GpuResetReason) {
        self.0.reset(reason);
    }
}

pub(crate) trait AnyD3D11Painter: Send {
    fn prepare(
        &mut self,
        context: &mut D3D11PrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn finish_prepare(
        &mut self,
        context: &mut D3D11PrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn paint(
        &mut self,
        context: &mut D3D11PaintContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn reset(&mut self, reason: GpuResetReason);
}

struct IndependentPainter<P>(P);

impl<P: D3D11Painter> AnyD3D11Painter for IndependentPainter<P> {
    fn prepare(
        &mut self,
        context: &mut D3D11PrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.prepare(context, snapshot)
    }

    fn finish_prepare(
        &mut self,
        context: &mut D3D11PrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.finish_prepare(context, snapshot)
    }

    fn paint(
        &mut self,
        context: &mut D3D11PaintContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.paint(context, snapshot)
    }

    fn reset(&mut self, reason: GpuResetReason) {
        self.0.reset(reason);
    }
}

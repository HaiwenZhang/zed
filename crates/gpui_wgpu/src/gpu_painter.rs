use anyhow::Result;
use gpui::{GpuDrawMode, GpuPaintTarget, GpuPainter, GpuPainterBackend, GpuResetReason};
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

/// Native Wgpu drawing access within GPUI's current UI pass.
///
/// GPUI sets the viewport and scissor before drawing. Attachments cannot be
/// replaced; use [`wgpu_painter`] for depth, multisampling or other passes.
/// Do not end the pass, retain its resources, or reenter GPUI.
pub struct WgpuDrawContext<'a, 'pass> {
    /// Target geometry and inherited opacity; apply opacity when writing color.
    pub target: GpuPaintTarget,
    /// Resources must be created on this device.
    pub device: &'a wgpu::Device,
    /// Current UI drawing pass, borrowed only for this callback.
    pub pass: &'a mut wgpu::RenderPass<'pass>,
    /// Color interpretation used by the target.
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

/// Independent-pass drawing using the window's native Wgpu resources.
pub trait WgpuPainter: Send + 'static {
    /// Immutable application data retained by the scene, including scene replays.
    type Snapshot: Send + Sync + 'static;

    /// Prepares resources before ordinary scene drawing, once per visible surface.
    /// All surfaces prepare before drawing. Keep separate resources or buffer ranges
    /// for different snapshots sharing this painter. Errors skip the remaining stages.
    fn prepare(
        &mut self,
        _context: &mut WgpuPrepareContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        Ok(())
    }

    /// Runs after all surfaces have prepared, before ordinary scene drawing.
    /// Called once per successfully prepared surface; an error skips its drawing.
    fn finish_prepare(
        &mut self,
        _context: &mut WgpuPrepareContext<'_>,
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
        context: &mut WgpuPaintContext<'_>,
        snapshot: &Self::Snapshot,
    ) -> Result<()>;

    /// Releases resources before device replacement or window destruction.
    /// Resize does not reset the painter. Recreate resources on the next callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// Drawing in GPUI's existing Wgpu UI pass.
/// Attachments cannot be replaced. Leave the pass open; GPUI restores UI state.
pub trait WgpuSharedPainter: Send + 'static {
    /// Immutable application data retained by the scene, including scene replays.
    type Snapshot: Send + Sync + 'static;

    /// Prepares resources before ordinary scene drawing, once per visible surface.
    /// All surfaces prepare before drawing. Keep separate resources or buffer ranges
    /// for different snapshots sharing this painter. Errors skip the remaining stages.
    fn prepare(
        &mut self,
        _context: &mut WgpuPrepareContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        Ok(())
    }

    /// Runs after all surfaces have prepared, before ordinary scene drawing.
    /// Called once per successfully prepared surface; an error skips its drawing.
    fn finish_prepare(
        &mut self,
        _context: &mut WgpuPrepareContext<'_>,
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
        context: &mut WgpuDrawContext<'_, '_>,
        snapshot: &Self::Snapshot,
    ) -> Result<()>;

    /// Releases resources before device replacement or window destruction.
    /// Resize does not reset the painter. Recreate resources on the next callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// Registers independent-pass drawing with typed application snapshots.
pub fn wgpu_painter<P: WgpuPainter>(painter: P) -> GpuPainter<P::Snapshot> {
    GpuPainter::new(WgpuPainterAdapter::independent(painter))
}

/// Registers shared-pass drawing with typed application snapshots.
pub fn wgpu_shared_painter<P: WgpuSharedPainter>(painter: P) -> GpuPainter<P::Snapshot> {
    GpuPainter::new(WgpuPainterAdapter::shared(painter))
}

pub(crate) struct WgpuPainterAdapter(pub(crate) Box<dyn AnyWgpuPainter>, GpuDrawMode);

impl WgpuPainterAdapter {
    pub(crate) fn independent<P: WgpuPainter>(painter: P) -> Self {
        Self(Box::new(IndependentPainter(painter)), GpuDrawMode::OwnPass)
    }

    pub(crate) fn shared<P: WgpuSharedPainter>(painter: P) -> Self {
        Self(Box::new(SharedPainter(painter)), GpuDrawMode::SharedPass)
    }
}

impl GpuPainterBackend for WgpuPainterAdapter {
    fn draw_mode(&self) -> GpuDrawMode {
        self.1
    }

    fn reset(&mut self, reason: GpuResetReason) {
        self.0.reset(reason);
    }
}

pub(crate) trait AnyWgpuPainter: Send {
    fn prepare(
        &mut self,
        context: &mut WgpuPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn finish_prepare(
        &mut self,
        context: &mut WgpuPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn paint(
        &mut self,
        _context: &mut WgpuPaintContext<'_>,
        _snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        anyhow::bail!("Independent-pass access requested for a shared painter")
    }
    fn draw_in_pass(
        &mut self,
        _context: &mut WgpuDrawContext<'_, '_>,
        _snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        anyhow::bail!("Shared-pass access requested for an independent painter")
    }
    fn reset(&mut self, reason: GpuResetReason);
}

struct IndependentPainter<P>(P);

impl<P: WgpuPainter> AnyWgpuPainter for IndependentPainter<P> {
    fn prepare(
        &mut self,
        context: &mut WgpuPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.prepare(context, snapshot)
    }

    fn finish_prepare(
        &mut self,
        context: &mut WgpuPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.finish_prepare(context, snapshot)
    }

    fn paint(
        &mut self,
        context: &mut WgpuPaintContext<'_>,
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

struct SharedPainter<P>(P);

impl<P: WgpuSharedPainter> AnyWgpuPainter for SharedPainter<P> {
    fn prepare(
        &mut self,
        context: &mut WgpuPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.prepare(context, snapshot)
    }

    fn finish_prepare(
        &mut self,
        context: &mut WgpuPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.finish_prepare(context, snapshot)
    }

    fn draw_in_pass(
        &mut self,
        context: &mut WgpuDrawContext<'_, '_>,
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

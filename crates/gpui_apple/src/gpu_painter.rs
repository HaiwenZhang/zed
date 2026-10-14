use anyhow::Result;
use gpui::{GpuDrawMode, GpuPaintTarget, GpuPainter, GpuPainterBackend, GpuResetReason};
use std::any::Any;

/// Native Metal resources borrowed before scene drawing begins.
///
/// Use application-owned targets for offscreen drawing. Do not submit or present,
/// retain frame resources, reenter GPUI, or leave an encoder active on return.
pub struct MetalPrepareContext<'a> {
    /// Geometry and opacity of the surface that will be drawn later.
    pub target: GpuPaintTarget,
    /// Resources must be created on this device.
    pub device: &'a metal::DeviceRef,
    /// Commands recorded here execute before ordinary scene drawing.
    pub command_buffer: &'a metal::CommandBufferRef,
    /// Color format of the eventual drawing target.
    pub color_format: metal::MTLPixelFormat,
}

/// Native Metal drawing access within GPUI's current UI pass.
///
/// GPUI sets the viewport and scissor before drawing. Attachments cannot be
/// replaced; use [`metal_painter`] for depth, multisampling or other passes.
/// Do not end the pass, retain its resources, or reenter GPUI.
pub struct MetalDrawContext<'a> {
    /// Target geometry and inherited opacity; apply opacity when writing color.
    pub target: GpuPaintTarget,
    /// Resources must be created on this device.
    pub device: &'a metal::DeviceRef,
    /// Current UI drawing pass, borrowed only for this callback.
    pub encoder: &'a metal::RenderCommandEncoderRef,
    /// Color interpretation used by the target.
    pub color_format: metal::MTLPixelFormat,
}

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

/// Independent-pass drawing using the window's native Metal resources.
pub trait MetalPainter: Send + 'static {
    /// Immutable application data retained by the scene, including scene replays.
    type Snapshot: Send + Sync + 'static;

    /// Prepares resources before ordinary scene drawing, once per visible surface.
    /// All surfaces prepare before drawing. Keep separate resources or buffer ranges
    /// for different snapshots sharing this painter. Errors skip the remaining stages.
    fn prepare(
        &mut self,
        _context: &mut MetalPrepareContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        Ok(())
    }

    /// Runs after all surfaces have prepared, before ordinary scene drawing.
    /// Called once per successfully prepared surface; an error skips its drawing.
    fn finish_prepare(
        &mut self,
        _context: &mut MetalPrepareContext<'_>,
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
        context: &mut MetalPaintContext<'_>,
        snapshot: &Self::Snapshot,
    ) -> Result<()>;

    /// Releases resources before device replacement or window destruction.
    /// Resize does not reset the painter. Recreate resources on the next callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// Drawing in GPUI's existing Metal UI pass.
/// Attachments cannot be replaced. Leave the pass open; GPUI restores UI state.
pub trait MetalSharedPainter: Send + 'static {
    /// Immutable application data retained by the scene, including scene replays.
    type Snapshot: Send + Sync + 'static;

    /// Prepares resources before ordinary scene drawing, once per visible surface.
    /// All surfaces prepare before drawing. Keep separate resources or buffer ranges
    /// for different snapshots sharing this painter. Errors skip the remaining stages.
    fn prepare(
        &mut self,
        _context: &mut MetalPrepareContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        Ok(())
    }

    /// Runs after all surfaces have prepared, before ordinary scene drawing.
    /// Called once per successfully prepared surface; an error skips its drawing.
    fn finish_prepare(
        &mut self,
        _context: &mut MetalPrepareContext<'_>,
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
        context: &mut MetalDrawContext<'_>,
        snapshot: &Self::Snapshot,
    ) -> Result<()>;

    /// Releases resources before device replacement or window destruction.
    /// Resize does not reset the painter. Recreate resources on the next callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// Registers independent-pass drawing with typed application snapshots.
pub fn metal_painter<P: MetalPainter>(painter: P) -> GpuPainter<P::Snapshot> {
    GpuPainter::new(MetalPainterAdapter::independent(painter))
}

/// Registers shared-pass drawing with typed application snapshots.
pub fn metal_shared_painter<P: MetalSharedPainter>(painter: P) -> GpuPainter<P::Snapshot> {
    GpuPainter::new(MetalPainterAdapter::shared(painter))
}

pub(crate) struct MetalPainterAdapter(pub(crate) Box<dyn AnyMetalPainter>, GpuDrawMode);

impl MetalPainterAdapter {
    pub(crate) fn independent<P: MetalPainter>(painter: P) -> Self {
        Self(Box::new(IndependentPainter(painter)), GpuDrawMode::OwnPass)
    }

    pub(crate) fn shared<P: MetalSharedPainter>(painter: P) -> Self {
        Self(Box::new(SharedPainter(painter)), GpuDrawMode::SharedPass)
    }
}

impl GpuPainterBackend for MetalPainterAdapter {
    fn draw_mode(&self) -> GpuDrawMode {
        self.1
    }

    fn reset(&mut self, reason: GpuResetReason) {
        self.0.reset(reason);
    }
}

pub(crate) trait AnyMetalPainter: Send {
    fn prepare(
        &mut self,
        context: &mut MetalPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn finish_prepare(
        &mut self,
        context: &mut MetalPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn paint(
        &mut self,
        _context: &mut MetalPaintContext<'_>,
        _snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        anyhow::bail!("Independent-pass access requested for a shared painter")
    }
    fn draw_in_pass(
        &mut self,
        _context: &mut MetalDrawContext<'_>,
        _snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        anyhow::bail!("Shared-pass access requested for an independent painter")
    }
    fn reset(&mut self, reason: GpuResetReason);
}

struct IndependentPainter<P>(P);

impl<P: MetalPainter> AnyMetalPainter for IndependentPainter<P> {
    fn prepare(
        &mut self,
        context: &mut MetalPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.prepare(context, snapshot)
    }

    fn finish_prepare(
        &mut self,
        context: &mut MetalPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.finish_prepare(context, snapshot)
    }

    fn paint(
        &mut self,
        context: &mut MetalPaintContext<'_>,
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

impl<P: MetalSharedPainter> AnyMetalPainter for SharedPainter<P> {
    fn prepare(
        &mut self,
        context: &mut MetalPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.prepare(context, snapshot)
    }

    fn finish_prepare(
        &mut self,
        context: &mut MetalPrepareContext<'_>,
        snapshot: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        let snapshot = snapshot
            .downcast_ref::<P::Snapshot>()
            .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot type mismatch"))?;
        self.0.finish_prepare(context, snapshot)
    }

    fn draw_in_pass(
        &mut self,
        context: &mut MetalDrawContext<'_>,
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

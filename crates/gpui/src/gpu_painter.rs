//! Application GPU drawing ordered with ordinary GPUI primitives.

use crate::{
    AnyWindowHandle, Bounds, ContentMask, GpuPaintContext, GpuPaintSurface, GpuPassContext,
    GpuPrepareContext, ScaledPixels,
};
use anyhow::Result;
use gpui_util::ResultExt;
use parking_lot::Mutex;
use std::{
    any::Any,
    fmt,
    sync::{Arc, Weak},
};

/// Application GPU drawing, registered with [`crate::Window::register_gpu_painter`].
///
/// Callbacks run serially and must not reenter GPUI, submit or present the frame,
/// or retain borrowed frame resources. Successful encoding is not GPU completion.
pub trait GpuPainter: Send + 'static {
    /// Immutable application data retained by scenes, including cached scene replays.
    type Snapshot: Send + Sync + 'static;

    /// Whether this painter implements drawing for the window's native backend.
    fn supports_backend(&self, backend: GpuBackend) -> bool;

    /// Prepares one visible surface before ordinary scene drawing.
    ///
    /// Runs again for every encoding attempt, including retries and scene replays.
    /// Surfaces sharing a painter must retain separate resources or buffer ranges
    /// when their snapshots differ. An error skips the surface's remaining stages.
    fn prepare(
        &mut self,
        _context: &mut GpuPrepareContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        Ok(())
    }

    /// Finishes preparation after all visible surfaces have run `prepare`.
    /// Called once per successfully prepared surface, before ordinary scene drawing.
    fn finish_prepare(
        &mut self,
        _context: &mut GpuPrepareContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        Ok(())
    }

    /// Selects access after this surface finishes preparation.
    /// Unsupported modes skip drawing and are reported in the handle's status.
    fn draw_mode(&self, _capabilities: GpuCapabilities, _snapshot: &Self::Snapshot) -> GpuDrawMode {
        GpuDrawMode::OwnPass
    }

    /// Draws in GPUI's existing pass when shared access is selected.
    ///
    /// GPUI sets the viewport and scissor and restores its drawing state afterwards.
    /// Preserve existing color and apply inherited opacity. Do not end the pass,
    /// replace attachments or leave queries active, including on error. Errors do
    /// not undo encoded commands or fall back to independent drawing.
    fn draw_in_pass(
        &mut self,
        _context: &mut GpuPassContext<'_, '_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        anyhow::bail!("Shared-pass drawing is not implemented by this painter")
    }

    /// Encodes independent drawing at this surface's position in the scene.
    ///
    /// Preserve existing color, restrict writes to the target's clip and apply its
    /// opacity. End application encoders before returning, including on error.
    /// Errors do not undo commands already encoded.
    fn paint(
        &mut self,
        _context: &mut GpuPaintContext<'_>,
        _snapshot: &Self::Snapshot,
    ) -> Result<()> {
        anyhow::bail!("Independent drawing is not implemented by this painter")
    }

    /// Releases resources before the device or renderer is retired.
    /// Resize does not reset the painter. After device replacement, recreate
    /// resources on the device supplied by the next callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// The native API used by a window's renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuBackend {
    /// Apple's Metal backend.
    Metal,
    /// The wgpu backend, regardless of its underlying graphics API.
    Wgpu,
    /// Windows' Direct3D 11 backend.
    D3D11,
}

impl GpuBackend {
    /// Drawing access provided by this GPUI backend.
    pub fn capabilities(self) -> GpuCapabilities {
        GpuCapabilities { backend: self }
    }
}

/// Access supported by the current window's GPU painter integration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuCapabilities {
    backend: GpuBackend,
}

impl GpuCapabilities {
    /// The current native backend.
    pub fn backend(self) -> GpuBackend {
        self.backend
    }

    /// Whether this backend provides the requested drawing access.
    pub fn supports(self, mode: GpuDrawMode) -> bool {
        mode == GpuDrawMode::OwnPass || self.backend != GpuBackend::D3D11
    }
}

/// The drawing access requested by an application GPU painter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuDrawMode {
    /// Draw into GPUI's current pass without replacing its attachments.
    SharedPass,
    /// End GPUI's pass and let the painter encode independent passes.
    #[default]
    OwnPass,
}

/// Why a renderer must release its resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuResetReason {
    /// The owning window's device is being replaced.
    DeviceReplaced,
    /// The owning window renderer is being destroyed.
    WindowDestroyed,
}

/// Geometry in physical pixels in the current target's coordinate system.
#[derive(Clone, Copy, Debug)]
pub struct GpuPaintTarget {
    /// Full color target dimensions.
    pub size: [u32; 2],
    /// Complete element bounds, even when partially clipped.
    pub bounds: Bounds<ScaledPixels>,
    /// Intersection of bounds, parent clip and target edges.
    pub clip: Bounds<ScaledPixels>,
    /// Logical to physical pixel scale.
    pub scale_factor: f32,
    /// Samples per color pixel; application attachments must match.
    pub sample_count: u32,
    /// Inherited element opacity; apply when writing color.
    pub opacity: f32,
}

impl GpuPaintTarget {
    /// Returns an integer scissor `[left, top, width, height] inside the clip and target.
    ///
    /// Fractional edges round inward so writes cannot escape the inherited clip.
    /// A subpixel clip may produce a zero width or height.
    pub fn scissor_rect(&self) -> [u32; 4] {
        let left = self.clip.origin.x.0.ceil().clamp(0.0, self.size[0] as f32) as u32;
        let top = self.clip.origin.y.0.ceil().clamp(0.0, self.size[1] as f32) as u32;
        let right = self.clip.right().0.floor().clamp(0.0, self.size[0] as f32) as u32;
        let bottom = self.clip.bottom().0.floor().clamp(0.0, self.size[1] as f32) as u32;
        [
            left,
            top,
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        ]
    }
}

/// Encoding diagnostics; these do not establish GPU execution completion.
#[derive(Clone, Debug, Default)]
pub struct GpuPainterStatus {
    /// Successful drawing callback encodings.
    pub encoded: u64,
    /// Lifecycle notifications.
    pub resets: u64,
    /// Most recent preparation or drawing error, cleared by the next successful draw.
    pub last_error: Option<String>,
}

/// Type-erased callbacks used by platform renderers.
#[doc(hidden)]
pub trait ErasedGpuPainter: Send {
    fn supports_backend_erased(&self, backend: GpuBackend) -> bool;
    fn prepare_erased(
        &mut self,
        context: &mut GpuPrepareContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn finish_prepare_erased(
        &mut self,
        context: &mut GpuPrepareContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn draw_mode_erased(
        &self,
        capabilities: GpuCapabilities,
        data: &(dyn Any + Send + Sync),
    ) -> Result<GpuDrawMode>;
    fn draw_in_pass_erased(
        &mut self,
        context: &mut GpuPassContext<'_, '_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn paint_erased(
        &mut self,
        context: &mut GpuPaintContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()>;
    fn reset_erased(&mut self, reason: GpuResetReason);
}

fn painter_snapshot<P: GpuPainter>(data: &(dyn Any + Send + Sync)) -> Result<&P::Snapshot> {
    data.downcast_ref::<P::Snapshot>()
        .ok_or_else(|| anyhow::anyhow!("GPU painter snapshot has an incompatible type"))
}

impl<P: GpuPainter> ErasedGpuPainter for P {
    fn supports_backend_erased(&self, backend: GpuBackend) -> bool {
        GpuPainter::supports_backend(self, backend)
    }
    fn prepare_erased(
        &mut self,
        context: &mut GpuPrepareContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        GpuPainter::prepare(self, context, painter_snapshot::<P>(data)?)
    }
    fn finish_prepare_erased(
        &mut self,
        context: &mut GpuPrepareContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        GpuPainter::finish_prepare(self, context, painter_snapshot::<P>(data)?)
    }
    fn draw_mode_erased(
        &self,
        capabilities: GpuCapabilities,
        data: &(dyn Any + Send + Sync),
    ) -> Result<GpuDrawMode> {
        Ok(GpuPainter::draw_mode(
            self,
            capabilities,
            painter_snapshot::<P>(data)?,
        ))
    }
    fn draw_in_pass_erased(
        &mut self,
        context: &mut GpuPassContext<'_, '_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        GpuPainter::draw_in_pass(self, context, painter_snapshot::<P>(data)?)
    }
    fn paint_erased(
        &mut self,
        context: &mut GpuPaintContext<'_>,
        data: &(dyn Any + Send + Sync),
    ) -> Result<()> {
        GpuPainter::paint(self, context, painter_snapshot::<P>(data)?)
    }
    fn reset_erased(&mut self, reason: GpuResetReason) {
        GpuPainter::reset(self, reason);
    }
}

struct PainterState {
    painter: Box<dyn ErasedGpuPainter>,
    status: GpuPainterStatus,
}

/// A window-scoped painter accepting only snapshots of type `Snapshot`.
///
/// ```no_run
/// use gpui::{Bounds, GpuPainterHandle, Pixels, Window};
/// use std::sync::Arc;
/// fn paint_snapshot(window: &mut Window, bounds: Bounds<Pixels>, handle: &GpuPainterHandle<u32>) -> anyhow::Result<()> {
///     window.paint_gpu(bounds, handle, Arc::new(42))
/// }
/// ```
///
/// ```compile_fail
/// use gpui::{Bounds, GpuPainterHandle, Pixels, Window};
/// use std::sync::Arc;
/// fn wrong_snapshot(window: &mut Window, bounds: Bounds<Pixels>, handle: &GpuPainterHandle<u32>) -> anyhow::Result<()> {
///     window.paint_gpu(bounds, handle, Arc::new(String::new()))
/// }
/// ```
pub struct GpuPainterHandle<Snapshot: Send + Sync + 'static> {
    inner: AnyGpuPainterHandle,
    // Dispatch downcasts to the exact snapshot type, so this handle must be invariant.
    snapshot: std::marker::PhantomData<fn(Snapshot) -> Snapshot>,
}

impl<Snapshot: Send + Sync + 'static> Clone for GpuPainterHandle<Snapshot> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            snapshot: std::marker::PhantomData,
        }
    }
}

impl<Snapshot: Send + Sync + 'static> fmt::Debug for GpuPainterHandle<Snapshot> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl<Snapshot: Send + Sync + 'static> GpuPainterHandle<Snapshot> {
    pub(crate) fn new(
        owner: AnyWindowHandle,
        painter: impl GpuPainter<Snapshot = Snapshot>,
    ) -> (Self, GpuPainterRegistration) {
        let (inner, registration) = AnyGpuPainterHandle::new(owner, painter);
        (
            Self {
                inner,
                snapshot: std::marker::PhantomData,
            },
            registration,
        )
    }

    pub(crate) fn erase(&self) -> AnyGpuPainterHandle {
        self.inner.clone()
    }

    pub(crate) fn owner(&self) -> AnyWindowHandle {
        self.inner.owner()
    }

    /// Encoding diagnostics; do not call from this painter's callbacks.
    pub fn status(&self) -> GpuPainterStatus {
        self.inner.status()
    }
}

/// Window-scoped registration retained by referencing scenes.
#[doc(hidden)]
#[derive(Clone)]
pub struct AnyGpuPainterHandle {
    owner: AnyWindowHandle,
    state: Arc<Mutex<PainterState>>,
}

impl fmt::Debug for AnyGpuPainterHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnyGpuPainterHandle")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl AnyGpuPainterHandle {
    pub(crate) fn new(
        owner: AnyWindowHandle,
        painter: impl GpuPainter,
    ) -> (Self, GpuPainterRegistration) {
        let state = Arc::new(Mutex::new(PainterState {
            painter: Box::new(painter),
            status: GpuPainterStatus::default(),
        }));
        let registration = GpuPainterRegistration(Arc::downgrade(&state));
        (Self { owner, state }, registration)
    }

    pub(crate) fn owner(&self) -> AnyWindowHandle {
        self.owner
    }

    /// Returns the painter's encoding diagnostics.
    ///
    /// Do not call from this painter's preparation, drawing or `reset` callbacks: they hold the
    /// same lock used to read these diagnostics.
    pub fn status(&self) -> GpuPainterStatus {
        self.state.lock().status.clone()
    }

    #[doc(hidden)]
    pub fn invoke(
        &self,
        paint: impl FnOnce(&mut dyn ErasedGpuPainter) -> Result<()>,
    ) -> Result<()> {
        self.invoke_callback(paint, true)
    }

    fn invoke_callback(
        &self,
        callback: impl FnOnce(&mut dyn ErasedGpuPainter) -> Result<()>,
        records_encoding: bool,
    ) -> Result<()> {
        let mut state = self.state.lock();
        let result = callback(state.painter.as_mut());
        match &result {
            Ok(()) if records_encoding => {
                state.status.encoded = state.status.encoded.saturating_add(1);
                state.status.last_error = None;
            }
            Err(error) => state.status.last_error = Some(format!("{error:#}")),
            Ok(()) => {}
        }
        result
    }
}

/// The two preparation stages run before ordinary scene drawing.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuPreparationPhase {
    /// Prepare a surface's resources.
    Prepare,
    /// Finish a surface's preparation after all surfaces have prepared.
    FinishPrepare,
}

/// Per-surface preparation results for one frame encoding attempt.
#[doc(hidden)]
pub struct GpuPaintPreparation {
    modes: Vec<Option<GpuDrawMode>>,
}

impl GpuPaintPreparation {
    /// Whether this surface is visible and both preparation stages succeeded.
    pub fn is_ready(&self, surface_index: usize) -> bool {
        self.draw_mode(surface_index).is_some()
    }

    /// The selected drawing mode, or `None` if this surface must be skipped.
    pub fn draw_mode(&self, surface_index: usize) -> Option<GpuDrawMode> {
        self.modes.get(surface_index).copied().flatten()
    }
}

/// Prepares visible surfaces, then finishes their preparation in scene order.
///
/// Each stage runs once per surface, including surfaces sharing a painter. Failed
/// surfaces are excluded from subsequent stages. Call again when encoding a replay
/// or retry of the scene; readiness must not be reused across frames.
#[doc(hidden)]
pub fn prepare_gpu_paints(
    surfaces: &[GpuPaintSurface],
    size: [u32; 2],
    sample_count: u32,
    capabilities: GpuCapabilities,
    mut callback: impl FnMut(
        &mut dyn ErasedGpuPainter,
        &GpuPaintPrimitive,
        GpuPaintTarget,
        GpuPreparationPhase,
    ) -> Result<()>,
) -> GpuPaintPreparation {
    let targets: Vec<_> = surfaces
        .iter()
        .map(|surface| {
            surface
                .draw
                .target(size, surface.bounds, surface.content_mask, sample_count)
        })
        .collect();
    let mut ready: Vec<_> = targets
        .iter()
        .map(|target| target.clip.size.width.0 > 0.0 && target.clip.size.height.0 > 0.0)
        .collect();
    let mut modes = vec![None; surfaces.len()];

    for phase in [
        GpuPreparationPhase::Prepare,
        GpuPreparationPhase::FinishPrepare,
    ] {
        for (((surface, target), ready), mode) in surfaces
            .iter()
            .zip(&targets)
            .zip(&mut ready)
            .zip(&mut modes)
        {
            if *ready {
                *ready = surface
                    .draw
                    .handle
                    .invoke_callback(
                        |painter| {
                            anyhow::ensure!(
                                painter.supports_backend_erased(capabilities.backend()),
                                "GPU painter does not support {:?}",
                                capabilities.backend()
                            );
                            callback(painter, &surface.draw, *target, phase)?;
                            if phase == GpuPreparationPhase::FinishPrepare {
                                let selected = painter
                                    .draw_mode_erased(capabilities, surface.draw.data.as_ref())?;
                                anyhow::ensure!(
                                    capabilities.supports(selected),
                                    "GPU backend {:?} does not support {selected:?}",
                                    capabilities.backend()
                                );
                                *mode = Some(selected);
                            }
                            Ok(())
                        },
                        false,
                    )
                    .log_err()
                    .is_some();
            }
        }
    }

    GpuPaintPreparation { modes }
}

/// A weak painter registration used by platform renderers.
#[doc(hidden)]
pub struct GpuPainterRegistration(Weak<Mutex<PainterState>>);

impl GpuPainterRegistration {
    /// Validates that the painter supports the window's native backend.
    pub fn validate(&self, backend: GpuBackend) -> Result<()> {
        let state = self
            .0
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("GPU painter was dropped before registration"))?;
        anyhow::ensure!(
            state.lock().painter.supports_backend_erased(backend),
            "GPU painter does not support {backend:?}"
        );
        Ok(())
    }

    fn reset(&self, reason: GpuResetReason) {
        if let Some(state) = self.0.upgrade() {
            let mut state = state.lock();
            state.painter.reset_erased(reason);
            state.status.resets = state.status.resets.saturating_add(1);
        }
    }
}

/// Notifies registered painters when a renderer retires its device, even before first paint.
#[doc(hidden)]
#[derive(Default)]
pub struct GpuPainterRegistry(Mutex<Vec<GpuPainterRegistration>>);

impl GpuPainterRegistry {
    /// Registers a painter without extending its lifetime.
    pub fn register(&self, registration: GpuPainterRegistration) {
        let mut registrations = self.0.lock();
        registrations.retain(|entry| entry.0.strong_count() > 0);
        registrations.push(registration);
    }

    /// Notify before retiring a device or renderer. Do not reenter this registry.
    pub fn reset(&self, reason: GpuResetReason) {
        let mut registrations = self.0.lock();
        for registration in registrations.iter() {
            registration.reset(reason);
        }
        if reason == GpuResetReason::WindowDestroyed {
            registrations.clear();
        }
    }
}

impl Drop for GpuPainterRegistry {
    fn drop(&mut self) {
        self.reset(GpuResetReason::WindowDestroyed);
    }
}

/// A painter and application snapshot retained by the scene.
#[derive(Clone)]
pub struct GpuPaintPrimitive {
    /// Window-scoped painter.
    pub handle: AnyGpuPainterHandle,
    /// Application snapshot.
    pub data: Arc<dyn Any + Send + Sync>,
    /// Logical to physical scale at scene construction.
    pub scale_factor: f32,
    /// Inherited element opacity.
    pub opacity: f32,
}

impl fmt::Debug for GpuPaintPrimitive {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GpuPaintPrimitive")
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

impl GpuPaintPrimitive {
    /// Build the effective physical clip for the actual rendering target.
    #[doc(hidden)]
    pub fn target(
        &self,
        size: [u32; 2],
        bounds: Bounds<ScaledPixels>,
        mask: ContentMask<ScaledPixels>,
        sample_count: u32,
    ) -> GpuPaintTarget {
        let target_bounds = Bounds {
            origin: crate::point(ScaledPixels(0.0), ScaledPixels(0.0)),
            size: crate::size(ScaledPixels(size[0] as f32), ScaledPixels(size[1] as f32)),
        };
        GpuPaintTarget {
            size,
            bounds,
            clip: bounds.intersect(&mask.bounds).intersect(&target_bounds),
            scale_factor: self.scale_factor,
            sample_count,
            opacity: self.opacity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AnyGpuPainterHandle, GpuBackend, GpuCapabilities, GpuDrawMode, GpuPaintPrimitive,
        GpuPaintTarget, GpuPainter, GpuPainterRegistration, GpuPainterRegistry, GpuPainterStatus,
        GpuPreparationPhase, GpuResetReason, PainterState, prepare_gpu_paints,
    };
    use crate::{
        Bounds, ContentMask, Context, GpuPaintSurface, IntoElement, PrimitiveBatch, Quad, Render,
        ScaledPixels, Scene, SceneBatch, Window, WindowHandle, WindowId, div, point, size,
    };
    use parking_lot::Mutex;
    use std::sync::Arc;

    struct TestView;

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct RecordingPainter(Arc<Mutex<Vec<GpuResetReason>>>);

    impl GpuPainter for RecordingPainter {
        type Snapshot = u32;
        fn supports_backend(&self, _: GpuBackend) -> bool {
            true
        }
        fn reset(&mut self, reason: GpuResetReason) {
            self.0.lock().push(reason);
        }
    }

    fn recording_surface(snapshot: u32) -> GpuPaintSurface {
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, _) =
            AnyGpuPainterHandle::new(owner, RecordingPainter(Arc::new(Mutex::new(Vec::new()))));
        let bounds = Bounds::new(
            point(ScaledPixels(0.0), ScaledPixels(0.0)),
            size(ScaledPixels(100.0), ScaledPixels(100.0)),
        );
        GpuPaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            draw: GpuPaintPrimitive {
                handle,
                data: Arc::new(snapshot),
                scale_factor: 2.0,
                opacity: 0.5,
            },
        }
    }

    #[test]
    fn draw_modes_are_selected_after_preparation_for_each_encoding_attempt() {
        struct SelectingPainter {
            mode: Arc<Mutex<GpuDrawMode>>,
            selections: Arc<Mutex<Vec<GpuDrawMode>>>,
        }
        impl GpuPainter for SelectingPainter {
            type Snapshot = u32;
            fn supports_backend(&self, _: GpuBackend) -> bool {
                true
            }
            fn draw_mode(&self, _: GpuCapabilities, _: &u32) -> GpuDrawMode {
                let mode = *self.mode.lock();
                self.selections.lock().push(mode);
                mode
            }
            fn reset(&mut self, _: GpuResetReason) {}
        }

        let selections = Arc::new(Mutex::new(Vec::new()));
        let mode = Arc::new(Mutex::new(GpuDrawMode::OwnPass));
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, _) = AnyGpuPainterHandle::new(
            owner,
            SelectingPainter {
                mode: mode.clone(),
                selections: selections.clone(),
            },
        );
        let mut surfaces = [
            recording_surface(1),
            recording_surface(2),
            recording_surface(3),
        ];
        for surface in &mut surfaces {
            surface.draw.handle = handle.clone();
        }

        for reverse in [false, true] {
            let preparation = prepare_gpu_paints(
                &surfaces,
                [100, 100],
                1,
                GpuBackend::Metal.capabilities(),
                |_, draw, _, phase| {
                    let snapshot = draw.data.downcast_ref::<u32>().copied().unwrap_or_default();
                    if snapshot == 3 {
                        anyhow::bail!("surface failed before mode selection");
                    }
                    if phase == GpuPreparationPhase::FinishPrepare {
                        *mode.lock() = if (snapshot == 1) != reverse {
                            GpuDrawMode::SharedPass
                        } else {
                            GpuDrawMode::OwnPass
                        };
                    }
                    Ok(())
                },
            );
            let expected = if reverse {
                [GpuDrawMode::OwnPass, GpuDrawMode::SharedPass]
            } else {
                [GpuDrawMode::SharedPass, GpuDrawMode::OwnPass]
            };
            assert_eq!(preparation.draw_mode(0), Some(expected[0]));
            assert_eq!(preparation.draw_mode(1), Some(expected[1]));
            assert_eq!(preparation.draw_mode(2), None);
        }
        assert_eq!(
            *selections.lock(),
            [
                GpuDrawMode::SharedPass,
                GpuDrawMode::OwnPass,
                GpuDrawMode::OwnPass,
                GpuDrawMode::SharedPass
            ]
        );
        assert_eq!(handle.status().encoded, 0);
    }

    #[test]
    fn scissor_rounds_inward_and_clamps_to_the_target() {
        let mut target = GpuPaintTarget {
            size: [100, 80],
            bounds: Bounds::default(),
            clip: Bounds::new(
                point(ScaledPixels(-5.5), ScaledPixels(4.5)),
                size(ScaledPixels(110.0), ScaledPixels(100.0)),
            ),
            scale_factor: 2.0,
            sample_count: 1,
            opacity: 1.0,
        };
        assert_eq!(target.scissor_rect(), [0, 5, 100, 75]);
        target.clip = Bounds::new(
            point(ScaledPixels(10.2), ScaledPixels(12.2)),
            size(ScaledPixels(0.5), ScaledPixels(0.5)),
        );
        assert_eq!(target.scissor_rect(), [11, 13, 0, 0]);
    }

    #[test]
    fn preparation_stages_run_in_order_for_shared_painters_and_scene_replays() {
        let first = recording_surface(1);
        let mut second = recording_surface(2);
        second.draw.handle = first.draw.handle.clone();
        let handle = first.draw.handle.clone();
        let mut scene = Scene::default();
        scene.insert_gpu_paint(first);
        scene.insert_gpu_paint(second);
        scene.finish();
        let mut replay = Scene::default();
        replay.replay(0..scene.len(), &scene);
        replay.finish();

        for scene in [&scene, &replay] {
            let mut calls = Vec::new();
            let preparation = prepare_gpu_paints(
                &scene.gpu_paints,
                [80, 60],
                1,
                GpuBackend::Metal.capabilities(),
                |_, draw, target, phase| {
                    assert_eq!(target.size, [80, 60]);
                    assert_eq!(
                        target.clip.size,
                        size(ScaledPixels(80.0), ScaledPixels(60.0))
                    );
                    assert_eq!(target.scale_factor, 2.0);
                    assert_eq!(target.opacity, 0.5);
                    calls.push((phase, *draw.data.downcast_ref::<u32>().expect("snapshot")));
                    Ok(())
                },
            );
            assert_eq!(
                calls,
                [
                    (GpuPreparationPhase::Prepare, 1),
                    (GpuPreparationPhase::Prepare, 2),
                    (GpuPreparationPhase::FinishPrepare, 1),
                    (GpuPreparationPhase::FinishPrepare, 2),
                ]
            );
            assert!(preparation.is_ready(0));
            assert!(preparation.is_ready(1));
            assert!(!preparation.is_ready(2));
        }
        assert_eq!(handle.status().encoded, 0);
    }

    #[test]
    fn preparation_errors_skip_later_stages_and_clipped_surfaces_do_not_prepare() {
        let mut clipped = recording_surface(4);
        clipped.bounds.origin = point(ScaledPixels(200.0), ScaledPixels(200.0));
        let surfaces = [
            recording_surface(1),
            recording_surface(2),
            recording_surface(3),
            clipped,
        ];
        let mut calls = Vec::new();
        let preparation = prepare_gpu_paints(
            &surfaces,
            [100, 100],
            1,
            GpuBackend::Metal.capabilities(),
            |_, draw, _, phase| {
                let snapshot = *draw.data.downcast_ref::<u32>().expect("snapshot");
                calls.push((phase, snapshot));
                if (phase, snapshot) == (GpuPreparationPhase::Prepare, 1)
                    || (phase, snapshot) == (GpuPreparationPhase::FinishPrepare, 2)
                {
                    anyhow::bail!("preparation failed for {snapshot}");
                }
                Ok(())
            },
        );
        assert_eq!(
            calls,
            [
                (GpuPreparationPhase::Prepare, 1),
                (GpuPreparationPhase::Prepare, 2),
                (GpuPreparationPhase::Prepare, 3),
                (GpuPreparationPhase::FinishPrepare, 2),
                (GpuPreparationPhase::FinishPrepare, 3),
            ]
        );
        assert_eq!(
            (0..surfaces.len())
                .map(|index| preparation.is_ready(index))
                .collect::<Vec<_>>(),
            [false, false, true, false]
        );
        for (surface, expected_error) in surfaces.iter().zip([
            Some("preparation failed for 1"),
            Some("preparation failed for 2"),
            None,
            None,
        ]) {
            assert_eq!(
                surface.draw.handle.status().last_error.as_deref(),
                expected_error
            );
            assert_eq!(surface.draw.handle.status().encoded, 0);
        }

        let recovery = prepare_gpu_paints(
            &surfaces,
            [100, 100],
            1,
            GpuBackend::Metal.capabilities(),
            |_, _, _, _| Ok(()),
        );
        for (index, surface) in surfaces.iter().enumerate() {
            if recovery.is_ready(index) {
                surface.draw.handle.invoke(|_| Ok(())).expect("draw");
                assert!(surface.draw.handle.status().last_error.is_none());
                assert_eq!(surface.draw.handle.status().encoded, 1);
            }
        }
    }

    #[test]
    fn dispatch_validates_backend_and_snapshot_and_records_results() {
        struct MetalOnly;
        impl GpuPainter for MetalOnly {
            type Snapshot = u32;
            fn supports_backend(&self, backend: GpuBackend) -> bool {
                backend == GpuBackend::Metal
            }
            fn reset(&mut self, _: GpuResetReason) {}
        }
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, registration) = AnyGpuPainterHandle::new(owner, MetalOnly);
        registration
            .validate(GpuBackend::Metal)
            .expect("supported backend");
        assert!(registration.validate(GpuBackend::D3D11).is_err());
        assert!(registration.validate(GpuBackend::Wgpu).is_err());

        let result = handle.invoke(|painter| {
            painter.draw_mode_erased(GpuBackend::Metal.capabilities(), &"wrong snapshot")?;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(handle.status().encoded, 0);
        assert!(handle.status().last_error.is_some());

        let result = handle.invoke(|_| anyhow::bail!("encoding failed"));
        assert!(result.is_err());
        assert_eq!(
            handle.status().last_error.as_deref(),
            Some("encoding failed")
        );

        handle
            .invoke(|painter| {
                assert_eq!(
                    painter.draw_mode_erased(GpuBackend::Metal.capabilities(), &42_u32)?,
                    GpuDrawMode::OwnPass
                );
                Ok(())
            })
            .expect("draw");
        assert_eq!(handle.status().encoded, 1);
        assert!(handle.status().last_error.is_none());
    }

    #[test]
    fn typed_painter_selects_supported_access_for_each_backend() {
        struct Snapshot;
        struct AdaptivePainter;
        impl GpuPainter for AdaptivePainter {
            type Snapshot = Snapshot;
            fn supports_backend(&self, _: GpuBackend) -> bool {
                true
            }
            fn draw_mode(&self, capabilities: GpuCapabilities, _: &Snapshot) -> GpuDrawMode {
                if capabilities.supports(GpuDrawMode::SharedPass) {
                    GpuDrawMode::SharedPass
                } else {
                    GpuDrawMode::OwnPass
                }
            }
            fn reset(&mut self, _: GpuResetReason) {}
        }
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, registration) = super::GpuPainterHandle::new(owner, AdaptivePainter);
        let cloned_handle = handle.clone();
        let mut surface = recording_surface(1);
        surface.draw.handle = cloned_handle.erase();
        surface.draw.data = Arc::new(Snapshot);
        for (backend, expected) in [
            (GpuBackend::Metal, GpuDrawMode::SharedPass),
            (GpuBackend::Wgpu, GpuDrawMode::SharedPass),
            (GpuBackend::D3D11, GpuDrawMode::OwnPass),
        ] {
            registration.validate(backend).expect("backend supported");
            let preparation = prepare_gpu_paints(
                std::slice::from_ref(&surface),
                [100, 100],
                1,
                backend.capabilities(),
                |_, _, _, _| Ok(()),
            );
            assert_eq!(preparation.draw_mode(0), Some(expected));
            assert!(handle.status().last_error.is_none());
        }
    }

    #[test]
    fn unsupported_draw_mode_skips_surface_and_records_error() {
        struct SharedPainter;
        impl GpuPainter for SharedPainter {
            type Snapshot = u32;
            fn supports_backend(&self, _: GpuBackend) -> bool {
                true
            }
            fn draw_mode(&self, _: GpuCapabilities, _: &u32) -> GpuDrawMode {
                GpuDrawMode::SharedPass
            }
            fn reset(&mut self, _: GpuResetReason) {}
        }
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, registration) = AnyGpuPainterHandle::new(owner, SharedPainter);
        registration
            .validate(GpuBackend::D3D11)
            .expect("backend supported");
        let mut surface = recording_surface(1);
        surface.draw.handle = handle.clone();
        let preparation = prepare_gpu_paints(
            &[surface],
            [100, 100],
            1,
            GpuBackend::D3D11.capabilities(),
            |_, _, _, _| Ok(()),
        );
        assert!(!preparation.is_ready(0));
        assert_eq!(handle.status().encoded, 0);
        assert_eq!(
            handle.status().last_error.as_deref(),
            Some("GPU backend D3D11 does not support SharedPass")
        );
    }

    #[test]
    fn registry_survives_device_reset_and_destroys_once() {
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Mutex::new(PainterState {
            painter: Box::new(RecordingPainter(reasons.clone())),
            status: GpuPainterStatus::default(),
        }));
        let registry = GpuPainterRegistry::default();
        registry.register(GpuPainterRegistration(Arc::downgrade(&state)));
        registry.reset(GpuResetReason::DeviceReplaced);
        registry.reset(GpuResetReason::WindowDestroyed);
        drop(registry);
        assert_eq!(
            *reasons.lock(),
            [
                GpuResetReason::DeviceReplaced,
                GpuResetReason::WindowDestroyed
            ]
        );
        assert_eq!(state.lock().status.resets, 2);
    }

    #[test]
    fn registry_does_not_keep_dropped_painter_alive() {
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Mutex::new(PainterState {
            painter: Box::new(RecordingPainter(reasons.clone())),
            status: GpuPainterStatus::default(),
        }));
        let registry = GpuPainterRegistry::default();
        registry.register(GpuPainterRegistration(Arc::downgrade(&state)));
        drop(state);
        registry.reset(GpuResetReason::DeviceReplaced);
        assert!(reasons.lock().is_empty());
    }

    #[test]
    fn registry_drop_notifies_painter_that_was_never_drawn() {
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, registration) =
            AnyGpuPainterHandle::new(owner, RecordingPainter(reasons.clone()));
        let registry = GpuPainterRegistry::default();
        registry.register(registration);
        drop(registry);

        assert_eq!(*reasons.lock(), [GpuResetReason::WindowDestroyed]);
        assert_eq!(handle.status().resets, 1);
        assert_eq!(handle.status().encoded, 0);
    }

    #[test]
    fn replayed_scene_keeps_painter_and_snapshot_alive() {
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, registration) =
            AnyGpuPainterHandle::new(owner, RecordingPainter(reasons.clone()));
        let registry = GpuPainterRegistry::default();
        registry.register(registration);
        let snapshot = Arc::new(42_u32);
        let weak_snapshot = Arc::downgrade(&snapshot);
        let bounds = Bounds::new(
            point(ScaledPixels(0.0), ScaledPixels(0.0)),
            size(ScaledPixels(100.0), ScaledPixels(100.0)),
        );
        let mut scene = Scene::default();
        let quad = Quad {
            bounds,
            content_mask: ContentMask { bounds },
            ..Default::default()
        };
        scene.insert_primitive(quad);
        scene.insert_gpu_paint(GpuPaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            draw: GpuPaintPrimitive {
                handle,
                data: snapshot,
                scale_factor: 1.0,
                opacity: 1.0,
            },
        });
        scene.insert_primitive(quad);
        scene.finish();
        let mut replay = Scene::default();
        replay.replay(0..scene.len(), &scene);
        replay.finish();
        drop(scene);

        let batches: Vec<_> = replay.render_batches().collect();
        assert!(matches!(
            batches.as_slice(),
            [
                SceneBatch::Primitive(PrimitiveBatch::Quads(_)),
                SceneBatch::GpuPaints(_),
                SceneBatch::Primitive(PrimitiveBatch::Quads(_)),
            ]
        ));
        assert!(
            replay
                .batches()
                .all(|batch| matches!(batch, PrimitiveBatch::Quads(_)))
        );
        assert!(weak_snapshot.upgrade().is_some());
        registry.reset(GpuResetReason::DeviceReplaced);
        assert_eq!(*reasons.lock(), [GpuResetReason::DeviceReplaced]);
        replay.clear();
        assert_eq!(replay.render_batches().count(), 0);
        assert!(weak_snapshot.upgrade().is_none());
        drop(registry);
        assert_eq!(*reasons.lock(), [GpuResetReason::DeviceReplaced]);
    }

    #[test]
    fn target_clip_uses_element_parent_and_actual_target_bounds() {
        let state = Arc::new(Mutex::new(PainterState {
            painter: Box::new(RecordingPainter(Arc::new(Mutex::new(Vec::new())))),
            status: GpuPainterStatus::default(),
        }));
        let draw = GpuPaintPrimitive {
            handle: AnyGpuPainterHandle {
                owner: WindowHandle::<TestView>::new(WindowId::default()).into(),
                state,
            },
            data: Arc::new(()),
            scale_factor: 2.0,
            opacity: 0.5,
        };
        let bounds = Bounds {
            origin: point(ScaledPixels(-10.0), ScaledPixels(20.0)),
            size: size(ScaledPixels(80.0), ScaledPixels(80.0)),
        };
        let mask = ContentMask {
            bounds: Bounds {
                origin: point(ScaledPixels(5.0), ScaledPixels(10.0)),
                size: size(ScaledPixels(100.0), ScaledPixels(60.0)),
            },
        };
        let target = draw.target([50, 60], bounds, mask, 1);
        assert_eq!(target.bounds, bounds);
        assert_eq!(
            target.clip.origin,
            point(ScaledPixels(5.0), ScaledPixels(20.0))
        );
        assert_eq!(
            target.clip.size,
            size(ScaledPixels(45.0), ScaledPixels(40.0))
        );
        assert_eq!(target.scale_factor, 2.0);
        assert_eq!(target.opacity, 0.5);
    }
}

//! Application GPU drawing ordered with ordinary GPUI primitives.

use crate::{AnyWindowHandle, Bounds, ContentMask, GpuPaintSurface, ScaledPixels};
use anyhow::Result;
use gpui_util::ResultExt;
use parking_lot::Mutex;
use std::{
    any::Any,
    fmt,
    marker::PhantomData,
    sync::{Arc, Weak},
};

/// Backend adapter used to register native drawing callbacks.
#[doc(hidden)]
pub trait GpuPainterBackend: Any + Send {
    /// Drawing access fixed by the native registration adapter.
    fn draw_mode(&self) -> GpuDrawMode {
        GpuDrawMode::OwnPass
    }

    /// Releases resources before the window's device or renderer is retired.
    ///
    /// Resize alone does not call this method. After [`GpuResetReason::DeviceReplaced`],
    /// recreate resources on the device supplied to the next preparation or drawing callback.
    fn reset(&mut self, reason: GpuResetReason);
}

/// A native painter awaiting registration, accepting snapshots of type `Snapshot`.
///
/// Construct with the painter adapters exported by the native backend crates.
pub struct GpuPainter<Snapshot> {
    painter: Box<dyn GpuPainterBackend>,
    snapshot: PhantomData<fn(Snapshot) -> Snapshot>,
}

impl<Snapshot: Send + Sync + 'static> GpuPainter<Snapshot> {
    #[doc(hidden)]
    pub fn new(painter: impl GpuPainterBackend) -> Self {
        Self {
            painter: Box::new(painter),
            snapshot: PhantomData,
        }
    }

    pub(crate) fn register(
        self,
        owner: AnyWindowHandle,
    ) -> (GpuPainterHandle<Snapshot>, GpuPainterRegistration) {
        let (handle, registration) = AnyGpuPainterHandle::from_box(owner, self.painter);
        (
            GpuPainterHandle {
                handle,
                snapshot: PhantomData,
            },
            registration,
        )
    }
}

/// A window-scoped native painter accepting snapshots of type `Snapshot`.
pub struct GpuPainterHandle<Snapshot> {
    pub(crate) handle: AnyGpuPainterHandle,
    snapshot: PhantomData<fn(Snapshot) -> Snapshot>,
}

impl<Snapshot> Clone for GpuPainterHandle<Snapshot> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            snapshot: PhantomData,
        }
    }
}

impl<Snapshot> fmt::Debug for GpuPainterHandle<Snapshot> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.handle.fmt(formatter)
    }
}

impl<Snapshot> GpuPainterHandle<Snapshot> {
    /// Returns encoding diagnostics. Do not call from this painter's callbacks.
    pub fn status(&self) -> GpuPainterStatus {
        self.handle.status()
    }

    /// Takes the latest callback error. Successful draws do not clear errors.
    /// Do not call from this painter's callbacks.
    pub fn take_error(&self) -> Option<String> {
        self.handle.take_error()
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
    /// Most recent preparation or drawing error, retained until explicitly taken.
    pub last_error: Option<String>,
}

struct PainterState {
    painter: Box<dyn GpuPainterBackend>,
    status: GpuPainterStatus,
}

/// Type-erased registration retained by referencing scenes.
#[doc(hidden)]
#[derive(Clone)]
pub struct AnyGpuPainterHandle {
    owner: AnyWindowHandle,
    state: Arc<Mutex<PainterState>>,
}

impl fmt::Debug for AnyGpuPainterHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GpuPainterHandle")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl AnyGpuPainterHandle {
    #[cfg(test)]
    pub(crate) fn new(
        owner: AnyWindowHandle,
        painter: impl GpuPainterBackend,
    ) -> (Self, GpuPainterRegistration) {
        Self::from_box(owner, Box::new(painter))
    }

    fn from_box(
        owner: AnyWindowHandle,
        painter: Box<dyn GpuPainterBackend>,
    ) -> (Self, GpuPainterRegistration) {
        let state = Arc::new(Mutex::new(PainterState {
            painter,
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

    pub fn take_error(&self) -> Option<String> {
        self.state.lock().status.last_error.take()
    }

    #[doc(hidden)]
    pub fn invoke<T: GpuPainterBackend>(
        &self,
        paint: impl FnOnce(&mut T) -> Result<()>,
    ) -> Result<()> {
        self.invoke_callback(paint, true)
    }

    fn invoke_callback<T: GpuPainterBackend>(
        &self,
        callback: impl FnOnce(&mut T) -> Result<()>,
        records_encoding: bool,
    ) -> Result<()> {
        let mut state = self.state.lock();
        let painter = (state.painter.as_mut() as &mut dyn Any).downcast_mut::<T>();
        let result = match painter {
            Some(painter) => callback(painter),
            None => Err(anyhow::anyhow!(
                "GPU painter is incompatible with this backend"
            )),
        };
        match &result {
            Ok(()) if records_encoding => {
                state.status.encoded = state.status.encoded.saturating_add(1);
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
pub fn prepare_gpu_paints<T: GpuPainterBackend>(
    surfaces: &[GpuPaintSurface],
    size: [u32; 2],
    sample_count: u32,
    mut callback: impl FnMut(
        &mut T,
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
                    .invoke_callback::<T>(
                        |painter| {
                            callback(painter, &surface.draw, *target, phase)?;
                            if phase == GpuPreparationPhase::FinishPrepare {
                                *mode = Some(painter.draw_mode());
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
    /// Checks that this registration uses the renderer's native adapter.
    pub fn is<T: GpuPainterBackend>(&self) -> bool {
        self.0.upgrade().is_some_and(|state| {
            let state = state.lock();
            (state.painter.as_ref() as &dyn Any).is::<T>()
        })
    }

    fn reset(&self, reason: GpuResetReason) {
        if let Some(state) = self.0.upgrade() {
            let mut state = state.lock();
            state.painter.reset(reason);
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
        AnyGpuPainterHandle as GpuPainterHandle, GpuDrawMode, GpuPaintPrimitive, GpuPaintTarget,
        GpuPainterBackend as GpuPainter, GpuPainterRegistration, GpuPainterRegistry,
        GpuPainterStatus, GpuPreparationPhase, GpuResetReason, PainterState, prepare_gpu_paints,
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
        fn reset(&mut self, reason: GpuResetReason) {
            self.0.lock().push(reason);
        }
    }

    #[test]
    fn typed_handles_clone_without_cloning_snapshots() {
        struct Snapshot;
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let painter = super::GpuPainter::<Snapshot>::new(RecordingPainter(reasons.clone()));
        let (handle, registration) = painter.register(owner);
        let clone: super::GpuPainterHandle<Snapshot> = handle.clone();
        let registry = GpuPainterRegistry::default();
        registry.register(registration);
        drop(handle);
        registry.reset(GpuResetReason::DeviceReplaced);
        assert_eq!(clone.status().resets, 1);
        assert_eq!(*reasons.lock(), [GpuResetReason::DeviceReplaced]);
    }

    #[test]
    fn successful_surface_does_not_clear_another_surfaces_preparation_error() {
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, _) =
            GpuPainterHandle::new(owner, RecordingPainter(Arc::new(Mutex::new(Vec::new()))));
        let mut surfaces = [recording_surface(1), recording_surface(2)];
        for surface in &mut surfaces {
            surface.draw.handle = handle.clone();
        }
        let preparation =
            prepare_gpu_paints::<RecordingPainter>(&surfaces, [100, 100], 1, |_, draw, _, _| {
                if draw.data.downcast_ref::<u32>() == Some(&1) {
                    anyhow::bail!("first surface failed");
                }
                Ok(())
            });
        assert!(!preparation.is_ready(0));
        assert!(preparation.is_ready(1));
        handle
            .invoke::<RecordingPainter>(|_| Ok(()))
            .expect("second surface draws");
        assert_eq!(handle.status().encoded, 1);
        assert_eq!(
            handle.status().last_error.as_deref(),
            Some("first surface failed")
        );
        assert_eq!(handle.take_error().as_deref(), Some("first surface failed"));
        assert!(handle.take_error().is_none());
    }

    fn recording_surface(snapshot: u32) -> GpuPaintSurface {
        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, _) =
            GpuPainterHandle::new(owner, RecordingPainter(Arc::new(Mutex::new(Vec::new()))));
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
    fn registered_modes_are_preserved_across_preparation_and_scene_replays() {
        struct FixedPainter(GpuDrawMode);
        impl GpuPainter for FixedPainter {
            fn draw_mode(&self) -> GpuDrawMode {
                self.0
            }
            fn reset(&mut self, _: GpuResetReason) {}
        }

        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let mut surfaces = [
            recording_surface(1),
            recording_surface(2),
            recording_surface(3),
        ];
        for (surface, mode) in surfaces.iter_mut().zip([
            GpuDrawMode::SharedPass,
            GpuDrawMode::OwnPass,
            GpuDrawMode::SharedPass,
        ]) {
            surface.draw.handle = GpuPainterHandle::new(owner, FixedPainter(mode)).0;
        }
        for _ in 0..2 {
            let preparation =
                prepare_gpu_paints::<FixedPainter>(&surfaces, [100, 100], 1, |_, draw, _, _| {
                    if draw.data.downcast_ref::<u32>() == Some(&3) {
                        anyhow::bail!("preparation failed");
                    }
                    Ok(())
                });
            assert_eq!(preparation.draw_mode(0), Some(GpuDrawMode::SharedPass));
            assert_eq!(preparation.draw_mode(1), Some(GpuDrawMode::OwnPass));
            assert_eq!(preparation.draw_mode(2), None);
        }
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
            let preparation = prepare_gpu_paints::<RecordingPainter>(
                &scene.gpu_paints,
                [80, 60],
                1,
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
        let preparation = prepare_gpu_paints::<RecordingPainter>(
            &surfaces,
            [100, 100],
            1,
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

        let recovery =
            prepare_gpu_paints::<RecordingPainter>(&surfaces, [100, 100], 1, |_, _, _, _| Ok(()));
        for (index, (surface, expected_error)) in surfaces
            .iter()
            .zip([
                Some("preparation failed for 1"),
                Some("preparation failed for 2"),
                None,
                None,
            ])
            .enumerate()
        {
            if recovery.is_ready(index) {
                surface
                    .draw
                    .handle
                    .invoke::<RecordingPainter>(|_| Ok(()))
                    .expect("draw");
                assert_eq!(surface.draw.handle.take_error().as_deref(), expected_error);
                assert!(surface.draw.handle.status().last_error.is_none());
                assert_eq!(surface.draw.handle.status().encoded, 1);
            }
        }
    }

    #[test]
    fn dispatch_checks_adapter_type_and_records_results() {
        struct OtherPainter;
        impl GpuPainter for OtherPainter {
            fn reset(&mut self, _: GpuResetReason) {}
        }

        let owner = WindowHandle::<TestView>::new(WindowId::default()).into();
        let (handle, registration) =
            GpuPainterHandle::new(owner, RecordingPainter(Arc::new(Mutex::new(Vec::new()))));
        assert!(registration.is::<RecordingPainter>());
        assert!(!registration.is::<OtherPainter>());

        let mut called = false;
        let result = handle.invoke::<OtherPainter>(|_| {
            called = true;
            Ok(())
        });
        assert!(result.is_err());
        assert!(!called);
        assert_eq!(handle.status().encoded, 0);
        assert!(handle.status().last_error.is_some());

        let result = handle.invoke::<RecordingPainter>(|_| anyhow::bail!("encoding failed"));
        assert!(result.is_err());
        assert_eq!(
            handle.status().last_error.as_deref(),
            Some("encoding failed")
        );

        let result = handle.invoke::<RecordingPainter>(|_| {
            called = true;
            Ok(())
        });
        assert!(result.is_ok());
        assert!(called);
        assert_eq!(handle.status().encoded, 1);
        assert_eq!(handle.take_error().as_deref(), Some("encoding failed"));
        assert!(handle.status().last_error.is_none());
        assert!(handle.take_error().is_none());
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
            GpuPainterHandle::new(owner, RecordingPainter(reasons.clone()));
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
            GpuPainterHandle::new(owner, RecordingPainter(reasons.clone()));
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
            handle: GpuPainterHandle {
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

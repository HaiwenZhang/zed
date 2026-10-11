//! Application GPU drawing ordered with ordinary GPUI primitives.

use crate::{AnyWindowHandle, Bounds, ContentMask, GpuPaintSurface, ScaledPixels};
use anyhow::Result;
use gpui_util::ResultExt;
use parking_lot::Mutex;
use std::{
    any::Any,
    fmt,
    sync::{Arc, Weak},
};

/// The lifecycle of a painter registered with a window's GPU backend.
///
/// Backend crates provide adapters that implement this trait and expose their native
/// drawing callbacks. Pass an adapter to [`crate::Window::register_gpu_painter`] and
/// queue drawing with [`crate::Window::paint_gpu`].
pub trait GpuPainter: Any + Send {
    /// Releases resources before the window's device or renderer is retired.
    ///
    /// Resize alone does not call this method. After [`GpuResetReason::DeviceReplaced`],
    /// recreate resources on the device supplied to the next preparation or drawing callback.
    fn reset(&mut self, reason: GpuResetReason);
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

/// Encoding diagnostics; these do not establish GPU execution completion.
#[derive(Clone, Debug, Default)]
pub struct GpuPainterStatus {
    /// Successful callback encodings.
    pub encoded: u64,
    /// Lifecycle notifications.
    pub resets: u64,
    /// Most recent preparation or drawing error, cleared by the next successful draw.
    pub last_error: Option<String>,
}

struct PainterState {
    painter: Box<dyn GpuPainter>,
    status: GpuPainterStatus,
}

/// Window-scoped registration retained by referencing scenes.
#[derive(Clone)]
pub struct GpuPainterHandle {
    owner: AnyWindowHandle,
    state: Arc<Mutex<PainterState>>,
}

impl fmt::Debug for GpuPainterHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GpuPainterHandle")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl GpuPainterHandle {
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
    pub fn invoke<T: GpuPainter>(&self, paint: impl FnOnce(&mut T) -> Result<()>) -> Result<()> {
        self.invoke_callback(paint, true)
    }

    fn invoke_callback<T: GpuPainter>(
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
    ready: Vec<bool>,
}

impl GpuPaintPreparation {
    /// Whether this surface is visible and both preparation stages succeeded.
    pub fn is_ready(&self, surface_index: usize) -> bool {
        self.ready.get(surface_index).copied().unwrap_or(false)
    }
}

/// Prepares visible surfaces, then finishes their preparation in scene order.
///
/// Each stage runs once per surface, including surfaces sharing a painter. Failed
/// surfaces are excluded from subsequent stages. Call again when encoding a replay
/// or retry of the scene; readiness must not be reused across frames.
#[doc(hidden)]
pub fn prepare_gpu_paints<T: GpuPainter>(
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

    for phase in [
        GpuPreparationPhase::Prepare,
        GpuPreparationPhase::FinishPrepare,
    ] {
        for ((surface, target), ready) in surfaces.iter().zip(&targets).zip(&mut ready) {
            if *ready {
                *ready = surface
                    .draw
                    .handle
                    .invoke_callback::<T>(
                        |painter| callback(painter, &surface.draw, *target, phase),
                        false,
                    )
                    .log_err()
                    .is_some();
            }
        }
    }

    GpuPaintPreparation { ready }
}

/// A weak painter registration used by platform renderers.
#[doc(hidden)]
pub struct GpuPainterRegistration(Weak<Mutex<PainterState>>);

impl GpuPainterRegistration {
    /// Checks that this registration uses the renderer's native adapter.
    pub fn is<T: GpuPainter>(&self) -> bool {
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
    pub handle: GpuPainterHandle,
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
        GpuPaintPrimitive, GpuPainter, GpuPainterHandle, GpuPainterRegistration,
        GpuPainterRegistry, GpuPainterStatus, GpuPreparationPhase, GpuResetReason, PainterState,
        prepare_gpu_paints,
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
        for (index, surface) in surfaces.iter().enumerate() {
            if recovery.is_ready(index) {
                surface
                    .draw
                    .handle
                    .invoke::<RecordingPainter>(|_| Ok(()))
                    .expect("draw");
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
        assert!(handle.status().last_error.is_none());
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

//! Native GPU access for application painter callbacks.

use crate::{GpuCapabilities, GpuPaintTarget};

/// Native Metal resources borrowed before scene drawing begins.
///
/// Use application-owned targets for offscreen drawing. Do not submit or present,
/// retain frame resources, reenter GPUI, or leave an encoder active on return.
#[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
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
/// replaced; use [`crate::GpuDrawMode::OwnPass`] for depth, multisampling or other passes.
/// Do not end the pass, retain its resources, or reenter GPUI.
#[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
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
#[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
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

/// Native Wgpu resources borrowed before scene drawing begins.
///
/// Use application-owned targets for offscreen drawing. Do not submit or present,
/// retain frame resources, reenter GPUI, or leave an encoder active on return.
#[cfg(feature = "gpu-wgpu")]
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
/// replaced; use [`crate::GpuDrawMode::OwnPass`] for depth, multisampling or other passes.
/// Do not end the pass, retain its resources, or reenter GPUI.
#[cfg(feature = "gpu-wgpu")]
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
#[cfg(feature = "gpu-wgpu")]
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

/// Native D3D11 resources borrowed before scene drawing begins.
///
/// Use application-owned targets for offscreen drawing. Do not submit or present,
/// retain frame resources, reenter GPUI, or leave an encoder active on return.
#[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
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
#[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
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

/// Native preparation access. Use application-owned targets for offscreen work.
/// End all application encoders before returning; never submit the frame.
#[non_exhaustive]
pub enum GpuPrepareContext<'a> {
    /// Metal device and command buffer.
    #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
    Metal(MetalPrepareContext<'a>),
    /// wgpu device, upload queue and command encoder.
    #[cfg(feature = "gpu-wgpu")]
    Wgpu(WgpuPrepareContext<'a>),
    /// State-isolated D3D11 device and immediate context.
    #[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
    D3D11(D3D11PrepareContext<'a>),
    #[doc(hidden)]
    #[cfg(not(any(
        feature = "gpu-wgpu",
        all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")),
        all(feature = "gpu-d3d11", target_os = "windows")
    )))]
    Unavailable(&'a std::convert::Infallible),
}

/// Access to GPUI's existing pass. Attachments and pass lifetime belong to GPUI.
#[non_exhaustive]
pub enum GpuPassContext<'a, 'pass> {
    /// Borrowed Metal render encoder.
    #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
    Metal(&'a mut MetalDrawContext<'pass>),
    /// Borrowed wgpu render pass.
    #[cfg(feature = "gpu-wgpu")]
    Wgpu(WgpuDrawContext<'a, 'pass>),
    #[doc(hidden)]
    #[cfg(not(any(
        feature = "gpu-wgpu",
        all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios"))
    )))]
    Unavailable(&'a &'pass std::convert::Infallible),
}

/// Independent drawing access at the surface's position in the scene.
/// Preserve the color target, clip writes and apply inherited opacity.
#[non_exhaustive]
pub enum GpuPaintContext<'a> {
    /// Metal command buffer and color attachment.
    #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
    Metal(MetalPaintContext<'a>),
    /// wgpu command encoder and color attachment.
    #[cfg(feature = "gpu-wgpu")]
    Wgpu(WgpuPaintContext<'a>),
    /// State-isolated D3D11 context and color attachment.
    #[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
    D3D11(D3D11PaintContext<'a>),
    #[doc(hidden)]
    #[cfg(not(any(
        feature = "gpu-wgpu",
        all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")),
        all(feature = "gpu-d3d11", target_os = "windows")
    )))]
    Unavailable(&'a std::convert::Infallible),
}

impl GpuPrepareContext<'_> {
    /// Geometry and opacity for this surface.
    pub fn target(&self) -> GpuPaintTarget {
        match self {
            #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
            Self::Metal(context) => context.target,
            #[cfg(feature = "gpu-wgpu")]
            Self::Wgpu(context) => context.target,
            #[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
            Self::D3D11(context) => context.target,
            #[cfg(not(any(
                feature = "gpu-wgpu",
                all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")),
                all(feature = "gpu-d3d11", target_os = "windows")
            )))]
            Self::Unavailable(never) => match **never {},
        }
    }
    /// Drawing access supported by this backend.
    pub fn capabilities(&self) -> GpuCapabilities {
        match self {
            #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
            Self::Metal(_) => crate::GpuBackend::Metal.capabilities(),
            #[cfg(feature = "gpu-wgpu")]
            Self::Wgpu(_) => crate::GpuBackend::Wgpu.capabilities(),
            #[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
            Self::D3D11(_) => crate::GpuBackend::D3D11.capabilities(),
            #[cfg(not(any(
                feature = "gpu-wgpu",
                all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")),
                all(feature = "gpu-d3d11", target_os = "windows")
            )))]
            Self::Unavailable(never) => match **never {},
        }
    }
}

impl GpuPassContext<'_, '_> {
    /// Geometry and opacity for this surface.
    pub fn target(&self) -> GpuPaintTarget {
        match self {
            #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
            Self::Metal(context) => context.target,
            #[cfg(feature = "gpu-wgpu")]
            Self::Wgpu(context) => context.target,
            #[cfg(not(any(
                feature = "gpu-wgpu",
                all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios"))
            )))]
            Self::Unavailable(never) => match ***never {},
        }
    }
    /// Drawing access supported by this backend.
    pub fn capabilities(&self) -> GpuCapabilities {
        match self {
            #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
            Self::Metal(_) => crate::GpuBackend::Metal.capabilities(),
            #[cfg(feature = "gpu-wgpu")]
            Self::Wgpu(_) => crate::GpuBackend::Wgpu.capabilities(),
            #[cfg(not(any(
                feature = "gpu-wgpu",
                all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios"))
            )))]
            Self::Unavailable(never) => match ***never {},
        }
    }
}

impl GpuPaintContext<'_> {
    /// Geometry and opacity for this surface.
    pub fn target(&self) -> GpuPaintTarget {
        match self {
            #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
            Self::Metal(context) => context.target,
            #[cfg(feature = "gpu-wgpu")]
            Self::Wgpu(context) => context.target,
            #[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
            Self::D3D11(context) => context.target,
            #[cfg(not(any(
                feature = "gpu-wgpu",
                all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")),
                all(feature = "gpu-d3d11", target_os = "windows")
            )))]
            Self::Unavailable(never) => match **never {},
        }
    }
    /// Drawing access supported by this backend.
    pub fn capabilities(&self) -> GpuCapabilities {
        match self {
            #[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
            Self::Metal(_) => crate::GpuBackend::Metal.capabilities(),
            #[cfg(feature = "gpu-wgpu")]
            Self::Wgpu(_) => crate::GpuBackend::Wgpu.capabilities(),
            #[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
            Self::D3D11(_) => crate::GpuBackend::D3D11.capabilities(),
            #[cfg(not(any(
                feature = "gpu-wgpu",
                all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")),
                all(feature = "gpu-d3d11", target_os = "windows")
            )))]
            Self::Unavailable(never) => match **never {},
        }
    }
}

#[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
impl<'a> GpuPrepareContext<'a> {
    /// Borrows the Metal resources, or reports a backend mismatch.
    pub fn metal(&mut self) -> anyhow::Result<&mut MetalPrepareContext<'a>> {
        match self {
            Self::Metal(context) => Ok(context),
            // A build may enable only this backend.
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("Expected Metal GPU context"),
        }
    }
}

#[cfg(feature = "gpu-wgpu")]
impl<'a> GpuPrepareContext<'a> {
    /// Borrows the Wgpu resources, or reports a backend mismatch.
    pub fn wgpu(&mut self) -> anyhow::Result<&mut WgpuPrepareContext<'a>> {
        match self {
            Self::Wgpu(context) => Ok(context),
            // A build may enable only this backend.
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("Expected Wgpu GPU context"),
        }
    }
}

#[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
impl<'a> GpuPrepareContext<'a> {
    /// Borrows the D3D11 resources, or reports a backend mismatch.
    pub fn d3d11(&mut self) -> anyhow::Result<&mut D3D11PrepareContext<'a>> {
        match self {
            Self::D3D11(context) => Ok(context),
            // A build may enable only this backend.
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("Expected D3D11 GPU context"),
        }
    }
}

#[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
impl<'a, 'pass> GpuPassContext<'a, 'pass> {
    /// Borrows the Metal resources, or reports a backend mismatch.
    pub fn metal(&mut self) -> anyhow::Result<&mut MetalDrawContext<'pass>> {
        match self {
            Self::Metal(context) => Ok(context),
            // A build may enable only this backend.
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("Expected Metal GPU context"),
        }
    }
}

#[cfg(feature = "gpu-wgpu")]
impl<'a, 'pass> GpuPassContext<'a, 'pass> {
    /// Borrows the Wgpu resources, or reports a backend mismatch.
    pub fn wgpu(&mut self) -> anyhow::Result<&mut WgpuDrawContext<'a, 'pass>> {
        match self {
            Self::Wgpu(context) => Ok(context),
            // A build may enable only this backend.
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("Expected Wgpu GPU context"),
        }
    }
}

#[cfg(all(feature = "gpu-metal", any(target_os = "macos", target_os = "ios")))]
impl<'a> GpuPaintContext<'a> {
    /// Borrows the Metal resources, or reports a backend mismatch.
    pub fn metal(&mut self) -> anyhow::Result<&mut MetalPaintContext<'a>> {
        match self {
            Self::Metal(context) => Ok(context),
            // A build may enable only this backend.
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("Expected Metal GPU context"),
        }
    }
}

#[cfg(feature = "gpu-wgpu")]
impl<'a> GpuPaintContext<'a> {
    /// Borrows the Wgpu resources, or reports a backend mismatch.
    pub fn wgpu(&mut self) -> anyhow::Result<&mut WgpuPaintContext<'a>> {
        match self {
            Self::Wgpu(context) => Ok(context),
            // A build may enable only this backend.
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("Expected Wgpu GPU context"),
        }
    }
}

#[cfg(all(feature = "gpu-d3d11", target_os = "windows"))]
impl<'a> GpuPaintContext<'a> {
    /// Borrows the D3D11 resources, or reports a backend mismatch.
    pub fn d3d11(&mut self) -> anyhow::Result<&mut D3D11PaintContext<'a>> {
        match self {
            Self::D3D11(context) => Ok(context),
            // A build may enable only this backend.
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("Expected D3D11 GPU context"),
        }
    }
}

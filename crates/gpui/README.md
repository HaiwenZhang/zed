# Welcome to GPUI!

GPUI is a hybrid immediate and retained mode, GPU accelerated, UI framework
for Rust, designed to support a wide variety of applications.

## Getting Started

GPUI is still in active development as we work on the Zed code editor, and is still pre-1.0. There will often be breaking changes between versions. You'll also need to use the latest version of stable Rust. Add `gpui`, and optionally `gpui_platform`, to your `Cargo.toml`:

```toml
gpui = { version = "*" }
gpui_platform = { version = "*", features = ["font-kit", "wayland", "x11"] }
```

Everything in a standalone GPUI app starts with an `Application`. You can create one with `gpui_platform::application()`, which picks the windowing and text backends for the host OS, and kick off your application by passing a callback to `Application::run()`. Inside this callback, you can create a new window with `App::open_window()` and register your first root view.

```rust,no_run
use gpui::*;

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        // ..
    });
}
```

### `gpui_platform`

The features on `gpui_platform` are platform-specific, so the list above is a safe cross-platform default. If you build for a single platform, you can trim it:

- **macOS** — Rendering uses Metal and is always available, but glyph rasterization needs `font-kit`. Without it, GPUI falls back to a placeholder text system that lays text out but renders no glyphs.

    ```toml
    gpui_platform = { version = "*", features = ["font-kit"] }
    ```

- **Linux / FreeBSD** — enable at least one windowing backend for desktop windows: `wayland`, `x11`, or both. These features also compile the renderer and text system, so no separate text feature is needed.

    ```toml
    gpui_platform = { version = "*", features = ["wayland", "x11"] }
    ```

- **Windows** — no features are required. Windowing uses Win32 and text uses DirectWrite. `font-kit` has no effect here.

### Additional Topics

- [Ownership and data flow](_ownership_and_data_flow)
- [Accessibility](_accessibility)

### Dependencies

GPUI has various system dependencies that it needs in order to work.

#### macOS

On macOS, GPUI uses Metal for rendering. In order to use Metal, you need to do the following:

- Install [Xcode](https://apps.apple.com/us/app/xcode/id497799835?mt=12) from the macOS App Store, or from the [Apple Developer](https://developer.apple.com/download/all/) website. Note this requires a developer account.

> Ensure you launch Xcode after installing, and install the macOS components, which is the default option.

- Install [Xcode command line tools](https://developer.apple.com/xcode/resources/)

  ```sh
  xcode-select --install
  ```

- Ensure that the Xcode command line tools are using your newly installed copy of Xcode:

  ```sh
  sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
  ```

## The Big Picture

GPUI offers three different [registers](<https://en.wikipedia.org/wiki/Register_(sociolinguistics)>) depending on your needs:

- State management and communication with `Entity`'s. Whenever you need to store application state that communicates between different parts of your application, you'll want to use GPUI's entities. Entities are owned by GPUI and are only accessible through an owned smart pointer similar to an `Rc`. See the `app::context` module for more information.

- High level, declarative UI with views. All UI in GPUI starts with a view. A view is simply an `Entity` that can be rendered, by implementing the `Render` trait. At the start of each frame, GPUI will call this render method on the root view of a given window. Views build a tree of `elements`, lay them out and style them with a tailwind-style API, and then give them to GPUI to turn into pixels. See the `div` element for an all purpose swiss-army knife of rendering.

- Low level, imperative UI with Elements. Elements are the building blocks of UI in GPUI, and they provide a nice wrapper around an imperative API that provides as much flexibility and control as you need. Elements have total control over how they and their child elements are rendered and can be used for making efficient views into large lists, implement custom layouting for a code editor, and anything else you can think of. See the `element` module for more information.

Each of these registers has one or more corresponding contexts that can be accessed from all GPUI services. This context is your main interface to GPUI, and is used extensively throughout the framework.

## Custom GPU rendering

Register a backend painter with `Window::register_gpu_painter`, then call
`Window::paint_gpu(bounds, &handle, snapshot)` during an element's paint phase.
The handle belongs to one window. The scene retains the immutable snapshot and
draws the surface in order with ordinary UI primitives, including cached scene
replays.

The native interfaces are `gpui_apple::MetalPainter`, `gpui_wgpu::WgpuPainter`
and `gpui_windows::D3D11Painter`, adapted with `metal_painter`, `wgpu_painter`
and `d3d11_painter`. Each supports these stages:

| Stage | Execution | Typical work |
| --- | --- | --- |
| `prepare` | Before ordinary scene drawing, once per visible surface | Upload geometry, allocate targets, run compute or offscreen passes |
| `finish_prepare` | After all surfaces have prepared, once per successfully prepared surface | Work that depends on preparation of other surfaces |
| `paint` | At the surface's position in the scene | Draw directly or composite an offscreen result |

Preparation methods default to success without doing any work. Existing painters
implementing only `paint` and `reset` continue to work. A preparation error skips
the surface's remaining stages; errors are logged and available through
`GpuPainterHandle::status()`. Its `encoded` counter counts successful `paint`
callbacks, not preparation or GPU completion.

For direct rendering, use the provided command encoder/buffer or D3D11 context
to create your own pass targeting the window's color attachment. Preserve its
existing contents, obey `target.clip`, and apply `target.opacity`. An
application-owned depth attachment must match the color target's full dimensions
and `target.sample_count`. The pipeline must also match the target format and
sample count. GPUI resumes ordinary UI rendering after your callback.

For an offscreen CAD viewport, own the color, depth, multisample and any picking
attachments in your painter. Render them during preparation, resolve multisampled
color if needed, and composite the result during `paint`. Offscreen attachments
can use a different resolution and sample count from the window. Apply clipping
and inherited opacity during the final composition. Additional passes can also
be encoded during `paint` when they must execute at that position in the scene.
GPUI does not supply a CAD renderer or manage these application-owned attachments.
Direct and offscreen surfaces can coexist in the same window. Both paths can
update every frame; offscreen rendering additionally allows reuse of a completed
texture when viewport content has not changed.

Callbacks execute serially. Never submit or present the frame, reenter GPUI,
retain borrowed frame targets, or leave an application encoder active on return,
including error returns. Encoding is not GPU completion: caches and asynchronous
readbacks must account for frames that fail or are abandoned before submission.
Keep separate resources or buffer ranges for different snapshots when multiple
surfaces share a painter. Preparation runs again on scene replays and encoding
retries. On device replacement, `reset` releases resources and subsequent callbacks
recreate them on the supplied device; resize alone does not invoke `reset`.

The macOS Metal example demonstrates an animated, depth-tested viewport with
switchable direct and offscreen rendering, MSAA resolve, clipping, opacity and
offscreen caching. Run it from the repository root:

```sh
cargo run -p gpui_apple --example cad_viewport --features runtime_shaders
```

Click the rendering mode to switch paths and pause animation to reuse the
offscreen result. The example uses one painter for one viewport; multiple
independent viewports can each register their own painter.

## Other Resources

In addition to the systems above, GPUI provides a range of smaller services that are useful for building complex applications:

- Actions are user-defined structs that are used for converting keystrokes into logical operations in your UI. Use this for implementing keyboard shortcuts, such as cmd-q. See the `action` module for more information.

- Platform services, such as `quit the app` or `open a URL` are available as methods on the `app::App`.

- An async executor that is integrated with the platform's event loop. See the `executor` module for more information.,

- The `[gpui::test]` macro provides a convenient way to write tests for your GPUI applications. Tests also have their own kind of context, a `TestAppContext` which provides ways of simulating common platform input. See `app::test_context` and `test` modules for more details.

Currently, the best way to learn about these APIs is to read the Zed source code or drop a question in the [Zed Discord](https://zed.dev/community-links). We're working on improving the documentation, creating more examples, and will be publishing more guides to GPUI on our [blog](https://zed.dev/blog).

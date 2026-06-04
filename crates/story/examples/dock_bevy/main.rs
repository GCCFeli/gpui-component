//! Dock + Bevy 3D — based on the `dock` story example, with one extra tab that
//! hosts a live Bevy 3D view.
//!
//! The Bevy view renders a spinning cube into a texture on GPUI's *shared* wgpu
//! device; the `BevyPanel` composites that texture into its own dock-panel
//! bounds each frame (zero copy) via `Window::set_wgpu_overlays`. Drag the tab,
//! resize the dock, or move it to another dock region — the 3D view follows.
//!
//! Run (main() also sets these if unset):
//!   $env:GPUI_RENDERER = "wgpu"; cargo run --example dock_bevy

mod bevy_view;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use gpui_component::{
    Root,
    dock::{ClosePanel, DockArea, DockItem, Panel, PanelControl, PanelEvent, ToggleZoom},
};
use gpui_component_assets::Assets;
use gpui_component_story::{
    AppTitleBar, ButtonStory, InputStory, ListStory, StoryContainer, TooltipStory,
};

const MAIN_DOCK_AREA: DockAreaTab = DockAreaTab {
    id: "dock-bevy",
    version: 1,
};

struct DockAreaTab {
    id: &'static str,
    version: usize,
}

// ----------------------------------------------------------------------------
// The Bevy 3D dock panel
// ----------------------------------------------------------------------------

struct BevyPanel {
    focus_handle: FocusHandle,
    bevy: Option<bevy_view::BevyView>,
    frame: u64,
    /// Device-pixel size the panel last laid out at, written by the canvas paint
    /// closure and consumed by `render` to resize the Bevy target (so its aspect
    /// ratio matches the panel and the image isn't stretched).
    size_request: Rc<Cell<Option<(u32, u32)>>>,
}

impl BevyPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bevy = match window.wgpu_shared() {
            Some(shared) => {
                println!(
                    "dock_bevy: shared wgpu device {} ({:?})",
                    shared.adapter_info.name, shared.adapter_info.backend
                );
                Some(bevy_view::BevyView::new(&shared, 1024, 768))
            }
            None => {
                eprintln!(
                    "dock_bevy: wgpu renderer not active (run with GPUI_RENDERER=wgpu); \
                     the Bevy panel will be empty."
                );
                None
            }
        };

        Self {
            focus_handle: cx.focus_handle(),
            bevy,
            frame: 0,
            size_request: Rc::new(Cell::new(None)),
        }
    }
}

impl EventEmitter<PanelEvent> for BevyPanel {}

impl Focusable for BevyPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BevyPanel {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Advance Bevy and keep animating while this panel is visible.
        let overlay_view = if let Some(bevy) = self.bevy.as_mut() {
            // Resize the render target to the size the panel laid out at last
            // frame, so the camera aspect ratio matches (no stretching).
            if let Some((w, h)) = self.size_request.get() {
                bevy.resize(w, h);
            }
            bevy.update();
            self.frame += 1;
            window.request_animation_frame();
            Some(bevy.gpui_view.clone())
        } else {
            None
        };
        let has_bevy = overlay_view.is_some();
        let size_request = self.size_request.clone();

        div()
            .id("bevy-panel")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .bg(rgb(0x0b0b12))
            .child(
                // The canvas captures this panel's on-screen bounds during paint:
                // it records the device-pixel size for next frame's resize and
                // asks the renderer to composite the Bevy texture into `bounds`.
                canvas(
                    move |_bounds, _window, _cx| {},
                    move |bounds, _, window, _cx| {
                        let scale = window.scale_factor();
                        let w = (f32::from(bounds.size.width) * scale).round() as u32;
                        let h = (f32::from(bounds.size.height) * scale).round() as u32;
                        size_request.set(Some((w.max(1), h.max(1))));

                        // Paint the Bevy texture into the scene at THIS panel's
                        // z-layer. It's the panel's base layer — the tab bar,
                        // dialogs, drag previews and anything painted later
                        // composite on top of it, and it's clipped to the panel.
                        if let Some(view) = overlay_view {
                            window.paint_external_texture(bounds, view);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .children((!has_bevy).then(|| {
                div()
                    .p_4()
                    .text_color(rgb(0xdddddd))
                    .child("wgpu renderer not active — run with GPUI_RENDERER=wgpu")
            }))
    }
}

impl Panel for BevyPanel {
    fn panel_name(&self) -> &'static str {
        "BevyPanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Bevy 3D")
    }

    fn zoomable(&self, _cx: &App) -> Option<PanelControl> {
        Some(PanelControl::Menu)
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        // Resume the render/animation loop when shown. When hidden, the panel
        // simply isn't painted, so no texture is emitted — nothing to clear.
        if active {
            cx.notify();
        }
    }
}

// ----------------------------------------------------------------------------
// Workspace (dock area), based on the `dock` story example without persistence.
// ----------------------------------------------------------------------------

struct StoryWorkspace {
    dock_area: Entity<DockArea>,
    title_bar: Entity<AppTitleBar>,
}

impl StoryWorkspace {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dock_area =
            cx.new(|cx| DockArea::new(MAIN_DOCK_AREA.id, Some(MAIN_DOCK_AREA.version), window, cx));
        Self::reset_default_layout(dock_area.downgrade(), window, cx);

        let title_bar = cx.new(|cx| AppTitleBar::new("Dock + Bevy 3D", window, cx));

        Self {
            dock_area,
            title_bar,
        }
    }

    fn reset_default_layout(dock_area: WeakEntity<DockArea>, window: &mut Window, cx: &mut App) {
        // Center: the Bevy 3D tab first, then a couple of regular stories so you
        // can tab between a wgpu/Bevy view and ordinary GPUI components.
        let bevy_panel = cx.new(|cx| BevyPanel::new(window, cx));
        // Wrap each dock region's tabs in a v_split (StackPanel container), the
        // same way the original `dock` story does — the dock's drag/drop expects
        // a split container at the top of each region.
        let center = DockItem::v_split(
            vec![DockItem::tabs(
                vec![
                    Arc::new(bevy_panel),
                    Arc::new(StoryContainer::panel::<ButtonStory>(window, cx)),
                    Arc::new(StoryContainer::panel::<InputStory>(window, cx)),
                ],
                &dock_area,
                window,
                cx,
            )],
            &dock_area,
            window,
            cx,
        );

        let left = DockItem::v_split(
            vec![DockItem::tab(
                StoryContainer::panel::<ListStory>(window, cx),
                &dock_area,
                window,
                cx,
            )],
            &dock_area,
            window,
            cx,
        );

        let bottom = DockItem::v_split(
            vec![DockItem::tabs(
                vec![Arc::new(StoryContainer::panel::<TooltipStory>(window, cx))],
                &dock_area,
                window,
                cx,
            )],
            &dock_area,
            window,
            cx,
        );

        _ = dock_area.update(cx, |view, cx| {
            view.set_version(MAIN_DOCK_AREA.version, window, cx);
            view.set_center(center, window, cx);
            view.set_left_dock(left, Some(px(300.)), true, window, cx);
            view.set_bottom_dock(bottom, Some(px(200.)), true, window, cx);
        });
    }
}

impl Render for StoryWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notification_layer = Root::render_notification_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);

        div()
            .id("dock-bevy-workspace")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(self.title_bar.clone())
            .child(self.dock_area.clone())
            .children(dialog_layer)
            .children(notification_layer)
    }
}

fn open_new(cx: &mut App) -> Task<()> {
    let mut window_size = size(px(1400.0), px(900.0));
    if let Some(display) = cx.primary_display() {
        let display_size = display.bounds().size;
        window_size.width = window_size.width.min(display_size.width * 0.85);
        window_size.height = window_size.height.min(display_size.height * 0.85);
    }
    let window_bounds = Bounds::centered(None, window_size, cx);

    cx.spawn(async move |cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            #[cfg(not(target_os = "linux"))]
            titlebar: Some(gpui_component::TitleBar::title_bar_options()),
            window_min_size: Some(gpui::Size {
                width: px(640.),
                height: px(480.),
            }),
            kind: WindowKind::Normal,
            ..Default::default()
        };

        let window = cx
            .open_window(options, |window, cx| {
                let workspace = cx.new(|cx| StoryWorkspace::new(window, cx));
                cx.new(|cx| Root::new(workspace, window, cx))
            })
            .expect("failed to open window");

        window
            .update(cx, |_, window, cx| {
                window.activate_window();
                window.set_window_title("Dock + Bevy 3D");
                cx.on_release(|_, cx| cx.quit()).detach();
            })
            .expect("failed to update window");
    })
}

fn init(cx: &mut App) {
    gpui_component_story::init(cx);
    cx.bind_keys(vec![
        KeyBinding::new("shift-escape", ToggleZoom, None),
        KeyBinding::new("ctrl-w", ClosePanel, None),
    ]);
    cx.activate(true);
}

fn main() {
    // Make `cargo run --example dock_bevy` work out of the box: select the wgpu
    // renderer and request the GPU's full limits so Bevy (sharing the device)
    // isn't constrained to GPUI's downlevel defaults.
    if std::env::var_os("GPUI_RENDERER").is_none() {
        unsafe { std::env::set_var("GPUI_RENDERER", "wgpu") };
    }
    if std::env::var_os("GPUI_WGPU_FULL_DEVICE").is_none() {
        unsafe { std::env::set_var("GPUI_WGPU_FULL_DEVICE", "1") };
    }

    let app = gpui_platform::application().with_assets(Assets);
    app.run(move |cx| {
        init(cx);
        open_new(cx).detach();
    });
}

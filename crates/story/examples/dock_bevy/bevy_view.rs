//! A headless Bevy app that shares GPUI's wgpu device and renders a spinning
//! PBR cube into an offscreen texture. The texture (`gpui_view`) is composited
//! into a GPUI dock panel each frame, zero-copy. Frames are driven manually via
//! `BevyView::update()` so GPUI controls pacing.

use std::sync::Arc;

use bevy::app::{App, TaskPoolPlugin};
use bevy::asset::{AssetPlugin, Assets};
use bevy::camera::{Camera3d, CameraPlugin, ManualTextureViewHandle, RenderTarget};
use bevy::color::Color;
use bevy::core_pipeline::CorePipelinePlugin;
use bevy::diagnostic::FrameCountPlugin;
use bevy::image::ImagePlugin;
use bevy::math::primitives::Cuboid;
use bevy::math::{UVec2, Vec3};
use bevy::mesh::{Mesh, Mesh3d, MeshPlugin};
use bevy::pbr::{MeshMaterial3d, PbrPlugin, StandardMaterial};
use bevy::prelude::{Component, Query, Res, Transform, With};
use bevy::render::RenderPlugin;
use bevy::render::render_resource::TextureView as BevyTextureView;
use bevy::render::renderer::{
    RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue, WgpuWrapper,
};
use bevy::render::settings::RenderCreation;
use bevy::render::texture::{ManualTextureView, ManualTextureViews};
use bevy::time::{Time, TimePlugin};
use bevy::transform::TransformPlugin;
use bevy::window::{ExitCondition, WindowPlugin};

const TARGET_HANDLE: ManualTextureViewHandle = ManualTextureViewHandle(0);
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[derive(Component)]
struct Spin;

/// A headless Bevy world rendering a spinning cube into a GPUI-shared texture.
pub struct BevyView {
    app: App,
    device: Arc<wgpu::Device>,
    #[allow(dead_code)]
    texture: wgpu::Texture,
    /// A view GPUI samples. Same texture Bevy renders into.
    pub gpui_view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl BevyView {
    /// Builds the headless Bevy app on GPUI's shared device, rendering into a
    /// freshly-created `width`x`height` texture.
    fn create_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bevy_render_target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    pub fn new(shared: &gpui::SharedWgpu, width: u32, height: u32) -> Self {
        let texture = Self::create_target(&shared.device, width, height);
        let bevy_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let gpui_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let render_creation = RenderCreation::manual(
            RenderDevice::from(shared.device.as_ref().clone()),
            RenderQueue(Arc::new(WgpuWrapper::new(shared.queue.as_ref().clone()))),
            RenderAdapterInfo(WgpuWrapper::new(shared.adapter_info.clone())),
            RenderAdapter(Arc::new(WgpuWrapper::new(shared.adapter.clone()))),
            RenderInstance(Arc::new(WgpuWrapper::new(shared.instance.clone()))),
        );

        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            TimePlugin,
            FrameCountPlugin,
            TransformPlugin,
            WindowPlugin {
                primary_window: None,
                primary_cursor_options: None,
                exit_condition: ExitCondition::DontExit,
                close_when_requested: false,
            },
            AssetPlugin::default(),
            MeshPlugin,
            ImagePlugin::default(),
            CameraPlugin,
            RenderPlugin {
                render_creation,
                synchronous_pipeline_compilation: true,
                ..Default::default()
            },
            CorePipelinePlugin,
            bevy::light::LightPlugin,
            PbrPlugin::default(),
        ));
        app.add_systems(bevy::app::Update, rotate_cube);

        {
            let world = app.world_mut();
            world.resource_mut::<ManualTextureViews>().insert(
                TARGET_HANDLE,
                ManualTextureView {
                    texture_view: BevyTextureView::from(bevy_view),
                    size: UVec2::new(width, height),
                    view_format: TARGET_FORMAT,
                },
            );

            let mesh = world.resource_mut::<Assets<Mesh>>().add(Cuboid::new(1.6, 1.6, 1.6));
            let material = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: Color::srgb(0.35, 0.6, 0.95),
                    ..Default::default()
                });
            world.spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::default(), Spin));

            world.spawn((
                bevy::light::DirectionalLight::default(),
                Transform::from_xyz(4.0, 8.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
            ));

            world.spawn((
                Camera3d::default(),
                bevy::core_pipeline::tonemapping::Tonemapping::None,
                bevy::core_pipeline::tonemapping::DebandDither::Disabled,
                RenderTarget::TextureView(TARGET_HANDLE),
                bevy::light::AmbientLight {
                    color: Color::WHITE,
                    brightness: 600.0,
                    ..Default::default()
                },
                Transform::from_xyz(0.0, 1.6, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            ));
        }

        app.finish();
        app.cleanup();

        Self {
            app,
            device: shared.device.clone(),
            texture,
            gpui_view,
            width,
            height,
        }
    }

    /// Resizes the render target to `width`x`height` (device pixels) so the
    /// camera's aspect ratio matches the panel and the image isn't stretched.
    /// No-op if the size is unchanged. The Bevy camera picks up the new size
    /// from `ManualTextureViews` on the next frame.
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return;
        }

        let texture = Self::create_target(&self.device, width, height);
        let bevy_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let gpui_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        self.app
            .world_mut()
            .resource_mut::<ManualTextureViews>()
            .insert(
                TARGET_HANDLE,
                ManualTextureView {
                    texture_view: BevyTextureView::from(bevy_view),
                    size: UVec2::new(width, height),
                    view_format: TARGET_FORMAT,
                },
            );

        self.texture = texture;
        self.gpui_view = gpui_view;
        self.width = width;
        self.height = height;
    }

    /// Advances the Bevy world by one frame, rendering into the shared texture.
    pub fn update(&mut self) {
        self.app.update();
    }
}

fn rotate_cube(time: Res<Time>, mut query: Query<&mut Transform, With<Spin>>) {
    let dt = time.delta_secs();
    for mut transform in &mut query {
        transform.rotate_y(dt * 0.9);
        transform.rotate_x(dt * 0.5);
    }
}

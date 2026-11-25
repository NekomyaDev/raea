mod bake;
mod extract;
// raea: the denoiser guides (primary surface replacement) without DLSS
mod guides;
mod node;
mod prepare;
// raea: RAEA_SOLARI_TIMES=1, each stage's GPU time
mod times;

use crate::{
    scene::{prepare_raytracing_scene_resources, RaytracingSceneBindings},
    SolariPlugins,
};
use bevy_app::{App, Plugin, PostUpdate};
use bevy_asset::embedded_asset;
use bevy_camera::Hdr;
use bevy_core_pipeline::{
    core_3d::main_opaque_pass_3d,
    prepass::{
        DeferredPrepass, DeferredPrepassDoubleBuffer, DepthPrepass, DepthPrepassDoubleBuffer,
        MotionVectorPrepass,
    },
    schedule::{Core3d, Core3dSystems},
};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    query::Has,
    reflect::ReflectComponent,
    schedule::IntoScheduleConfigs,
    system::{Commands, Query},
};
use bevy_pbr::DefaultOpaqueRendererMethod;
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render::{
    init_gpu_resource, renderer::RenderDevice, ExtractSchedule, Render, RenderApp, RenderStartup,
    RenderSystems,
};
use bevy_shader::load_shader_library;
use extract::extract_solari_lighting;
use node::{init_solari_lighting_pipelines, solari_lighting};
use prepare::{
    prepare_solari_lighting_resources, setup_raytracing_scene_needs_previous_frame_data,
};
use tracing::warn;

pub use guides::{SolariGuideBuffers, SolariGuideTextures};

/// Raytraced direct and indirect lighting.
///
/// When using this plugin, it's highly recommended to set `shadow_maps_enabled: false` on all lights, as Solari replaces
/// traditional shadow mapping.
pub struct SolariLightingPlugin;

impl Plugin for SolariLightingPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "gbuffer_utils.wesl");
        load_shader_library!(app, "bindings.wesl");
        load_shader_library!(app, "presample_light_tiles.wesl");
        load_shader_library!(app, "initial_path.wesl");
        embedded_asset!(app, "restir.wesl");
        embedded_asset!(app, "no_restir.wesl");
        load_shader_library!(app, "world_cache_query.wesl");
        embedded_asset!(app, "world_cache_compact.wesl");
        embedded_asset!(app, "world_cache_update.wesl");

        load_shader_library!(app, "resolve_dlss_rr_textures.wesl");

        app.insert_resource(DefaultOpaqueRendererMethod::deferred());
    }

    fn finish(&self, app: &mut App) {
        let features = app
            .sub_app(RenderApp)
            .world()
            .resource::<RenderDevice>()
            .features();
        if !features.contains(SolariPlugins::required_wgpu_features()) {
            warn!(
                "SolariLightingPlugin not loaded. GPU lacks support for required features: {:?}.",
                SolariPlugins::required_wgpu_features().difference(features)
            );
            return;
        }

        app.add_systems(PostUpdate, manage_prepass_double_buffers);

        app.sub_app_mut(RenderApp)
            .add_systems(
                RenderStartup,
                init_solari_lighting_pipelines.after(init_gpu_resource::<RaytracingSceneBindings>),
            )
            .add_systems(ExtractSchedule, extract_solari_lighting)
            .add_systems(
                Render,
                (
                    prepare_solari_lighting_resources,
                    setup_raytracing_scene_needs_previous_frame_data
                        .before(prepare_raytracing_scene_resources),
                )
                    .in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Core3d,
                solari_lighting
                    .before(main_opaque_pass_3d)
                    .in_set(Core3dSystems::MainPass),
            );
    }
}

/// A component for a 3d camera entity to enable the Solari raytraced lighting system.
///
/// Must be used with `CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING)`, and
/// `Msaa::Off`.
#[derive(Component, Reflect, Clone)]
#[reflect(Component, Default, Clone)]
#[require(Hdr, DeferredPrepass, DepthPrepass, MotionVectorPrepass)]
pub struct SolariLighting {
    /// [ReSTIR](https://en.wikipedia.org/wiki/Spatiotemporal_reservoir_resampling) is a technique to reuse path samples
    /// between pixels and frames. This dramatically reduces noise, at the cost of a few extra rays per pixel.
    ///
    /// However, modern denoisers cope well with very noisy input. In many cases, turning this on
    /// won't dramatically improve quality after denoising.
    ///
    /// If you want more fine shadow detail, or have scenes with more difficult lighting conditions,
    /// turning this on may improve quality and stability, at the cost of a decent chunk of performance.
    ///
    /// Whether to enable this setting or not will be very scene dependent.
    ///
    /// Defaults to `false`.
    pub restir: bool,

    /// Maximum confidence weight (effective temporal history length) a pixel
    /// can accumulate during temporal resampling.
    ///
    /// Has no effect when [`SolariLighting::restir`] is `false`.
    ///
    /// Higher values are more stable but slower to react to lighting changes
    /// and will lead to increased artifacts.
    pub confidence_weight_cap: f32,

    /// Number of direct light samples taken for the camera's primary hit during
    /// initial sampling.
    ///
    /// Higher values reduce noise in directly-lit areas at the cost of more work
    /// per frame. Lower values are faster but noisier.
    pub primary_di_samples: u32,

    /// Number of direct light samples taken at each indirect bounce during
    /// initial sampling.
    ///
    /// Higher values reduce noise in indirect lighting at the cost of more work
    /// per frame. Lower values are faster but noisier.
    pub secondary_di_samples: u32,

    /// Maximum number of bounces traced when generating an initial path.
    ///
    /// Higher values capture more detail in nested reflections and more indirect lighting,
    /// at the cost of more rays traced per frame. Lower values are faster but lose
    /// multi-bounce lighting for specular paths.
    pub max_bounces: u32,

    /// raea: the chance a pixel traces its first bounce this frame (Russian roulette on it). The bounce's light is
    /// divided by it, so the picture keeps its light on average, and ReSTIR's reuse spreads what a pixel skipped from
    /// its neighbours and last frames. The bounce is most of the frame's tracing in dense foliage: 0.5 halves it.
    ///
    /// Defaults to `1.0` (every pixel, every frame).
    pub first_bounce_probability: f32,

    /// raea (11-25): initial paths traced a pixel a frame, 1 to 16. One path a pixel on MALL RAT's sunlit court has a
    /// standard deviation as large as its mean (raea light steady, denoiser and ReSTIR off: white noise), which ReSTIR's
    /// reuse and the denoiser turn into patches that crawl. N paths cut that variance N times: their directly shaded
    /// light averaged, their reservoirs resampled into one (RIS over the N). Costs N times the initial path's rays.
    ///
    /// Defaults to `1`.
    pub paths_per_pixel: u32,
    /// raea (11-25): the firefly filter (RTXDI's boiling filter). After temporal reuse, a reservoir whose contribution
    /// is over this many times the mean of its 8x8 block has its weight cut to that. One very bright sample (a bounce
    /// landing beside a small lamp) is otherwise reused for many frames and spread to its neighbours: a patch jumps
    /// 30% in a frame and takes 30 frames to fall back. It costs the light those samples carried, so it's a trade:
    /// MALL RAT's party room, lit by bulbs, held still at 6 (boiling 1.63% to 0.73%) and read 0.29 stops darker; at 32
    /// nothing is lost and little is gained. RTXDI's usual strengths are 31 to 41.
    ///
    /// Defaults to `0.0` (off).
    pub firefly_cap: f32,

    /// How responsive the world cache is to changes in lighting.
    ///
    /// Higher values accumulate more temporal history, giving more stable but
    /// less responsive (slower to update) lighting. Lower values react faster
    /// but are noisier and less stable.
    pub world_cache_max_temporal_samples: f32,

    /// How many direct light samples each world cache cell takes when updating
    /// each frame.
    ///
    /// Higher values reduce noise in cached lighting at the cost of more work
    /// per frame. Lower values are faster but noisier.
    pub world_cache_direct_light_sample_count: u32,

    /// Maximum distance to trace GI rays between two world cache cells.
    ///
    /// Higher values capture indirect light from farther away for more accurate
    /// GI at the cost of longer (more expensive) ray traversal and increased noise.
    /// Lower values are faster and less noisy but may miss distant lighting or leak sky lighting.
    ///
    /// raea: a ray that misses within this distance counts the environment map light only if the rest of it
    /// reaches the sky too (an any-hit query past the cut), so a cut shorter than the scene no longer leaks sky light
    /// into the cache; one blocked past the cut adds nothing. Defaults to 1000 m (upstream 50 m, which flooded photo
    /// 5's hall with sky light).
    pub world_cache_max_gi_ray_distance: f32,

    /// Soft upper limit on the number of world cache cells to update each frame.
    ///
    /// Higher values let the cache converge faster after lighting changes at the
    /// cost of more work per frame. Lower values are cheaper but make the cache
    /// slower to update.
    ///
    /// This is a stochastic target that only takes effect when the number of
    /// active cells exceeds it: each active cell is then updated with
    /// probability `target / active_cells`, so on average this many cells
    /// update, though individual frames may update more or fewer. When there
    /// are fewer active cells than the target, all of them update every frame.
    pub world_cache_cell_updates_soft_target: u32,

    /// Size of a world cache cell at the lowest LOD, in meters.
    ///
    /// Smaller values give finer spatial resolution and more detailed indirect
    /// lighting at the cost of more cells to fill and update. Larger values are
    /// cheaper but coarser, which can cause light leaking.
    pub world_cache_position_base_cell_size: f32,

    /// How fast the world cache transitions between LODs as a function of
    /// distance to the camera.
    ///
    /// Higher values keep cells small (high detail) out to greater distances for
    /// better quality at the cost of more cells to fill. Lower values transition
    /// to larger cells sooner, which is cheaper but coarser farther from the
    /// camera.
    pub world_cache_position_lod_scale: f32,

    /// Set to true to delete the saved temporal history (past frames).
    ///
    /// Useful for preventing ghosting when the history is no longer
    /// representative of the current frame, such as in sudden camera cuts.
    ///
    /// After setting this to true, it will automatically be toggled
    /// back to false at the end of the frame.
    pub reset: bool,

    /// raea: the world cache baked (docs/solari-efficiency.md). A take's lights and sets hold still, so its cache can be
    /// gathered once, converged, and read back while it renders: a live cell takes one bounce ray a frame and keeps a
    /// short history, and each cell's own error is a blotch the denoiser spreads over floors, walls and reflections.
    ///
    /// Cells never age out while set: a bake keeps every cell its camera path meets. A reset still empties the cache.
    pub world_cache_keep: bool,
    /// raea: each cell keeps its whole history (up to `world_cache_max_temporal_samples`) instead of shortening it when
    /// its light changes, which a cell's own noise reads as. For a bake, with a long history.
    pub world_cache_steady: bool,
    /// raea: the cache's update (each cell's bounce sample, blended) run this many times a frame, each with its own
    /// random numbers while `world_cache_steady`. A bake converges in fewer frames. Defaults to 1.
    pub world_cache_update_iterations: u32,
    /// raea (11-25): how many frames a world cache cell lives without being queried, up to 65535; 0 is Solari's 10. A
    /// moving camera that leaves a place and comes back finds its cells dead after 10 frames and its light starting
    /// cold again: MALL RAT's court kept settling as the camera crossed it (raea light steady). Longer lives keep more
    /// cells, which the cache's 2^20 slots must hold.
    pub world_cache_cell_lifetime: u32,
    /// raea: write the whole cache to this file at the end of this frame (a bake, a few frames before its last). Empty:
    /// no write.
    pub world_cache_save: String,
    /// raea: read the cache from this file (a bake's `world_cache_save`) and hold it: no decay, no updates, and a reset
    /// doesn't empty it. Read again when the name changes (a take whose lights change at a cut: one bake each side).
    /// A cell the bake never met stays empty and a path that finds it carries on tracing. Empty: the live cache.
    pub world_cache_load: String,
}

impl Default for SolariLighting {
    fn default() -> Self {
        Self {
            restir: false,
            confidence_weight_cap: 8.0,
            primary_di_samples: 8,
            secondary_di_samples: 4,
            max_bounces: 3,
            first_bounce_probability: 1.0,
            paths_per_pixel: 1,
            firefly_cap: 0.0,
            world_cache_max_temporal_samples: 32.0,
            world_cache_direct_light_sample_count: 32,
            world_cache_max_gi_ray_distance: 1000.0,
            world_cache_cell_updates_soft_target: 40000,
            world_cache_position_base_cell_size: 0.15,
            world_cache_position_lod_scale: 15.0,
            reset: true, // No temporal history on the first frame
            world_cache_keep: false,
            world_cache_steady: false,
            world_cache_update_iterations: 1,
            world_cache_cell_lifetime: 0,
            world_cache_save: String::new(),
            world_cache_load: String::new(),
        }
    }
}

/// Adds or removes the prepass double-buffer components according to [`SolariLighting::restir`].
fn manage_prepass_double_buffers(
    views: Query<(
        Entity,
        &SolariLighting,
        Has<DeferredPrepassDoubleBuffer>,
        Has<DepthPrepassDoubleBuffer>,
    )>,
    mut commands: Commands,
) {
    for (entity, solari_lighting, deferred_double_buffered, depth_double_buffered) in &views {
        let mut entity = commands.entity(entity);
        if solari_lighting.restir {
            if !deferred_double_buffered {
                entity.insert(DeferredPrepassDoubleBuffer);
            }
            if !depth_double_buffered {
                entity.insert(DepthPrepassDoubleBuffer);
            }
        } else {
            if deferred_double_buffered {
                entity.remove::<DeferredPrepassDoubleBuffer>();
            }
            if depth_double_buffered {
                entity.remove::<DepthPrepassDoubleBuffer>();
            }
        }
    }
}

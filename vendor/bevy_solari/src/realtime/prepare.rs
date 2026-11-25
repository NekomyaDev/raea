use super::SolariLighting;
use crate::scene::RaytracingSceneNeedsPreviousFrameData;
#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy_anti_alias::dlss::{
    Dlss, DlssRayReconstructionFeature, ViewDlssRayReconstructionTextures,
};
use bevy_camera::MainPassResolutionOverride;
use bevy_diagnostic::FrameCount;
#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy_ecs::query::Has;
use bevy_ecs::{
    component::Component,
    entity::Entity,
    system::{Commands, Local, Query, Res},
};
#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy_image::ToExtents;
use bevy_math::UVec2;
use bevy_render::{
    camera::ExtractedCamera,
    render_resource::{Buffer, BufferDescriptor, BufferInitDescriptor, BufferUsages},
    renderer::{RenderDevice, RenderQueue},
};
#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy_render::{
    render_resource::{
        TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
    },
    texture::CachedTexture,
};
use bytemuck::{Pod, Zeroable};

/// Size of the `LightSample` shader struct in bytes.
const LIGHT_SAMPLE_STRUCT_SIZE: u64 = 8;

/// Size of the `ResolvedLightSamplePacked` shader struct in bytes.
const RESOLVED_LIGHT_SAMPLE_STRUCT_SIZE: u64 = 24;

/// Size of the `Reservoir` shader struct in bytes.
const RESERVOIR_STRUCT_SIZE: u64 = 48;

pub const LIGHT_TILE_BLOCKS: u64 = 128;
pub const LIGHT_TILE_SAMPLES_PER_BLOCK: u64 = 1024;

/// Amount of entries in the world cache (must be a power of 2, and >= 2^10)
pub const WORLD_CACHE_SIZE: u64 = 2u64.pow(20);
/// Sum of per-cell field sizes in `WorldCache`. Keep in sync with `realtime_bindings.wgsl`.
const WORLD_CACHE_ENTRY_SIZE: u64 = 84;
/// Size of the fixed `b` array (`array<u32, WORLD_CACHE_SIZE / 1024>`).
const WORLD_CACHE_B_SIZE: u64 = (WORLD_CACHE_SIZE / 1024) * size_of::<u32>() as u64;
/// Offset of `active_cells_count`.
pub const WORLD_CACHE_ACTIVE_CELLS_COUNT_OFFSET: u64 =
    WORLD_CACHE_SIZE * WORLD_CACHE_ENTRY_SIZE + WORLD_CACHE_B_SIZE;
/// Must stay under wgpu's default `max_storage_buffer_binding_size` (128 MiB or 2^27 bytes).
pub const WORLD_CACHE_BUFFER_SIZE: u64 =
    (WORLD_CACHE_ACTIVE_CELLS_COUNT_OFFSET + size_of::<u32>() as u64).next_multiple_of(16);

/// GPU representation of the user-configurable [`SolariLighting`] settings, plus
/// per-frame state.
///
/// Field order and types must match the `SolariLightingSettings` struct in
/// `realtime_bindings.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SolariLightingUniforms {
    confidence_weight_cap: f32,
    primary_di_samples: u32,
    secondary_di_samples: u32,
    max_bounces: u32,
    world_cache_max_temporal_samples: f32,
    world_cache_direct_light_sample_count: u32,
    world_cache_max_gi_ray_distance: f32,
    world_cache_cell_updates_soft_target: u32,
    world_cache_position_base_cell_size: f32,
    world_cache_position_lod_scale: f32,
    frame_rng: u32,
    reset: u32,
    // raea: SolariLighting::first_bounce_probability, padded to keep the uniform a multiple of 16 bytes
    first_bounce_probability: f32,
    // raea: SolariLighting::world_cache_steady (a bake's cells keep their whole history and draw new random numbers
    // each update in a frame)
    world_cache_steady: u32,
    // raea (11-25): ReSTIR's spatial reuse radius in pixels, Bevy's 30 unless RAEA_SOLARI_SPATIAL_RADIUS says; a 30 px
    // disc is the size of the patches MALL RAT's held frames show (raea light steady)
    spatial_reuse_radius: f32,
    // raea (11-25): SolariLighting::paths_per_pixel (RAEA_SOLARI_PATHS overrides it)
    paths_per_pixel: u32,
    // raea (11-25): the firefly filter (restir.wesl): a reservoir whose contribution is over this many times its 8x8
    // block's mean is cut back to that, 0 off (RAEA_SOLARI_FIREFLY)
    firefly_cap: f32,
}

impl SolariLightingUniforms {
    fn new(settings: &SolariLighting, frame_count: u32, force_reset: bool, still: bool) -> Self {
        Self {
            confidence_weight_cap: settings.confidence_weight_cap,
            primary_di_samples: settings.primary_di_samples,
            secondary_di_samples: settings.secondary_di_samples,
            max_bounces: settings.max_bounces,
            world_cache_max_temporal_samples: settings.world_cache_max_temporal_samples,
            world_cache_direct_light_sample_count: settings.world_cache_direct_light_sample_count,
            world_cache_max_gi_ray_distance: settings.world_cache_max_gi_ray_distance,
            world_cache_cell_updates_soft_target: settings.world_cache_cell_updates_soft_target,
            world_cache_position_base_cell_size: settings.world_cache_position_base_cell_size,
            world_cache_position_lod_scale: settings.world_cache_position_lod_scale,
            frame_rng: frame_count.wrapping_mul(5782582),
            reset: (settings.reset || force_reset) as u32,
            first_bounce_probability: settings.first_bounce_probability.clamp(0.05, 1.0),
            // bit 1: a steady cache with the camera held (world_cache_query.wesl world_cache_still)
            // bits 16-31: a cell's life in frames unqueried (world_cache_query.wesl cell_life), 0 for Solari's
            world_cache_steady: (settings.world_cache_steady || steady_override()) as u32 | ((((settings.world_cache_steady || steady_override()) && still) as u32) << 1) | (life_override().unwrap_or(settings.world_cache_cell_lifetime).min(65535) << 16),
            spatial_reuse_radius: spatial_reuse_radius(),
            firefly_cap: firefly_override().unwrap_or(settings.firefly_cap).max(0.0),
            paths_per_pixel: paths_override().unwrap_or(settings.paths_per_pixel).clamp(1, 16),
        }
    }
}

/// Declares to the raytracing scene whether any view needs last frame's TLAS and light ids.
pub fn setup_raytracing_scene_needs_previous_frame_data(
    views: Query<&SolariLighting>,
    needs_previous_frame_data: Option<Res<RaytracingSceneNeedsPreviousFrameData>>,
    mut commands: Commands,
) {
    let restir_used = views.iter().any(|solari_lighting| solari_lighting.restir);
    match (restir_used, needs_previous_frame_data.is_some()) {
        (true, false) => commands.insert_resource(RaytracingSceneNeedsPreviousFrameData),
        (false, true) => commands.remove_resource::<RaytracingSceneNeedsPreviousFrameData>(),
        _ => {}
    }
}

/// Internal rendering resources used for Solari lighting.
#[derive(Component)]
pub struct SolariLightingResources {
    pub constants: Buffer,
    pub light_tile_samples: Buffer,
    pub light_tile_resolved_samples: Buffer,
    pub reservoirs: Option<SolariReservoirBuffers>,
    pub world_cache: Buffer,
    pub world_cache_active_cells_dispatch: Buffer,
    pub view_size: UVec2,
    /// raea: the baked cache file this view's world cache holds (SolariLighting::world_cache_load), "" for none
    pub world_cache_loaded: std::sync::Arc<std::sync::Mutex<String>>,
}

/// raea: read a baked world cache (SolariLighting::world_cache_save) into a view's cache buffer. A file of another size
/// (another build's cache layout) is refused, said once, and the view keeps its live cache.
fn load_world_cache(render_queue: &RenderQueue, buffer: &Buffer, path: &str) -> bool {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() as u64 == WORLD_CACHE_BUFFER_SIZE => {
            render_queue.write_buffer(buffer, 0, &bytes);
            let at = WORLD_CACHE_ACTIVE_CELLS_COUNT_OFFSET as usize;
            let cells = u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
            eprintln!("raea: the world cache baked in {path} read ({cells} cells); held while it renders");
            true
        }
        Ok(bytes) => {
            eprintln!("raea: REFUSED the baked world cache {path}: {} bytes, this build's cache is {WORLD_CACHE_BUFFER_SIZE} (bake it again with this raea); the live cache is used", bytes.len());
            false
        }
        Err(e) => {
            eprintln!("raea: REFUSED the baked world cache {path}: {e}; the live cache is used");
            false
        }
    }
}

pub struct SolariReservoirBuffers {
    pub a: Buffer,
    pub b: Buffer,
}

pub fn prepare_solari_lighting_resources(
    #[cfg(any(not(feature = "dlss"), feature = "force_disable_dlss"))] query: Query<(
        Entity,
        &ExtractedCamera,
        &SolariLighting,
        Option<&SolariLightingResources>,
        Option<&MainPassResolutionOverride>,
    )>,
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))] query: Query<(
        Entity,
        &ExtractedCamera,
        &SolariLighting,
        Option<&SolariLightingResources>,
        Option<&MainPassResolutionOverride>,
        Has<Dlss<DlssRayReconstructionFeature>>,
    )>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    frame_count: Res<FrameCount>,
    // raea (11-25): each view's camera last frame, so the cache knows when it holds still (world_cache_still)
    views: Query<&bevy_render::view::ExtractedView>,
    mut last_camera: Local<std::collections::HashMap<Entity, bevy_math::Mat4>>,
    // raea: the guides for a denoiser other than DLSS-RR (guides.rs)
    guides: Option<Res<super::guides::SolariGuideBuffers>>,
    mut commands: Commands,
) {
    for query_item in &query {
        #[cfg(any(not(feature = "dlss"), feature = "force_disable_dlss"))]
        let (entity, camera, solari_lighting, solari_lighting_resources, resolution_override) =
            query_item;
        #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
        let (
            entity,
            camera,
            solari_lighting,
            solari_lighting_resources,
            resolution_override,
            has_dlss_rr,
        ) = query_item;

        let Some(mut view_size) = camera.physical_viewport_size else {
            continue;
        };
        if let Some(MainPassResolutionOverride(resolution_override)) = resolution_override {
            view_size = *resolution_override;
        }

        let reusable = solari_lighting_resources.filter(|r| {
            r.view_size == view_size && r.reservoirs.is_some() == solari_lighting.restir
        });
        let camera_now = views.get(entity).map(|v| v.world_from_view.to_matrix()).ok();
        let still = camera_now.is_some() && last_camera.get(&entity).copied() == camera_now;
        if let Some(m) = camera_now {
            last_camera.insert(entity, m);
        }
        let uniforms =
            SolariLightingUniforms::new(solari_lighting, frame_count.0, reusable.is_none(), still);

        if let Some(solari_lighting_resources) = reusable {
            // The constants uniform can change every frame, so always upload it.
            render_queue.write_buffer(
                &solari_lighting_resources.constants,
                0,
                bytemuck::bytes_of(&uniforms),
            );
            // raea: a baked cache named for the first time, or another one (a cut between two bakes)
            let mut loaded = solari_lighting_resources.world_cache_loaded.lock().unwrap();
            if !solari_lighting.world_cache_load.is_empty() && *loaded != solari_lighting.world_cache_load {
                load_world_cache(&render_queue, &solari_lighting_resources.world_cache, &solari_lighting.world_cache_load);
                *loaded = solari_lighting.world_cache_load.clone();
            }
            continue;
        }

        let constants = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("solari_lighting_constants"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

        let light_tile_samples = render_device.create_buffer(&BufferDescriptor {
            label: Some("solari_lighting_light_tile_samples"),
            size: LIGHT_TILE_BLOCKS * LIGHT_TILE_SAMPLES_PER_BLOCK * LIGHT_SAMPLE_STRUCT_SIZE,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let light_tile_resolved_samples = render_device.create_buffer(&BufferDescriptor {
            label: Some("solari_lighting_light_tile_resolved_samples"),
            size: LIGHT_TILE_BLOCKS
                * LIGHT_TILE_SAMPLES_PER_BLOCK
                * RESOLVED_LIGHT_SAMPLE_STRUCT_SIZE,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let reservoirs = solari_lighting.restir.then(|| {
            let reservoirs_buffer = |name| {
                render_device.create_buffer(&BufferDescriptor {
                    label: Some(name),
                    size: (view_size.x * view_size.y) as u64 * RESERVOIR_STRUCT_SIZE,
                    usage: BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            };
            SolariReservoirBuffers {
                a: reservoirs_buffer("solari_lighting_reservoirs_a"),
                b: reservoirs_buffer("solari_lighting_reservoirs_b"),
            }
        });

        let world_cache = render_device.create_buffer(&BufferDescriptor {
            label: Some("solari_lighting_world_cache"),
            size: WORLD_CACHE_BUFFER_SIZE,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // raea: a baked cache, read in place of the live one's empty start
        let mut world_cache_loaded = String::new();
        if !solari_lighting.world_cache_load.is_empty() {
            load_world_cache(&render_queue, &world_cache, &solari_lighting.world_cache_load);
            world_cache_loaded = solari_lighting.world_cache_load.clone();
        }

        let world_cache_active_cells_dispatch = render_device.create_buffer(&BufferDescriptor {
            label: Some("solari_lighting_world_cache_active_cells_dispatch"),
            size: size_of::<[u32; 3]>() as u64,
            usage: BufferUsages::INDIRECT | BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        commands.entity(entity).insert(SolariLightingResources {
            constants,
            light_tile_samples,
            light_tile_resolved_samples,
            reservoirs,
            world_cache,
            world_cache_active_cells_dispatch,
            view_size,
            world_cache_loaded: std::sync::Arc::new(std::sync::Mutex::new(world_cache_loaded)),
        });
        if guides.is_some() {
            commands
                .entity(entity)
                .insert(super::guides::make(&render_device, view_size));
        }

        #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
        if has_dlss_rr {
            let diffuse_albedo = render_device.create_texture(&TextureDescriptor {
                label: Some("solari_lighting_diffuse_albedo"),
                size: view_size.to_extents(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            });
            let diffuse_albedo_view = diffuse_albedo.create_view(&TextureViewDescriptor::default());

            let specular_albedo = render_device.create_texture(&TextureDescriptor {
                label: Some("solari_lighting_specular_albedo"),
                size: view_size.to_extents(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            });
            let specular_albedo_view =
                specular_albedo.create_view(&TextureViewDescriptor::default());

            let normal_roughness = render_device.create_texture(&TextureDescriptor {
                label: Some("solari_lighting_normal_roughness"),
                size: view_size.to_extents(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba16Float,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            });
            let normal_roughness_view =
                normal_roughness.create_view(&TextureViewDescriptor::default());

            let dlss_rr_depth = render_device.create_texture(&TextureDescriptor {
                label: Some("solari_lighting_dlss_rr_depth"),
                size: view_size.to_extents(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R32Float,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            });
            let dlss_rr_depth_view = dlss_rr_depth.create_view(&TextureViewDescriptor::default());

            let specular_motion_vectors = render_device.create_texture(&TextureDescriptor {
                label: Some("solari_lighting_specular_motion_vectors"),
                size: view_size.to_extents(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rg16Float,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            });
            let specular_motion_vectors_view =
                specular_motion_vectors.create_view(&TextureViewDescriptor::default());

            commands
                .entity(entity)
                .insert(ViewDlssRayReconstructionTextures {
                    diffuse_albedo: CachedTexture {
                        texture: diffuse_albedo,
                        default_view: diffuse_albedo_view,
                    },
                    specular_albedo: CachedTexture {
                        texture: specular_albedo,
                        default_view: specular_albedo_view,
                    },
                    normal_roughness: CachedTexture {
                        texture: normal_roughness,
                        default_view: normal_roughness_view,
                    },
                    depth: CachedTexture {
                        texture: dlss_rr_depth,
                        default_view: dlss_rr_depth_view,
                    },
                    specular_motion_vectors: CachedTexture {
                        texture: specular_motion_vectors,
                        default_view: specular_motion_vectors_view,
                    },
                });
        }
    }
}

/// raea: ReSTIR's spatial reuse radius in pixels (restir.wesl), 30 as Bevy has it unless RAEA_SOLARI_SPATIAL_RADIUS says
fn spatial_reuse_radius() -> f32 {
    static R: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *R.get_or_init(|| std::env::var("RAEA_SOLARI_SPATIAL_RADIUS").ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| *v >= 1.0).unwrap_or(30.0))
}

/// raea: RAEA_SOLARI_FIREFLY, the firefly filter's multiplier for every camera whatever the composition asks (0 off)
fn firefly_override() -> Option<f32> {
    static F: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var("RAEA_SOLARI_FIREFLY").ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| *v >= 0.0))
}

/// raea: RAEA_SOLARI_PATHS, paths a pixel for every camera whatever the composition asks (a sweep)
fn paths_override() -> Option<u32> {
    static P: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *P.get_or_init(|| std::env::var("RAEA_SOLARI_PATHS").ok().and_then(|v| v.parse::<u32>().ok()).filter(|v| *v >= 1))
}

/// raea: RAEA_SOLARI_STEADY=1, the steady cache for every camera whatever the composition asks (a check on a scene that
/// doesn't set cache.steady)
fn steady_override() -> bool {
    static S: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *S.get_or_init(|| std::env::var("RAEA_SOLARI_STEADY").as_deref() == Ok("1"))
}

/// raea: RAEA_SOLARI_CELL_LIFE, a world cache cell's life in frames for every camera whatever the composition asks
fn life_override() -> Option<u32> {
    static L: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *L.get_or_init(|| std::env::var("RAEA_SOLARI_CELL_LIFE").ok().and_then(|v| v.parse::<u32>().ok()))
}

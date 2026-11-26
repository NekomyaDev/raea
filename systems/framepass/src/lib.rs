//! A shader over the frame, written in TypeScript (docs/shaders.feature SH6): <FramePass wesl|wgsl uniforms textures
//! at prepass> in a <Canvas3D>, more than one in order. Written the way Bevy 0.20's FullscreenMaterial is (its
//! FullscreenShader vertex state, a ViewTarget post_process_write, ordered in Core3d by Core3dSystems), but one generic
//! pass whose pipeline is specialized per shader, as ShaderMaterial's is: the source's module is hashed into a uuid.
//!
//! The source defines `fn shade(in: FullscreenVertexOutput) -> vec4<f32>` (or its own @fragment), and reads
//!   frame(uv)      the frame as drawn so far
//!   u_NAME()       a uniform, tex_NAME / samp_NAME a texture (as ShaderMesh has them; render:NAME included)
//!   depth(uv)      with `prepass`: the distance from the camera in metres (a perspective camera's; nothing drawn is
//!                  1e30)
//!   normal(uv)     with `prepass`: the surface's world-space normal
//! `at`: "before-tonemap" (the HDR frame, after bloom) or "after-tonemap" (the display colours; the default).
//!
//! `in.uv` spans the camera's Viewport, not its target: the Engine's target is w x (h + 1), a tag row under the frame,
//! and Bevy's frame textures are the target's size, so a pass drawn over the whole texture saw uv 0..1 over h + 1 rows
//! (a pattern by uv drifting a row by the frame's foot, R3F25). The pass draws in the Viewport, and frame(), depth()
//! and normal() map its uv back onto the texture.

use std::collections::HashMap;

use bevy::core_pipeline::{prepass::ViewPrepassTextures, schedule::Core3d, tonemapping::tonemapping, Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::{
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    render_asset::RenderAssets,
    render_resource::{
        binding_types::{sampler, texture_2d, texture_2d_array, texture_depth_2d, uniform_buffer_sized},
        *,
    },
    renderer::{RenderContext, RenderDevice, ViewQuery},
    texture::{FallbackImage, GpuImage},
    camera::ExtractedCamera,
    view::{ExtractedView, ViewTarget},
    Render, RenderApp, RenderStartup, RenderSystems,
};
use raea_sys_material::shader_material::{uniform, SLOTS, TEXTURES};
use raea_sys_shader::{accessors, defines, register, Inputs};
use serde_json::Value;

pub mod written;

/// Where in the frame a pass runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum At {
    BeforeTonemap,
    AfterTonemap,
}

/// One pass, built.
#[derive(Clone, Debug)]
pub struct Pass {
    pub shader: u128,
    pub at: At,
    pub prepass: bool,
    pub slots: [Vec4; SLOTS],
    pub textures: Vec<Handle<Image>>,
    /// the one texture array (docs/shaders.feature SH9)
    pub array: Option<Handle<Image>>,
    /// a pass as written (a <FramePass file>): its own bindings (written.rs)
    pub written: Option<written::Written>,
    /// an inline pass's uniforms, followed frame by frame (`follow`)
    pub inline: Option<Inline>,
}

/// An inline pass's row and its uniforms' names and types in slot order: `follow` puts each frame's values in its
/// slots, as a pass as written's are (until 11-11 an inline pass kept its first frame's uniforms for a whole render:
/// a camera's matrices or a time handed in stood still).
#[derive(Clone, Debug, Default)]
pub struct Inline {
    pub id: String,
    pub names: Vec<(String, &'static str)>,
    last: String,
}

impl Inline {
    pub fn new(names: Vec<(String, &'static str)>) -> Inline {
        Inline { id: String::new(), names, last: String::new() }
    }

    /// This frame's uniforms (JSON, {NAME: value}) into `slots` when they've changed; whether they had. A uniform the
    /// frame leaves out keeps its value; one whose type changes is refused, the module being built for the first
    /// frame's.
    pub fn follow(&mut self, uniforms: &str, slots: &mut [Vec4; SLOTS]) -> Result<bool, String> {
        if uniforms == self.last {
            return Ok(false);
        }
        let v: Value = serde_json::from_str(uniforms).map_err(|e| format!("its uniforms: {e}"))?;
        let o = v.as_object().ok_or_else(|| format!("its uniforms are {{name: value}}, got {v}"))?;
        for (i, (name, ty)) in self.names.iter().enumerate() {
            let Some(x) = o.get(name) else { continue };
            let u = uniform(name, x)?;
            if u.ty != *ty {
                return Err(format!("uniform {name} is a {ty} (its first frame's), got a {}", u.ty));
            }
            slots[i] = u.value;
        }
        self.last = uniforms.to_string();
        Ok(true)
    }
}

/// The camera's passes, in order.
#[derive(Component, Clone, Debug, Default, ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
pub struct FramePasses(pub Vec<Pass>);

/// The module a pass's source is compiled as, in WESL: its bindings and accessors, the source, and the entry point
/// unless it has its own. Bindings: 0 the frame, 1 its sampler, 2 the uniforms (the camera's near plane, the
/// Viewport's place in the texture, then the slots), 3 + 2i a texture and 4 + 2i its sampler, 11 the texture array and
/// 12 its sampler, 13 the depth and 14 the normals with a prepass.
pub fn module(source: &str, uniforms: &[(String, &'static str)], textures: &[String], array: Option<&str>, prepass: bool) -> String {
    let mut m = String::from(
        "import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;\n\n\
         @group(0) @binding(0) var raea_frame: texture_2d<f32>;\n\
         @group(0) @binding(1) var raea_frame_sampler: sampler;\n\
         struct RaeaPass { near: vec4<f32>, view: vec4<f32>, s: array<vec4<f32>, 16> };\n\
         @group(0) @binding(2) var<uniform> raea_pass: RaeaPass;\n\
         // a uv over the camera's Viewport as one over the frame's texture (view: its size, then its corner, over the texture's)\n\
         fn raea_view_uv(uv: vec2<f32>) -> vec2<f32> { return raea_pass.view.zw + uv * raea_pass.view.xy; }\n\
         // read past its edge, the frame's edge pixel (as a clamped texture is), never the row under the Viewport\n\
         fn frame(uv: vec2<f32>) -> vec4<f32> {\n\
         \x20   let h = 0.5 / (vec2<f32>(textureDimensions(raea_frame)) * raea_pass.view.xy);\n\
         \x20   return textureSampleLevel(raea_frame, raea_frame_sampler, raea_view_uv(clamp(uv, h, 1.0 - h)), 0.0);\n\
         }\n",
    );
    for (i, t) in textures.iter().enumerate() {
        m += &format!("@group(0) @binding({}) var tex_{t}: texture_2d<f32>;\n@group(0) @binding({}) var samp_{t}: sampler;\n", 3 + i * 2, 4 + i * 2);
    }
    if let Some(a) = array {
        m += &format!("@group(0) @binding(11) var tex_{a}: texture_2d_array<f32>;\n@group(0) @binding(12) var samp_{a}: sampler;\n");
    }
    if prepass {
        m += "@group(0) @binding(13) var raea_depth: texture_depth_2d;\n\
              @group(0) @binding(14) var raea_normal: texture_2d<f32>;\n\
              // the main pass may have drawn the view smaller, in its Viewport's corner (a render scale), and wider (overscan):\n\
              // near.yz is the part of the Viewport the frame's own view takes, near.w how much wider a side it drew\n\
              fn raea_texel(uv: vec2<f32>, size: vec2<u32>) -> vec2<i32> { return clamp(vec2<i32>((raea_pass.view.zw + (clamp(uv, vec2<f32>(0.0), vec2<f32>(0.99999)) + raea_pass.near.w) * raea_pass.near.yz * raea_pass.view.xy) * vec2<f32>(size)), vec2<i32>(0), vec2<i32>(size) - vec2<i32>(1)); }\n\
              // reverse-Z: the depth is near / distance, 0 where nothing was drawn\n\
              fn depth(uv: vec2<f32>) -> f32 {\n\
              \x20   let d = textureLoad(raea_depth, raea_texel(uv, textureDimensions(raea_depth)), 0);\n\
              \x20   if d <= 0.0 { return 1e30; }\n\
              \x20   return raea_pass.near.x / d;\n\
              }\n\
              fn normal(uv: vec2<f32>) -> vec3<f32> { return normalize(textureLoad(raea_normal, raea_texel(uv, textureDimensions(raea_normal)), 0).xyz * 2.0 - 1.0); }\n";
    }
    m += &accessors(uniforms, "raea_pass");
    m += "\n";
    m += source;
    if !source.contains("@fragment") {
        m += "\n\n@fragment\nfn raea_frame_pass(in: FullscreenVertexOutput) -> @location(0) vec4<f32> { return shade(in); }\n";
    }
    m
}

/// A pass from a <FramePass>'s spec, {wesl | wgsl, uniforms, textures, at, prepass}, its module added to
/// Assets<Shader> under the uuid of its hash. `image` resolves a texture as raea-sys-sampler's `texture` does.
pub fn build(world: &mut World, v: &Value, mut image: impl FnMut(&mut World, &Value) -> Result<Handle<Image>, String>) -> Result<Pass, String> {
    let o = v.as_object().ok_or_else(|| format!("a frame pass is {{wesl, uniforms, textures, at, prepass}}, got {v}"))?;
    let at = match o.get("at").and_then(Value::as_str) {
        None | Some("after-tonemap") => At::AfterTonemap,
        Some("before-tonemap") => At::BeforeTonemap,
        Some(a) => return Err(format!("a frame pass's `at` is \"before-tonemap\" or \"after-tonemap\", got {a:?}")),
    };
    if o.contains_key("file") {
        let (shader, w) = written::build(world, o, &mut image)?;
        return Ok(Pass { shader, at, prepass: false, slots: [Vec4::ZERO; SLOTS], textures: vec![], array: None, written: Some(w), inline: None });
    }
    let ins = Inputs::read(world, o, "a frame pass", &mut image)?;
    if !(ins.src.contains("@fragment") || defines(&ins.src, "shade")) {
        return Err("a frame pass defines fn shade(in: FullscreenVertexOutput) -> vec4<f32>, or its own @fragment".into());
    }
    let prepass = match o.get("prepass") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(Value::Object(p)) => p.values().any(|x| x.as_bool() == Some(true)),
        Some(p) => return Err(format!("a frame pass's prepass is {{depth, normal}} or true, got {p}")),
    };
    let (names, tnames) = ins.names();
    let shader = register(world, module(&ins.src, &names, &tnames, ins.array.as_ref().map(|a| a.0.as_str()), prepass), &ins.defs, "raea frame pass", "raea_sys_framepass");
    let inline = Some(Inline::new(names));
    Ok(Pass { shader, at, prepass, slots: ins.slots(), textures: ins.textures.into_iter().map(|t| t.1).collect(), array: ins.array.map(|a| a.1), written: None, inline })
}

/// The passes onto a camera: with a prepass, its depth and normal prepasses on and its multisampling off (a
/// multisampled prepass texture is another binding type).
pub fn put(world: &mut World, cam: Entity, passes: Vec<Pass>) {
    let mut e = world.entity_mut(cam);
    if passes.iter().any(|p| p.prepass) {
        // PrepassRead: msaa_for_3d turned a video's MSAA back on under the pass, refused at its third frame (as #159)
        e.insert((bevy::core_pipeline::prepass::DepthPrepass, bevy::core_pipeline::prepass::NormalPrepass, Msaa::Off, raea_sys_compose::passes::PrepassRead));
    }
    e.insert(FramePasses(passes));
}

pub fn install(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<FramePasses>::default());
    app.add_systems(PostUpdate, follow);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
    render_app.add_systems(RenderStartup, init_pipelines);
    // queued before wait_pipelines (RenderSystems::Render), so a pass's first frame has its pipeline
    render_app.add_systems(Render, queue.in_set(RenderSystems::Queue));
    render_app.add_systems(Core3d, (draw::<true>.before(tonemapping), draw::<false>.after(tonemapping)).in_set(Core3dSystems::PostProcess));
}

/// Each pass's uniforms from its row's keyed `uniforms` this frame, as written or inline (a bad one is logged); the
/// camera's passes marked changed only when one has, so they're extracted again.
fn follow(frame: Option<Res<raea_sys_frame::FrameRows>>, mut cams: Query<&mut FramePasses>) {
    let Some(frame) = frame else { return };
    let cols = &frame.cols;
    for mut passes in &mut cams {
        let mut changed = false;
        for pass in passes.bypass_change_detection().0.iter_mut() {
            let Pass { written, inline, slots, .. } = pass;
            if let Some(w) = written.as_mut() {
                let Some(r) = cols.id.iter().position(|i| *i == w.id) else { continue };
                match w.follow(&raea_sys_frame::keyed(|c| frame.at(r, c), "uniforms").unwrap_or_default()) {
                    Ok(c) => changed |= c,
                    Err(e) => error!("FramePass {}: {e}", w.id),
                }
            } else if let Some(i) = inline.as_mut() {
                let Some(r) = cols.id.iter().position(|x| *x == i.id) else { continue };
                let Some(u) = raea_sys_frame::keyed(|c| frame.at(r, c), "uniforms") else { continue };
                match i.follow(&u, slots) {
                    Ok(c) => changed |= c,
                    Err(e) => error!("FramePass {}: {e}", i.id),
                }
            }
        }
        if changed {
            passes.set_changed();
        }
    }
}

#[derive(Resource)]
struct Pipelines {
    plain: BindGroupLayoutDescriptor,
    deep: BindGroupLayoutDescriptor,
    sampler: Sampler,
    ids: HashMap<(u128, TextureFormat, bool), CachedRenderPipelineId>,
    /// a pass as written's pipeline and layout by its shader and format, and its samplers by its shader and their names
    written: HashMap<(u128, TextureFormat), (BindGroupLayoutDescriptor, CachedRenderPipelineId)>,
    samplers: HashMap<(u128, String), Sampler>,
}

const UNIFORM_BYTES: u64 = (16 * (2 + SLOTS)) as u64;

fn init_pipelines(mut commands: Commands, device: Res<RenderDevice>) {
    let tex = || texture_2d(TextureSampleType::Float { filterable: true });
    let samp = || sampler(SamplerBindingType::Filtering);
    let arr = || texture_2d_array(TextureSampleType::Float { filterable: true });
    let size = std::num::NonZeroU64::new(UNIFORM_BYTES);
    let plain = BindGroupLayoutDescriptor::new(
        "raea_frame_pass",
        &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (tex(), samp(), uniform_buffer_sized(false, size), tex(), samp(), tex(), samp(), tex(), samp(), tex(), samp(), arr(), samp())),
    );
    let deep = BindGroupLayoutDescriptor::new(
        "raea_frame_pass_prepass",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (tex(), samp(), uniform_buffer_sized(false, size), tex(), samp(), tex(), samp(), tex(), samp(), tex(), samp(), arr(), samp(), texture_depth_2d(), texture_2d(TextureSampleType::Float { filterable: false })),
        ),
    );
    let sampler = device.create_sampler(&SamplerDescriptor { mag_filter: FilterMode::Linear, min_filter: FilterMode::Linear, ..default() });
    commands.insert_resource(Pipelines { plain, deep, sampler, ids: HashMap::new(), written: HashMap::new(), samplers: HashMap::new() });
}

fn queue(views: Query<(&ViewTarget, &FramePasses)>, pipelines: Option<ResMut<Pipelines>>, fullscreen: Res<FullscreenShader>, cache: Res<PipelineCache>, device: Res<RenderDevice>) {
    let Some(mut p) = pipelines else { return };
    for (target, passes) in &views {
        let format = target.main_texture_format();
        for pass in &passes.0 {
            if let Some(w) = &pass.written {
                if !p.written.contains_key(&(pass.shader, format)) {
                    let layout = BindGroupLayoutDescriptor::new("raea_frame_pass_written", &written::layout(&w.decls));
                    let shader = Handle::Uuid(bevy::asset::uuid::Uuid::from_u128(pass.shader), core::marker::PhantomData);
                    let fragment = FragmentState { shader, entry_point: Some(w.entry.clone().into()), targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })], ..default() };
                    let id = cache.queue_render_pipeline(RenderPipelineDescriptor { label: Some("raea_frame_pass_written".into()), layout: vec![layout.clone()], vertex: fullscreen.to_vertex_state(), fragment: Some(fragment), ..default() });
                    p.written.insert((pass.shader, format), (layout, id));
                }
                for (name, d) in &w.samplers {
                    p.samplers.entry((pass.shader, name.clone())).or_insert_with(|| device.create_sampler(&d.as_wgpu()));
                }
                continue;
            }
            let key = (pass.shader, format, pass.prepass);
            if p.ids.contains_key(&key) {
                continue;
            }
            let layout = if pass.prepass { p.deep.clone() } else { p.plain.clone() };
            let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("raea_frame_pass".into()),
                layout: vec![layout],
                vertex: fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: Handle::Uuid(bevy::asset::uuid::Uuid::from_u128(pass.shader), core::marker::PhantomData),
                    targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                    ..default()
                }),
                ..default()
            });
            p.ids.insert(key, id);
        }
    }
}

type Views<'w, 's> = ViewQuery<'w, 's, (&'static ViewTarget, &'static FramePasses, &'static ExtractedView, Option<&'static ViewPrepassTextures>, Option<&'static ExtractedCamera>, Option<&'static bevy::camera::MainPassResolutionOverride>, Option<&'static raea_sys_viewbox::Overscan>)>;

/// The camera's Viewport in its target (x, y, w, h in px), and its size and corner over the target's (the uniforms'
/// `view`); none without a Viewport, the pass then spanning the target.
fn viewport(camera: Option<&ExtractedCamera>) -> Option<([f32; 4], Vec4)> {
    let c = camera?;
    let v = c.viewport.as_ref()?;
    rect(v.physical_position, v.physical_size, c.physical_target_size?)
}

/// A Viewport at `at`, `size` big, in a `target`-sized texture, as `viewport` gives it.
fn rect(at: UVec2, size: UVec2, target: UVec2) -> Option<([f32; 4], Vec4)> {
    let (at, size, t) = (at.as_vec2(), size.as_vec2(), target.as_vec2());
    if t.x <= 0.0 || t.y <= 0.0 || size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    Some(([at.x, at.y, size.x, size.y], Vec4::new(size.x / t.x, size.y / t.y, at.x / t.x, at.y / t.y)))
}

/// Where the frame's own view lies in the depth and the normals: the share of the camera's Viewport it takes a side
/// (x, y), and how much wider a side the main pass drew (z). The main pass draws a `part` of the Viewport, from its
/// corner, when the view has a render scale, and `k` wider a side with overscan; the frame is the middle of that.
fn deep(part: Option<UVec2>, port: Option<UVec2>, k: f32) -> Vec3 {
    let share = part.zip(port).filter(|(_, v)| v.x > 0 && v.y > 0).map_or(Vec2::ONE, |(p, v)| p.as_vec2() / v.as_vec2());
    (share / (1.0 + 2.0 * k)).extend(k)
}

/// The passes placed before tone mapping (BEFORE) or after it.
fn draw<const BEFORE: bool>(view: Views<'_, '_>, p: Option<Res<Pipelines>>, cache: Res<PipelineCache>, images: Res<RenderAssets<GpuImage>>, fallback: Res<FallbackImage>, mut ctx: RenderContext) {
    let Some(p) = p else { return };
    let at = if BEFORE { At::BeforeTonemap } else { At::AfterTonemap };
    let (target, passes, extracted, prepass, camera, part, over) = view.into_inner();
    // a perspective camera's near plane, from its infinite reverse-Z projection
    let near = extracted.clip_from_view.w_axis.z;
    let port = viewport(camera);
    let view_uv = port.map_or(Vec4::new(1.0, 1.0, 0.0, 0.0), |p| p.1);
    let deep = deep(part.map(|p| p.0), camera.and_then(|c| c.physical_viewport_size), raea_sys_viewbox::Overscan::drawn(over));
    for pass in passes.0.iter().filter(|x| x.at == at) {
        if let Some(w) = &pass.written {
            let Some((layout, id)) = p.written.get(&(pass.shader, target.main_texture_format())) else { continue };
            let Some(rp) = cache.get_render_pipeline(*id) else { continue };
            let post = target.post_process_write();
            let sampler = |n: &str| p.samplers.get(&(pass.shader, n.to_string())).cloned();
            // over the whole texture, as it reads its frame binding by its own uv: it has no frame() to map
            if let Some(group) = written::group(ctx.render_device(), &cache.get_bind_group_layout(layout), w, post.source, &images, sampler) {
                blit(&mut ctx, post.destination, rp, &group, None);
            }
            continue;
        }
        let Some(rp) = p.ids.get(&(pass.shader, target.main_texture_format(), pass.prepass)).and_then(|id| cache.get_render_pipeline(*id)) else { continue };
        let (depth, normal) = match prepass {
            Some(t) => (t.depth_only_view(), t.normal_view()),
            None => (None, None),
        };
        if pass.prepass && (depth.is_none() || normal.is_none()) {
            continue;
        }
        let mut bytes: Vec<u8> = Vec::with_capacity(UNIFORM_BYTES as usize);
        for v in [Vec4::new(near, deep.x, deep.y, deep.z), view_uv].into_iter().chain(pass.slots.iter().copied()) {
            for f in v.to_array() {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
        }
        let buffer = ctx.render_device().create_buffer_with_data(&BufferInitDescriptor { label: Some("raea_frame_pass"), contents: &bytes, usage: BufferUsages::UNIFORM });
        let tex: Vec<&GpuImage> = (0..TEXTURES).map(|i| pass.textures.get(i).and_then(|h| images.get(h.id())).unwrap_or(&fallback.d2)).collect();
        let arr = match &pass.array {
            Some(h) => match images.get(h.id()) {
                Some(a) => a,
                None => continue,
            },
            None => &fallback.d2_array,
        };
        let post = target.post_process_write();
        let group = if pass.prepass {
            ctx.render_device().create_bind_group(
                "raea_frame_pass",
                &cache.get_bind_group_layout(&p.deep),
                &BindGroupEntries::sequential((
                    post.source,
                    &p.sampler,
                    buffer.as_entire_binding(),
                    &tex[0].texture_view,
                    &tex[0].sampler,
                    &tex[1].texture_view,
                    &tex[1].sampler,
                    &tex[2].texture_view,
                    &tex[2].sampler,
                    &tex[3].texture_view,
                    &tex[3].sampler,
                    &arr.texture_view,
                    &arr.sampler,
                    depth.unwrap(),
                    normal.unwrap(),
                )),
            )
        } else {
            ctx.render_device().create_bind_group(
                "raea_frame_pass",
                &cache.get_bind_group_layout(&p.plain),
                &BindGroupEntries::sequential((
                    post.source,
                    &p.sampler,
                    buffer.as_entire_binding(),
                    &tex[0].texture_view,
                    &tex[0].sampler,
                    &tex[1].texture_view,
                    &tex[1].sampler,
                    &tex[2].texture_view,
                    &tex[2].sampler,
                    &tex[3].texture_view,
                    &tex[3].sampler,
                    &arr.texture_view,
                    &arr.sampler,
                )),
            )
        };
        blit(&mut ctx, post.destination, rp, &group, port.map(|p| p.0));
    }
}

/// The pass's full-screen triangle drawn into `dest`, in `port` (x, y, w, h) when the camera has a Viewport. What's
/// outside it (the tag row) no pass after reads: Bevy's upscaling scissors to the Viewport.
fn blit(ctx: &mut RenderContext, dest: &TextureView, rp: &RenderPipeline, group: &BindGroup, port: Option<[f32; 4]>) {
    let mut rpass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("raea_frame_pass"),
        color_attachments: &[Some(RenderPassColorAttachment { view: dest, depth_slice: None, resolve_target: None, ops: Operations::default() })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if let Some([x, y, w, h]) = port {
        rpass.set_viewport(x, y, w, h, 0.0, 1.0);
    }
    rpass.set_pipeline(rp);
    rpass.set_bind_group(0, group, &[]);
    rpass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_has_what_the_pass_asks_for() {
        let m = module("fn shade(in: FullscreenVertexOutput) -> vec4<f32> { return frame(in.uv); }", &[("k".into(), "f32")], &["pal".into()], Some("frames"), false);
        assert!(m.contains("@binding(11) var tex_frames: texture_2d_array<f32>;"));
        assert!(m.contains("fn u_k() -> f32 { return raea_pass.s[0].x; }"));
        assert!(m.contains("@binding(3) var tex_pal") && m.contains("@binding(4) var samp_pal"));
        assert!(!m.contains("fn depth(") && m.contains("return shade(in);"));
        let d = module("@fragment fn f(in: FullscreenVertexOutput) -> @location(0) vec4<f32> { return vec4(depth(in.uv)); }", &[], &[], None, true);
        assert!(d.contains("fn depth(") && d.contains("fn normal(") && !d.contains("raea_frame_pass("));
        // the frame, the depth and the normals are read at the Viewport's uv mapped onto their textures
        assert!(m.contains("textureSampleLevel(raea_frame, raea_frame_sampler, raea_view_uv(clamp(uv, h, 1.0 - h)), 0.0)"));
        assert!(d.contains("(raea_pass.view.zw + (clamp(uv, vec2<f32>(0.0), vec2<f32>(0.99999)) + raea_pass.near.w) * raea_pass.near.yz * raea_pass.view.xy) * vec2<f32>(size)"));
    }

    #[test]
    fn the_depth_is_read_where_the_main_pass_drew_the_frame() {
        assert_eq!(deep(None, Some(UVec2::new(640, 360)), 0.0), Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(deep(Some(UVec2::new(320, 180)), Some(UVec2::new(640, 360)), 0.0), Vec3::new(0.5, 0.5, 0.0));
        // 5% overscan drawn at 1 / 1.1 of the size, filling the Viewport: the frame is its middle, uv 0 at 0.05 / 1.1
        let d = deep(Some(UVec2::new(640, 360)), Some(UVec2::new(640, 360)), 0.05);
        assert!((d.x - 1.0 / 1.1).abs() < 1e-6 && ((0.0 + d.z) * d.x - 0.05 / 1.1).abs() < 1e-6 && ((1.0 + d.z) * d.x - 1.05 / 1.1).abs() < 1e-6);
    }

    #[test]
    fn a_pass_spans_the_cameras_viewport_not_the_tag_row_under_it() {
        // the Engine's 640 x 361 target, its camera's 640 x 360 Viewport at the top: uv 1 is the frame's last row
        let (port, view) = rect(UVec2::ZERO, UVec2::new(640, 360), UVec2::new(640, 361)).unwrap();
        assert_eq!(port, [0.0, 0.0, 640.0, 360.0]);
        assert_eq!(view, Vec4::new(1.0, 360.0 / 361.0, 0.0, 0.0));
        // a Viewport placed in its target: its corner over the target's
        let (_, v) = rect(UVec2::new(100, 50), UVec2::new(200, 100), UVec2::new(400, 200)).unwrap();
        assert_eq!(v, Vec4::new(0.5, 0.5, 0.25, 0.25));
        assert!(rect(UVec2::ZERO, UVec2::ZERO, UVec2::new(640, 361)).is_none());
    }

    #[test]
    fn an_inline_pass_follows_its_uniforms_frame_by_frame() {
        // a render's frames each hand the pass their own values (a camera, a time): they were the first frame's
        let mut i = Inline::new(vec![("k".into(), "f32"), ("eye".into(), "vec3<f32>")]);
        let mut slots = [Vec4::ZERO; SLOTS];
        assert_eq!(i.follow(r#"{"k": 0.5, "eye": [1, 2, 3]}"#, &mut slots), Ok(true));
        assert_eq!((slots[0].x, slots[1]), (0.5, Vec4::new(1.0, 2.0, 3.0, 0.0)));
        // the same values again change nothing; one left out keeps its value
        assert_eq!(i.follow(r#"{"k": 0.5, "eye": [1, 2, 3]}"#, &mut slots), Ok(false));
        assert_eq!(i.follow(r#"{"k": 0.75}"#, &mut slots), Ok(true));
        assert_eq!((slots[0].x, slots[1].z), (0.75, 3.0));
        // its type is its first frame's
        assert!(i.follow(r#"{"k": [1, 2]}"#, &mut slots).is_err());
    }
}

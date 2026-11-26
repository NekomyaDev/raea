//! The view a camera on the Engine's target draws: the frame above its tag row, or the box a page lays out for it
//! (Engine::set_view_box). raea-render's Engine gives the target and the box.

use bevy::camera::RenderTarget;
use bevy::prelude::*;

/// The Engine's target: the frame, and its tag row under it.
#[derive(Resource)]
pub struct TagRowTarget(pub Handle<Image>);

/// Engine::set_view_box's box, in target px.
#[derive(Resource, Default)]
pub struct ViewBox(pub Option<[u32; 4]>);

pub use raea_sys_compose::passes::Overscan;

/// A sub view drawn `k` wider on every side, about its target's middle.
pub fn widened(v: bevy::camera::SubCameraView, k: f32) -> bevy::camera::SubCameraView {
    if k <= 0.0 {
        return v;
    }
    // only the ratios count: in hundredths of a pixel, so the wider size isn't rounded to whole ones
    let (full, size) = (v.full_size * 100, v.size.as_vec2() * 100.0);
    bevy::camera::SubCameraView { full_size: full, offset: v.offset * 100.0 - size * k, size: (size * (1.0 + 2.0 * k)).round().as_uvec2() }
}

/// A camera drawing to the Engine's target draws a w x h Viewport of it, the frame's own, above the tag row: a view of
/// w x (h + 1) drew the 3D 0.14% larger and half a pixel low against a camera on a w x h target, a plain App's
/// (docs/blitz-bevy.feature B3 H10), and against the looks' cells. With a view box, its own view is the box's (the
/// target a sub view of it, offset by the box's place). The Viewport this gave a camera (`ours`) is taken off when a
/// box comes: a camera given it on the frames before the page had laid the box out (a render's first draws) kept it,
/// and drew the whole page's view, cropped to the box (#130).
pub fn tag_row_view(t: Res<TagRowTarget>, images: Res<Assets<Image>>, view_box: Option<Res<ViewBox>>, mut ours: Local<bevy::platform::collections::HashSet<Entity>>, mut q: Query<(Entity, &mut Camera, &RenderTarget, Option<&mut Overscan>), Without<raea_sys_frame::TagCamera>>) {
    let Some(img) = images.get(&t.0) else { return };
    let s = img.size();
    let view = match view_box.and_then(|v| v.0) {
        Some([x, y, w, h]) => bevy::camera::SubCameraView { full_size: UVec2::new(w, h), offset: Vec2::new(-(x as f32), -(y as f32)), size: s },
        None => bevy::camera::SubCameraView { full_size: UVec2::new(s.x, s.y - 1), offset: Vec2::ZERO, size: s },
    };
    for (e, mut c, rt, mut over) in &mut q {
        let k = Overscan::of(over.as_deref());
        // drawn wider only below, where this gives the camera its view
        let mut drawn = 0.0;
        // a camera with its own Viewport (a ViewportNode's) sees its viewport's shape, as Bevy sizes it, never the tag row
        let own = c.viewport.is_some() && !ours.contains(&e);
        if own || !matches!(rt, RenderTarget::Image(i) if i.handle == t.0) {
            if own && c.sub_camera_view.is_some() { c.sub_camera_view = None; }
            if let Some(o) = over.as_mut() && o.drawn != 0.0 { o.drawn = 0.0; }
            continue;
        }
        if view.offset == Vec2::ZERO {
            // a w x h Viewport: Bevy's orthographic sub view takes its area from the w x (h + 1) target, 0.14% small
            // (2d/sprite_slice), and a draw in clip space (a PhaseItem, custom_phase_item) maps to its h + 1 rows
            if c.viewport.as_ref().is_none_or(|v| v.physical_position != UVec2::ZERO || v.physical_size != view.full_size) { c.viewport = Some(bevy::camera::Viewport { physical_size: view.full_size, ..default() }); }
            // overscan: the Viewport's own view, wider
            let wide = (k > 0.0).then(|| widened(bevy::camera::SubCameraView { full_size: view.full_size, offset: Vec2::ZERO, size: view.full_size }, k));
            if c.sub_camera_view != wide { c.sub_camera_view = wide; }
            ours.insert(e);
            drawn = k;
        } else {
            if c.viewport.is_some() { c.viewport = None; }
            let view = widened(view, k);
            if c.sub_camera_view != Some(view) { c.sub_camera_view = Some(view); }
            ours.remove(&e);
            drawn = k;
        }
        if let Some(o) = over.as_mut() && o.drawn != drawn { o.drawn = drawn; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    /// a 600 x 600 frame's target (its tag row under it), a camera on it, and its own view box
    fn app() -> (App, Entity, Handle<Image>) {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        let img = Image::new_fill(Extent3d { width: 600, height: 601, depth_or_array_layers: 1 }, TextureDimension::D2, &[0, 0, 0, 0], TextureFormat::Rgba8Unorm, bevy::asset::RenderAssetUsages::default());
        let h = app.world_mut().resource_mut::<Assets<Image>>().add(img);
        app.insert_resource(TagRowTarget(h.clone())).init_resource::<ViewBox>().add_systems(Update, tag_row_view);
        let cam = app.world_mut().spawn((Camera::default(), RenderTarget::Image(h.clone().into()))).id();
        (app, cam, h)
    }

    #[test]
    fn a_box_laid_out_after_the_first_frames_takes_the_cameras_view_and_gives_it_back() {
        let (mut app, cam, _) = app();
        app.update();
        let c = app.world().get::<Camera>(cam).unwrap();
        assert_eq!(c.viewport.as_ref().map(|v| v.physical_size), Some(UVec2::new(600, 600)), "the frame above the tag row");
        // the page lays the box out (#130): the box's view, offset by its place, and no Viewport left from before
        app.world_mut().resource_mut::<ViewBox>().0 = Some([100, 100, 400, 400]);
        app.update();
        let c = app.world().get::<Camera>(cam).unwrap();
        assert!(c.viewport.is_none());
        let v = c.sub_camera_view.expect("the box's sub view");
        assert_eq!((v.full_size, v.offset, v.size), (UVec2::new(400, 400), Vec2::new(-100.0, -100.0), UVec2::new(600, 601)));
        // the box goes: the frame's Viewport again
        app.world_mut().resource_mut::<ViewBox>().0 = None;
        app.update();
        let c = app.world().get::<Camera>(cam).unwrap();
        assert!(c.sub_camera_view.is_none() && c.viewport.as_ref().is_some_and(|v| v.physical_size == UVec2::new(600, 600)));
    }

    #[test]
    fn overscan_widens_the_view_about_its_middle() {
        let (mut app, cam, _) = app();
        app.world_mut().entity_mut(cam).insert(Overscan { margin: 0.05, drawn: 0.0 });
        app.update();
        let c = app.world().get::<Camera>(cam).unwrap();
        assert_eq!(c.viewport.as_ref().map(|v| v.physical_size), Some(UVec2::new(600, 600)), "the Viewport stays");
        let v = c.sub_camera_view.expect("a wider sub view");
        assert_eq!(app.world().get::<Overscan>(cam).unwrap().drawn, 0.05);
        assert_eq!((v.full_size, v.offset, v.size), (UVec2::new(60000, 60000), Vec2::new(-3000.0, -3000.0), UVec2::new(66000, 66000)));
        // with a view box: wider about the target's middle, the box's view in the middle of it
        app.world_mut().resource_mut::<ViewBox>().0 = Some([100, 100, 400, 400]);
        app.update();
        let v = app.world().get::<Camera>(cam).unwrap().sub_camera_view.unwrap();
        assert_eq!((v.full_size, v.offset, v.size), (UVec2::new(40000, 40000), Vec2::new(-13000.0, -13005.0), UVec2::new(66000, 66110)));
        // taken off: the frame's Viewport and no sub view
        app.world_mut().resource_mut::<ViewBox>().0 = None;
        app.world_mut().entity_mut(cam).remove::<Overscan>();
        app.update();
        assert!(app.world().get::<Camera>(cam).unwrap().sub_camera_view.is_none());
    }

    #[test]
    fn a_cameras_own_viewport_is_left_as_it_is() {
        let (mut app, _, h) = app();
        let own = bevy::camera::Viewport { physical_position: UVec2::new(10, 20), physical_size: UVec2::new(50, 60), ..default() };
        let cam = app.world_mut().spawn((Camera { viewport: Some(own), ..default() }, RenderTarget::Image(h.into()))).id();
        app.world_mut().resource_mut::<ViewBox>().0 = Some([100, 100, 400, 400]);
        app.update();
        let c = app.world().get::<Camera>(cam).unwrap();
        assert!(c.sub_camera_view.is_none() && c.viewport.as_ref().is_some_and(|v| v.physical_position == UVec2::new(10, 20) && v.physical_size == UVec2::new(50, 60)));
    }
}

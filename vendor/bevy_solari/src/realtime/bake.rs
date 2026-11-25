//! raea: a baked world cache written out (SolariLighting::world_cache_save). The node copies the cache into a buffer
//! it can map at the end of the frame's update; a later frame, once that copy has run, maps it and writes the file.
//! A bake asks a few frames before its last, so the final frame's node finds the write still to finish.
use super::prepare::{WORLD_CACHE_ACTIVE_CELLS_COUNT_OFFSET, WORLD_CACHE_BUFFER_SIZE};
use std::sync::Mutex;

static PENDING: Mutex<Vec<(wgpu::Buffer, String)>> = Mutex::new(Vec::new());

pub fn save(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, cache: &wgpu::Buffer, path: &str) {
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("solari_world_cache_bake"),
        size: WORLD_CACHE_BUFFER_SIZE,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_buffer_to_buffer(cache, 0, &read, 0, WORLD_CACHE_BUFFER_SIZE);
    PENDING.lock().unwrap().push((read, path.to_string()));
}

pub fn finish(device: &wgpu::Device) {
    let pending: Vec<_> = std::mem::take(&mut *PENDING.lock().unwrap());
    for (read, path) in pending {
        let slice = read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        if let Err(e) = device.poll(wgpu::PollType::wait_indefinitely()) {
            eprintln!("raea: REFUSED the world cache bake {path}: the device: {e}");
            continue;
        }
        let bytes = match slice.get_mapped_range() {
            Ok(view) => view.to_vec(),
            Err(e) => { eprintln!("raea: REFUSED the world cache bake {path}: its copy: {e}"); continue; }
        };
        read.unmap();
        let at = WORLD_CACHE_ACTIVE_CELLS_COUNT_OFFSET as usize;
        let cells = u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        if let Some(dir) = std::path::Path::new(&path).parent() { let _ = std::fs::create_dir_all(dir); }
        match std::fs::write(&path, &bytes) {
            // a cache over about half full has started losing cells to collisions (3 probes): say so
            Ok(()) => eprintln!("raea: the world cache baked to {path} ({cells} cells, {:.0}% of its {} slots{})", 100.0 * cells as f64 / (super::prepare::WORLD_CACHE_SIZE as f64), super::prepare::WORLD_CACHE_SIZE, if cells as u64 > super::prepare::WORLD_CACHE_SIZE / 2 { "; over half full, cells are being lost: bake with larger cells (cache.cellSize)" } else { "" }),
            Err(e) => eprintln!("raea: REFUSED the world cache bake {path}: {e}"),
        }
    }
}

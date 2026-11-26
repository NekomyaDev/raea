//! The process's one shared GPU device (One renderer: one device, one frame graph): Bevy's renderer (raea-render's engine),
//! the kernels of the direct render (raea-render's gpuview) and the stage's present (present.rs) all run on it, so a
//! texture one of them draws is one the others can read. Made once, on first use, as Bevy's renderer asks for its own
//! device (its Functionality priority: every feature the adapter has, less mappable primary buffers on a
//! discrete GPU). RAEA_SHARED_DEVICE=0: callers make their
//! own devices instead (`enabled`).

use std::sync::OnceLock;

/// what a page's Vello scene uses of each bump buffer, counted on the CPU (RAEA_VELLO_BUMP_USE)
pub mod bump;
/// whether the GPU still runs our work, and a black frame checked before it's kept
pub mod check;
/// windows a texture on the shared device is presented on (live play)
pub mod present;
mod telemetry;

pub use telemetry::*;

/// What the shared device is: the instance and adapter it came from, the device and its queue.
pub type Handles = (wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue);

/// Whether callers share the device (on unless RAEA_SHARED_DEVICE=0).
pub fn enabled() -> bool {
    std::env::var("RAEA_SHARED_DEVICE").as_deref() != Ok("0")
}

/// The shared device, made on first call; every later call gets the same one (or the same error).
pub fn device() -> Result<Handles, String> {
    static SHARED: OnceLock<Result<Handles, String>> = OnceLock::new();
    SHARED.get_or_init(make).clone()
}

/// Intel's and AMD's PCI vendor ids, as DXGI and Vulkan report them.
const INTEL: u32 = 0x8086;
const AMD: u32 = 0x1002;
/// Qualcomm's (Adreno, on a Windows on ARM PC): DXGI reports "QCOM" as its vendor, some drivers 0x5143
const QUALCOMM: [u32; 2] = [0x4D4F_4351, 0x5143];

/// Whether the shared device runs on DX12 alone: on Windows, when WGPU_BACKEND isn't set and DX12 lists Intel's or
/// AMD's own hardware among its GPUs (`dx12`: each one's vendor and kind; WARP, Windows' software DirectX, is a Cpu),
/// whichever GPU it then picks. raea 0.1.7 crashed on an Intel integrated GPU under Vulkan on a Windows 10 PC, and
/// 0.1.3's app crashed before its window in AMD's OpenGL driver (atio6axx.dll, 0xc0000005) on a laptop with an AMD
/// 780M beside an RTX 5060, under wgpu's every-backend default and under Vulkan alike (#155); both ran with
/// WGPU_BACKEND=dx12. On that laptop DX12's high-performance pick is the NVIDIA GPU, so the rule looks at every GPU,
/// not the pick. A Snapdragon PC's Adreno too (#82): under Vulkan Vello's 2D pages came out striped and with grey boxes
/// behind fading text on an Adreno X1-85, and on DX12 they were clean. A PC with only NVIDIA's GPUs, a PC with no GPU,
/// a Mac or Linux, and a WGPU_BACKEND someone set
/// (Windows' check sets vulkan, for lavapipe) keep wgpu's own pick.
pub fn dx12_first(windows: bool, env: Option<wgpu::Backends>, dx12: &[(u32, wgpu::DeviceType)]) -> bool {
    windows && env.is_none() && dx12.iter().any(|&(vendor, kind)| (matches!(vendor, INTEL | AMD) || QUALCOMM.contains(&vendor)) && kind != wgpu::DeviceType::Cpu)
}

/// The backends every wgpu instance of raea's asks for (the shared device's, Bevy's own when it isn't there, systems/k's
/// kernels): WGPU_BACKEND when it's set; else, on Windows, DX12 alone when DX12 lists an Intel or AMD GPU (dx12_first);
/// else every backend, wgpu's own default. Found once a process: on Windows it asks DX12 for its adapters first.
pub fn backends() -> wgpu::Backends {
    static B: std::sync::OnceLock<wgpu::Backends> = std::sync::OnceLock::new();
    *B.get_or_init(|| {
        let env = wgpu::Backends::from_env();
        if let Some(b) = env {
            return b;
        }
        let dx12: Vec<(u32, wgpu::DeviceType)> = if cfg!(windows) {
            let i = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::DX12, ..wgpu::InstanceDescriptor::new_without_display_handle() });
            pollster::block_on(i.enumerate_adapters(wgpu::Backends::DX12)).iter().map(|a| (a.get_info().vendor, a.get_info().device_type)).collect()
        } else {
            Vec::new()
        };
        if dx12_first(cfg!(windows), env, &dx12) { wgpu::Backends::DX12 } else { wgpu::Backends::all() }
    })
}

/// Whether the shared device drops wgpu's indirect-draw validation: in a release build, unless
/// RAEA_WGPU_INDIRECT_VALIDATION=1 or the device can be DX12's. On DX12 that pass is also what writes each indirect
/// draw's first vertex and first instance (wgpu-core's D3D12 special constants), so without it Bevy's meshes are drawn
/// with each other's data: a box and a sphere came out as one mangled shape on an RTX 4070 under DX12, and whole with
/// the pass on (11-19), and DX12 is what an Intel or AMD GPU on Windows runs on (dx12_first).
pub fn indirect_validation_off(debug: bool, windows: bool, backends: wgpu::Backends, env: Option<&str>) -> bool {
    !debug && env != Some("1") && !(windows && backends.contains(wgpu::Backends::DX12))
}

fn make() -> Result<Handles, String> {
    // wgpu's indirect-draw validation off in release (it stays in debug builds and wherever DX12 can be the backend,
    // which needs it: indirect_validation_off; RAEA_WGPU_INDIRECT_VALIDATION=1 puts it back): it checks each indirect
    // draw's arguments in compute passes before drawing, and the indirect draws are Bevy's own, written by its mesh
    // preprocessing on this device. On arena3d it was ~7 extra passes and their blits a frame, ~0.07 ms of GPU
    // (lightspeed F2); pixels are the same on Metal and Vulkan.
    // wgpu's own variables are read (WGPU_BACKEND, WGPU_DX12_COMPILER, WGPU_VALIDATION, ...; nothing changes when unset,
    // but for an Intel or AMD GPU on Windows, which goes on DX12: dx12_first):
    // Windows' check renders on Mesa's lavapipe with WGPU_BACKEND=vulkan, because WARP crashes compiling one of Bevy's
    // compute shaders on GitHub's GPU-less runner (d3d10warp ComputeShaderTransformer, PT20)
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    desc.backends = backends();
    if indirect_validation_off(cfg!(debug_assertions), cfg!(windows), desc.backends, std::env::var("RAEA_WGPU_INDIRECT_VALIDATION").ok().as_deref()) {
        desc.flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
    }
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
    let mut features = adapter.features();
    if adapter.get_info().device_type == wgpu::DeviceType::DiscreteGpu {
        features.remove(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS);
    }
    let limits = adapter.limits();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("raea shared"),
        required_features: features,
        required_limits: limits,
        // as Bevy's own request does (bevy_render renderer::initialize_renderer)
        experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
        // RAEA_WGPU_TRACE=DIR: every call on the device written to DIR/trace.ron, for tools/readbacks.sh to count
        // the copies the CPU reads back (docs/blitz-bevy.feature B1d)
        trace: std::env::var_os("RAEA_WGPU_TRACE").map_or(wgpu::Trace::Off, |d| {
            let _ = std::fs::create_dir_all(&d);
            wgpu::Trace::Directory(d.into())
        }),
        ..Default::default()
    }))
    .map_err(|e| format!("GPU device: {e}"))?;
    // GPU errors nothing caught, and the device lost, in the run's trace, put to the subsystem whose labels they name
    // (quads, Bevy rendering on this device, raea-render, video; the driver when none): telemetry
    telemetry::watch(&device, "quads");
    Ok((instance, adapter, device, queue))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::{Backends, DeviceType};

    // the indirect-draw validation stays wherever DX12 can draw: it writes D3D12's first vertex and first instance
    #[test]
    fn dx12_keeps_indirect_validation() {
        let (all, dx12, vk) = (Backends::all(), Backends::DX12, Backends::VULKAN);
        assert!(!indirect_validation_off(false, true, dx12, None), "Windows on DX12 (an Intel or AMD GPU)");
        assert!(!indirect_validation_off(false, true, all, None), "Windows on every backend: DX12 may be the pick");
        assert!(indirect_validation_off(false, true, vk, None), "Windows with WGPU_BACKEND=vulkan");
        assert!(indirect_validation_off(false, false, all, None), "a Mac or Linux release");
        assert!(!indirect_validation_off(false, false, all, Some("1")), "RAEA_WGPU_INDIRECT_VALIDATION=1");
        assert!(!indirect_validation_off(true, false, all, None), "a debug build");
    }

    // an Intel or AMD GPU on Windows goes on DX12 unless WGPU_BACKEND says otherwise; NVIDIA alone and every other system
    // keep wgpu's pick
    #[test]
    fn intel_or_amd_on_windows_runs_on_dx12() {
        let (igpu, dgpu, warp) = (DeviceType::IntegratedGpu, DeviceType::DiscreteGpu, (0x1414, DeviceType::Cpu));
        assert!(dx12_first(true, None, &[(INTEL, igpu), warp]), "Intel's integrated GPU");
        assert!(dx12_first(true, None, &[(INTEL, dgpu)]), "Intel Arc");
        assert!(dx12_first(true, None, &[(AMD, igpu), (0x10de, dgpu), warp]), "#155: an AMD 780M beside an RTX 5060 (DX12 picks the NVIDIA)");
        assert!(dx12_first(true, None, &[(0x10de, dgpu), (AMD, igpu)]), "the same laptop listed NVIDIA first");
        assert!(dx12_first(true, None, &[(AMD, dgpu)]), "an AMD Radeon card");
        assert!(dx12_first(true, None, &[(0x4D4F_4351, igpu), warp]), "#82: a Snapdragon's Adreno (DXGI's QCOM)");
        assert!(dx12_first(true, None, &[(0x5143, igpu)]), "#82: an Adreno reported by its PCI id");
        assert!(dx12_first(true, None, &[(INTEL, igpu), (0x10de, dgpu)]), "an Intel laptop with an NVIDIA GPU");
        assert!(!dx12_first(true, Some(Backends::VULKAN), &[(INTEL, igpu)]), "WGPU_BACKEND=vulkan is kept");
        assert!(!dx12_first(true, Some(Backends::DX12), &[(AMD, igpu)]), "WGPU_BACKEND set: wgpu's own read of it");
        assert!(!dx12_first(true, None, &[(0x10de, dgpu), warp]), "NVIDIA alone keeps wgpu's pick");
        assert!(!dx12_first(true, None, &[warp]), "WARP alone: wgpu's pick (lavapipe, when it's there)");
        assert!(!dx12_first(true, None, &[]), "no DX12 adapter");
        assert!(!dx12_first(false, None, &[(INTEL, igpu)]), "an Intel Mac or Linux");
        assert!(!dx12_first(false, None, &[(AMD, dgpu)]), "an AMD GPU on Linux");
    }
}

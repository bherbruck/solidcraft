//! What draws a headless picture: every adapter the platform's native backends offer is
//! classified as hardware (a discrete, integrated or virtual GPU) or software (a GPU driver that
//! runs on the CPU, such as llvmpipe, lavapipe or WARP), and a [`Policy`] picks one. The last
//! tier, below any adapter, is SolidCraft's own CPU rasterizer (no surface analysis, highlights
//! or legends): `auto` lands there only when no adapter qualifies, `cpu` forces it, `hardware`
//! and `software` never use it. macOS uses Metal only, and has no software adapter. `SOLIDCRAFT_GPU`
//! (`auto`, `hardware`, `software`, `cpu`) sets the policy from the environment; `WGPU_BACKEND`
//! still narrows the backends.

use serde::Serialize;

/// Whether an adapter is a GPU or a software rasterizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Hardware,
    Software,
}

/// Which adapters a headless device may open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Policy {
    /// The best hardware adapter, else a software one.
    #[default]
    Auto,
    /// A hardware adapter or nothing.
    Hardware,
    /// A software adapter or nothing.
    Software,
    /// The CPU rasterizer, whatever adapters there are.
    Cpu,
}

/// Where a policy lands: an adapter of a tier, or the CPU rasterizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Landing {
    Hardware,
    Software,
    Cpu,
}

impl Landing {
    /// What a picture's reply names as its renderer: `gpu`, `gpu-software` or `cpu`.
    pub fn renderer(self) -> &'static str {
        match self {
            Landing::Hardware => "gpu",
            Landing::Software => "gpu-software",
            Landing::Cpu => "cpu",
        }
    }
}

impl From<Tier> for Landing {
    fn from(t: Tier) -> Self {
        match t {
            Tier::Hardware => Landing::Hardware,
            Tier::Software => Landing::Software,
        }
    }
}

impl Policy {
    /// `auto`, `hardware`, `software` or `cpu` (case-insensitive).
    pub fn parse(s: &str) -> Option<Policy> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Policy::Auto),
            "hardware" | "hw" | "gpu" => Some(Policy::Hardware),
            "software" | "sw" => Some(Policy::Software),
            "cpu" => Some(Policy::Cpu),
            _ => None,
        }
    }

    /// The policy `SOLIDCRAFT_GPU` names; [`Policy::Auto`] when it is unset, and an error when
    /// it names none.
    pub fn from_env() -> Result<Policy, String> {
        match std::env::var("SOLIDCRAFT_GPU") {
            Ok(v) => Policy::parse(&v).ok_or_else(|| format!("SOLIDCRAFT_GPU must be auto, hardware, software or cpu, not `{v}`")),
            Err(_) => Ok(Policy::Auto),
        }
    }

    /// `auto`, `hardware`, `software` or `cpu`.
    pub fn name(self) -> &'static str {
        match self {
            Policy::Auto => "auto",
            Policy::Hardware => "hardware",
            Policy::Software => "software",
            Policy::Cpu => "cpu",
        }
    }

    pub(crate) fn admits(self, tier: Tier) -> bool {
        match self {
            Policy::Auto => true,
            Policy::Hardware => tier == Tier::Hardware,
            Policy::Software => tier == Tier::Software,
            Policy::Cpu => false,
        }
    }

    /// Where the policy lands when the best qualifying adapter is of tier `adapter` (`None`: no
    /// adapter qualifies): that adapter; else the CPU rasterizer under `auto` (and always under
    /// `cpu`); else nowhere, and a picture is an error.
    pub fn lands_on(self, adapter: Option<Tier>) -> Option<Landing> {
        match (self, adapter) {
            (Policy::Cpu, _) => Some(Landing::Cpu),
            (_, Some(t)) => Some(t.into()),
            (Policy::Auto, None) => Some(Landing::Cpu),
            (Policy::Hardware | Policy::Software, None) => None,
        }
    }
}

/// One adapter as the survey saw it.
#[derive(Clone, Debug, Serialize)]
pub struct AdapterReport {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    pub tier: Tier,
}

/// The backends a headless device looks at on this platform: Metal on macOS, Vulkan and D3D12
/// on Windows, Vulkan and GL elsewhere; `WGPU_BACKEND` narrows them further.
pub fn native_backends() -> wgpu::Backends {
    let native = if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else if cfg!(target_os = "windows") {
        wgpu::Backends::DX12 | wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::VULKAN | wgpu::Backends::GL
    };
    match wgpu::Backends::from_env() {
        Some(env) if !(env & native).is_empty() => env & native,
        _ => native,
    }
}

pub(crate) fn tier_of(t: wgpu::DeviceType) -> Tier {
    match t {
        wgpu::DeviceType::Cpu => Tier::Software,
        wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::VirtualGpu | wgpu::DeviceType::Other => Tier::Hardware,
    }
}

/// Preference among adapters (higher first): a discrete GPU, an integrated one, a virtual one,
/// an unknown kind, a software rasterizer.
fn rank(t: wgpu::DeviceType) -> u8 {
    match t {
        wgpu::DeviceType::DiscreteGpu => 4,
        wgpu::DeviceType::IntegratedGpu => 3,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 1,
        wgpu::DeviceType::Cpu => 0,
    }
}

fn report(info: &wgpu::AdapterInfo) -> AdapterReport {
    AdapterReport {
        name: info.name.clone(),
        backend: format!("{:?}", info.backend),
        device_type: format!("{:?}", info.device_type),
        driver: [info.driver.as_str(), info.driver_info.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" "),
        tier: tier_of(info.device_type),
    }
}

fn instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    desc.backends = native_backends();
    wgpu::Instance::new(desc)
}

/// Every adapter the native backends offer, best first.
#[cfg(not(target_arch = "wasm32"))]
fn adapters(instance: &wgpu::Instance) -> Vec<wgpu::Adapter> {
    let mut all = pollster::block_on(instance.enumerate_adapters(native_backends()));
    all.sort_by_key(|a| std::cmp::Reverse(rank(a.get_info().device_type)));
    all
}

/// What a survey found, which adapter the policy picks (`chosen` indexes `adapters`) and where
/// it lands (`None`: nowhere, a picture is an error).
#[derive(Clone, Debug, Serialize)]
pub struct Survey {
    pub backends: String,
    pub adapters: Vec<AdapterReport>,
    pub chosen: Option<usize>,
    pub lands_on: Option<Landing>,
}

/// Every adapter the native backends offer, best first, and which one `policy` picks.
#[cfg(not(target_arch = "wasm32"))]
pub fn survey(policy: Policy) -> Survey {
    let found: Vec<AdapterReport> = adapters(&instance()).iter().map(|a| report(&a.get_info())).collect();
    let chosen = found.iter().position(|r| policy.admits(r.tier));
    let lands_on = policy.lands_on(chosen.and_then(|i| found.get(i)).map(|r| r.tier));
    Survey { backends: format!("{:?}", native_backends()), adapters: found, chosen, lands_on }
}

/// The adapter `policy` picks, with its report; `None` when no adapter qualifies.
#[cfg(not(target_arch = "wasm32"))]
pub fn pick(policy: Policy) -> Option<(wgpu::Adapter, AdapterReport)> {
    adapters(&instance())
        .into_iter()
        .map(|a| {
            let r = report(&a.get_info());
            (a, r)
        })
        .find(|(_, r)| policy.admits(r.tier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_parse_and_admit_their_tiers() {
        assert_eq!(Policy::parse(" Hardware "), Some(Policy::Hardware));
        assert_eq!(Policy::parse("sw"), Some(Policy::Software));
        assert_eq!(Policy::parse("metal"), None);
        assert_eq!(Policy::parse("CPU"), Some(Policy::Cpu));
        assert!(!Policy::Cpu.admits(Tier::Software) && !Policy::Cpu.admits(Tier::Hardware));
        assert!(Policy::Auto.admits(Tier::Software) && Policy::Auto.admits(Tier::Hardware));
        assert!(!Policy::Hardware.admits(Tier::Software));
        assert!(!Policy::Software.admits(Tier::Hardware));
    }

    /// Hardware GPU, then a software adapter, then the CPU rasterizer: `auto` walks all three,
    /// `hardware` and `software` never fall to the CPU, and `cpu` always lands there.
    #[test]
    fn policies_land_on_their_tiers() {
        use Landing as L;
        let (hw, sw) = (Some(Tier::Hardware), Some(Tier::Software));
        assert_eq!([hw, sw, None].map(|t| Policy::Auto.lands_on(t)), [Some(L::Hardware), Some(L::Software), Some(L::Cpu)]);
        assert_eq!(Policy::Hardware.lands_on(hw), Some(L::Hardware));
        assert_eq!(Policy::Hardware.lands_on(None), None);
        assert_eq!(Policy::Software.lands_on(sw), Some(L::Software));
        assert_eq!(Policy::Software.lands_on(None), None);
        assert_eq!([hw, sw, None].map(|t| Policy::Cpu.lands_on(t)), [Some(L::Cpu); 3]);
        assert_eq!([L::Hardware, L::Software, L::Cpu].map(L::renderer), ["gpu", "gpu-software", "cpu"]);
    }

    #[test]
    fn software_rasterizers_rank_last() {
        assert_eq!(tier_of(wgpu::DeviceType::Cpu), Tier::Software);
        assert!(rank(wgpu::DeviceType::IntegratedGpu) > rank(wgpu::DeviceType::Cpu));
        assert!(rank(wgpu::DeviceType::DiscreteGpu) > rank(wgpu::DeviceType::IntegratedGpu));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_surveys_metal_only() {
        assert_eq!(native_backends() & !wgpu::Backends::METAL, wgpu::Backends::empty());
        let s = survey(Policy::Auto);
        assert!(s.adapters.iter().all(|a| a.backend == "Metal"), "{s:?}");
    }
}

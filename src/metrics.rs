//! Đọc số liệu hệ thống. Không cần driver, không cần quyền admin:
//! CPU và GPU qua PDH (cùng nguồn với Task Manager), RAM qua GlobalMemoryStatusEx,
//! tên và dung lượng VRAM của card qua DXGI.

use std::collections::{HashMap, VecDeque};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::Performance::*;
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};

pub const HISTORY_LEN: usize = 60;
pub const ENGINE_LABELS: [&str; 5] = ["3D", "Copy", "Video Decode", "Video Encode", "Compute"];

const PDH_MORE_DATA: u32 = 0x800007D2;
const MICROSOFT_VENDOR_ID: u32 = 0x1414;

pub type Ring = VecDeque<f32>;

fn push(r: &mut Ring, v: f32) {
    if r.len() == HISTORY_LEN {
        r.pop_front();
    }
    r.push_back(v);
}

#[derive(Clone, Default)]
pub struct GpuStat {
    pub name: String,
    pub util: f32,
    /// (chỉ số trong ENGINE_LABELS, %), chỉ gồm loại engine card này từng báo về
    pub engines: Vec<(usize, f32)>,
    pub vram_used: u64,
    pub vram_total: u64,
    pub shared_used: u64,
}

impl GpuStat {
    pub fn vram_pct(&self) -> f32 {
        pct(self.vram_used, self.vram_total)
    }
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub cpu: f32,
    pub cpu_mhz: f32,
    pub cores: Vec<f32>,
    pub ram_used: u64,
    pub ram_total: u64,
    pub commit_used: u64,
    pub commit_limit: u64,
    pub processes: u32,
    pub threads: u32,
    pub handles: u32,
    pub uptime_s: u64,
    pub gpus: Vec<GpuStat>,
    /// Card hiển thị trên thanh taskbar: card có VRAM riêng lớn nhất
    pub main_gpu: Option<usize>,
}

impl Snapshot {
    pub fn ram_pct(&self) -> f32 {
        pct(self.ram_used, self.ram_total)
    }

    pub fn main_gpu(&self) -> Option<&GpuStat> {
        self.main_gpu.and_then(|i| self.gpus.get(i))
    }
}

fn pct(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64 * 100.0) as f32
    }
}

struct Adapter {
    name: String,
    /// "0x00000000_0x0000d1e3", khớp với tên instance của PDH
    luid: String,
    vram_total: u64,
    seen_engines: [bool; ENGINE_LABELS.len()],
}

pub struct Monitor {
    pdh: Pdh,
    adapters: Vec<Adapter>,
    pub cpu_name: String,
    pub snap: Snapshot,
    pub cpu_hist: Ring,
    pub ram_hist: Ring,
    pub gpu_hist: Vec<Ring>,
    pub vram_hist: Vec<Ring>,
}

impl Monitor {
    pub fn new() -> Self {
        let adapters = enum_adapters();
        let n = adapters.len();
        Self {
            pdh: Pdh::new(),
            adapters,
            cpu_name: cpu_name(),
            snap: Snapshot::default(),
            cpu_hist: Ring::new(),
            ram_hist: Ring::new(),
            gpu_hist: vec![Ring::new(); n],
            vram_hist: vec![Ring::new(); n],
        }
    }

    pub fn tick(&mut self) {
        unsafe { PdhCollectQueryData(self.pdh.query) };
        let mut s = Snapshot::default();

        // CPU
        if let Some(c) = self.pdh.cpu {
            s.cpu = self.pdh.value(c).unwrap_or(0.0).clamp(0.0, 100.0) as f32;
        }
        if let (Some(f), Some(p)) = (self.pdh.freq, self.pdh.perf) {
            if let (Some(f), Some(p)) = (self.pdh.value(f), self.pdh.value(p)) {
                s.cpu_mhz = (f * p / 100.0) as f32;
            }
        }
        if let Some(c) = self.pdh.cores {
            let mut cores: Vec<((u32, u32), f32)> = Vec::new();
            self.pdh.each(c, |name, v| {
                // instance dạng "0,5" = (nhóm, luồng); bỏ "_Total" và "0,_Total"
                let Some((g, i)) = name.split_once(',') else { return };
                if let (Ok(g), Ok(i)) = (g.parse(), i.parse()) {
                    cores.push(((g, i), v.clamp(0.0, 100.0) as f32));
                }
            });
            cores.sort_by_key(|c| c.0);
            s.cores = cores.into_iter().map(|c| c.1).collect();
        }

        // RAM
        let mut ms = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if unsafe { GlobalMemoryStatusEx(&mut ms) }.is_ok() {
            s.ram_total = ms.ullTotalPhys;
            s.ram_used = ms.ullTotalPhys - ms.ullAvailPhys;
        }
        let mut pi = PERFORMANCE_INFORMATION {
            cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
            ..Default::default()
        };
        if unsafe { GetPerformanceInfo(&mut pi, pi.cb) }.is_ok() {
            let page = pi.PageSize as u64;
            s.commit_used = pi.CommitTotal as u64 * page;
            s.commit_limit = pi.CommitLimit as u64 * page;
            s.processes = pi.ProcessCount;
            s.threads = pi.ThreadCount;
            s.handles = pi.HandleCount;
        }
        s.uptime_s = unsafe { GetTickCount64() } / 1000;

        // GPU
        let mut gpus: Vec<GpuStat> = self
            .adapters
            .iter()
            .map(|a| GpuStat {
                name: a.name.clone(),
                vram_total: a.vram_total,
                ..Default::default()
            })
            .collect();
        let adapters = &self.adapters;
        let find = |luid: &str| adapters.iter().position(|a| a.luid.eq_ignore_ascii_case(luid));

        // Instance: "pid_1234_luid_0x00000000_0x0000D1E3_phys_0_eng_0_engtype_3D".
        // Cộng dồn mọi tiến trình theo từng engine; % của card = engine bận nhất (giống Task Manager).
        let mut engines: HashMap<(usize, String), (Option<usize>, f64)> = HashMap::new();
        if let Some(c) = self.pdh.gpu_engine {
            self.pdh.each(c, |name, v| {
                let (Some(luid), Some(eng), Some(ty)) = (
                    between(name, "luid_", "_phys_"),
                    between(name, "_phys_", "_engtype_"),
                    name.split("_engtype_").nth(1),
                ) else {
                    return;
                };
                let Some(ai) = find(luid) else { return };
                let e = engines.entry((ai, eng.to_string())).or_insert((engine_kind(ty), 0.0));
                e.1 += v;
            });
        }
        let mut kinds = vec![[0f32; ENGINE_LABELS.len()]; gpus.len()];
        for ((ai, _), (kind, v)) in engines {
            let v = v.clamp(0.0, 100.0) as f32;
            gpus[ai].util = gpus[ai].util.max(v);
            if let Some(k) = kind {
                kinds[ai][k] = kinds[ai][k].max(v);
                self.adapters[ai].seen_engines[k] = true;
            }
        }
        for (i, g) in gpus.iter_mut().enumerate() {
            g.engines = (0..ENGINE_LABELS.len())
                .filter(|&k| self.adapters[i].seen_engines[k])
                .map(|k| (k, kinds[i][k]))
                .collect();
        }

        // Instance: "luid_0x00000000_0x0000D1E3_phys_0"
        let adapters = &self.adapters;
        let find = |luid: &str| adapters.iter().position(|a| a.luid.eq_ignore_ascii_case(luid));
        for (counter, shared) in [(self.pdh.gpu_dedicated, false), (self.pdh.gpu_shared, true)] {
            let Some(c) = counter else { continue };
            self.pdh.each(c, |name, v| {
                let Some(ai) = between(name, "luid_", "_phys").and_then(find) else { return };
                if shared {
                    gpus[ai].shared_used += v as u64;
                } else {
                    gpus[ai].vram_used += v as u64;
                }
            });
        }
        s.main_gpu = (0..gpus.len()).max_by_key(|&i| gpus[i].vram_total);
        s.gpus = gpus;

        push(&mut self.cpu_hist, s.cpu);
        push(&mut self.ram_hist, s.ram_pct());
        for (i, g) in s.gpus.iter().enumerate() {
            push(&mut self.gpu_hist[i], g.util);
            push(&mut self.vram_hist[i], g.vram_pct());
        }
        self.snap = s;
    }
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)? + i;
    Some(&s[i..j])
}

fn engine_kind(ty: &str) -> Option<usize> {
    let ty = ty.replace(' ', "");
    match ty.as_str() {
        "3D" => Some(0),
        "Copy" => Some(1),
        "VideoDecode" => Some(2),
        "VideoEncode" => Some(3),
        t if t.starts_with("Compute") || t.starts_with("Cuda") => Some(4),
        _ => None,
    }
}

struct Pdh {
    query: PDH_HQUERY,
    cpu: Option<PDH_HCOUNTER>,
    cores: Option<PDH_HCOUNTER>,
    freq: Option<PDH_HCOUNTER>,
    perf: Option<PDH_HCOUNTER>,
    gpu_engine: Option<PDH_HCOUNTER>,
    gpu_dedicated: Option<PDH_HCOUNTER>,
    gpu_shared: Option<PDH_HCOUNTER>,
    /// Bộ đệm cho PdhGetFormattedCounterArrayW, u64 để đủ căn lề 8 byte
    buf: Vec<u64>,
}

/// PDH_FMT_DOUBLE | PDH_FMT_NOCAP100 (crate không export hằng NOCAP100)
const FMT: PDH_FMT = PDH_FMT(PDH_FMT_DOUBLE.0 | 0x8000);

impl Pdh {
    fn new() -> Self {
        let mut query = PDH_HQUERY::default();
        unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
        let add = |path: PCWSTR| {
            let mut c = PDH_HCOUNTER::default();
            (unsafe { PdhAddEnglishCounterW(query, path, 0, &mut c) } == 0).then_some(c)
        };
        let pdh = Self {
            query,
            // "% Processor Utility" là số Task Manager hiển thị; máy cũ không có thì lùi về "% Processor Time"
            cpu: add(w!("\\Processor Information(_Total)\\% Processor Utility"))
                .or_else(|| add(w!("\\Processor Information(_Total)\\% Processor Time"))),
            cores: add(w!("\\Processor Information(*)\\% Processor Utility"))
                .or_else(|| add(w!("\\Processor Information(*)\\% Processor Time"))),
            freq: add(w!("\\Processor Information(_Total)\\Processor Frequency")),
            perf: add(w!("\\Processor Information(_Total)\\% Processor Performance")),
            gpu_engine: add(w!("\\GPU Engine(*)\\Utilization Percentage")),
            gpu_dedicated: add(w!("\\GPU Adapter Memory(*)\\Dedicated Usage")),
            gpu_shared: add(w!("\\GPU Adapter Memory(*)\\Shared Usage")),
            buf: Vec::new(),
        };
        // Counter dạng tỉ lệ cần hai lần lấy mẫu, lấy lần đầu ngay để tick đầu tiên đã có số
        unsafe { PdhCollectQueryData(query) };
        pdh
    }

    fn value(&self, c: PDH_HCOUNTER) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let st = unsafe { PdhGetFormattedCounterValue(c, FMT, None, &mut v) };
        (st == 0 && v.CStatus <= 1).then(|| unsafe { v.Anonymous.doubleValue })
    }

    /// Gọi `f(tên instance, giá trị)` cho từng instance của counter wildcard.
    fn each(&mut self, c: PDH_HCOUNTER, mut f: impl FnMut(&str, f64)) {
        // Số instance có thể tăng giữa hai lần gọi (tiến trình mới dùng GPU), nên thử lại vài lần
        for _ in 0..3 {
            let (mut size, mut count) = (0u32, 0u32);
            let st = unsafe { PdhGetFormattedCounterArrayW(c, FMT, &mut size, &mut count, None) };
            if st != PDH_MORE_DATA {
                return;
            }
            let words = (size as usize).div_ceil(8);
            if self.buf.len() < words {
                self.buf.resize(words, 0);
            }
            let ptr = self.buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            let st = unsafe { PdhGetFormattedCounterArrayW(c, FMT, &mut size, &mut count, Some(ptr)) };
            if st == PDH_MORE_DATA {
                continue;
            }
            if st != 0 {
                return;
            }
            let items = unsafe { std::slice::from_raw_parts(ptr, count as usize) };
            let mut name = String::new();
            for it in items {
                if it.FmtValue.CStatus > 1 {
                    continue;
                }
                name.clear();
                let wide = unsafe { it.szName.as_wide() };
                name.extend(char::decode_utf16(wide.iter().copied()).map(|r| r.unwrap_or('?')));
                f(&name, unsafe { it.FmtValue.Anonymous.doubleValue });
            }
            return;
        }
    }
}

fn enum_adapters() -> Vec<Adapter> {
    let mut out: Vec<Adapter> = Vec::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return out;
    };
    let mut i = 0;
    while let Ok(a) = unsafe { factory.EnumAdapters1(i) } {
        i += 1;
        let Ok(d) = (unsafe { a.GetDesc1() }) else { continue };
        if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || d.VendorId == MICROSOFT_VENDOR_ID {
            continue;
        }
        let luid = format!("0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
        if out.iter().any(|x| x.luid == luid) {
            continue;
        }
        let len = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
        out.push(Adapter {
            name: String::from_utf16_lossy(&d.Description[..len]).trim().to_string(),
            luid,
            vram_total: d.DedicatedVideoMemory as u64,
            seen_engines: Default::default(),
        });
    }
    out
}

fn cpu_name() -> String {
    let mut buf = [0u16; 128];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as _),
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS {
        return "CPU".into();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len]).trim().to_string()
}

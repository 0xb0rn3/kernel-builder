// Plain-English meaning of every option that goes into a built kernel, so a user choosing what to
// put in their custom kernel understands each one. Surfaced by `akt explain` and reused by the GUI.
use crate::{Distro, Flavor};

pub struct Item { pub key: &'static str, pub what: &'static str }

/// The tuned performance base (from CachyOS), applied to every flavor.
pub const PERF: &[Item] = &[
    Item { key: "BORE scheduler", what: "Burst-Oriented Response Enhancer: a CPU scheduler tuned for desktop responsiveness under load (snappier UI, faster interactive tasks)." },
    Item { key: "Clang ThinLTO", what: "Whole-kernel link-time optimization: smaller, faster code. Heavier to build, faster at runtime. Disabled on the RT flavor (RT and LTO do not mix)." },
    Item { key: "BBRv3 congestion", what: "Google's TCP congestion control: higher throughput and lower latency on real networks than the default CUBIC." },
    Item { key: "1000 Hz + full preempt", what: "The kernel checks for work 1000x/second and can preempt almost anywhere: lower latency, more responsive under load." },
    Item { key: "x86-64-v3 baseline", what: "Builds for 2015+ CPUs (AVX2 and friends), so the compiler can emit faster instructions than the generic x86-64 baseline." },
    Item { key: "THP + MGLRU", what: "Transparent huge pages and the multi-generational LRU: better memory throughput and smarter reclaim under pressure." },
];

/// The ARXOS hardening / DEATHSTROKE config delta (the "our patches" part).
pub const HARDEN: &[Item] = &[
    Item { key: "init_on_free", what: "Zeroes memory the moment it is freed, so secrets (LUKS keys, passwords) cannot be scavenged from freed RAM." },
    Item { key: "init_on_alloc", what: "Zeroes memory when it is allocated: kills whole classes of uninitialized-memory info leaks and exploits." },
    Item { key: "hibernation OFF", what: "No suspend-to-disk, so an encryption key is never written to a swap file in plaintext." },
    Item { key: "IOMMU enforce+strict", what: "IOMMU on with strict flushing, so a malicious DMA device (Thunderbolt, PCIe) cannot read arbitrary RAM." },
    Item { key: "lockdown LSM (early)", what: "Restricts even root from reading/altering the running kernel (/dev/mem, kexec, raw MSRs). Enforced when you add lockdown= to the cmdline or enable Secure Boot." },
    Item { key: "IMA measured boot", what: "Measures boot components into the TPM so boot-chain tampering is detectable, and can gate TPM key release (deathstr0ke seal)." },
    Item { key: "dmesg restricted", what: "Only root can read the kernel log, so its addresses and secrets are not exposed to unprivileged users." },
    Item { key: "livepatch", what: "Apply security fixes to the running kernel without a reboot." },
];

/// Security / wireless-auditing capabilities (Kali-style). On Arch these live in the ARXOS config
/// fragment; on the Debian path they are added as Kali patches. Present on both build paths.
pub const SECURITY: &[Item] = &[
    Item { key: "wireless injection", what: "Wi-Fi adapters can enter monitor mode and inject frames, so aircrack-ng and the wireless-audit suite work (the core Kali capability)." },
    Item { key: "extra Wi-Fi drivers", what: "Injection-capable out-of-tree Realtek/Atheros chipset drivers, covering adapters mainline does not." },
    Item { key: "USB HID / BadUSB", what: "Raw USB HID gadget support for keystroke-injection testing and hardware security work." },
    Item { key: "netfilter NFQUEUE", what: "NFNETLINK_QUEUE lets security tools intercept, inspect and rewrite packets in the kernel path (also what AnonKit builds on)." },
    Item { key: "eBPF + BTF", what: "In-kernel programmable tracing and filtering for observability and defensive tooling, with CO-RE type info." },
];

fn json_arr(items: &[Item]) -> String {
    let v: Vec<String> = items.iter()
        .map(|i| format!("{{\"option\":\"{}\",\"meaning\":\"{}\"}}", crate::json_escape(i.key), crate::json_escape(i.what)))
        .collect();
    format!("[{}]", v.join(","))
}

/// The same meaning table as a JSON object (drives the GUI's /api/explain).
pub fn as_json(_distro: Distro, fl: &Flavor, kname: &str) -> String {
    format!("{{\"kernel\":\"linux-{}\",\"flavor\":{{\"key\":\"{}\",\"meaning\":\"{}\"}},\"performance\":{},\"hardening\":{},\"security\":{}}}",
        crate::json_escape(kname), fl.key, crate::json_escape(fl.about),
        json_arr(PERF), json_arr(HARDEN), json_arr(SECURITY))
}

/// Print the meaning of everything that will go into `linux-<kname>` for this host + flavor.
pub fn print(distro: Distro, fl: &Flavor, kname: &str, json: bool) {
    if json { println!("{}", as_json(distro, fl, kname)); return; }
    println!("== what goes into your kernel: linux-{kname}  (flavor: {}) ==", fl.key);
    println!("\n  flavor {:<10} {}", fl.key, fl.about);
    let grp = |title: &str, items: &[Item]| {
        println!("\n  {title}");
        for i in items { println!("    - {:<22} {}", i.key, i.what); }
    };
    grp("PERFORMANCE  (the tuned base, every flavor)", PERF);
    grp("HARDENING  (ARXOS / DEATHSTROKE delta)", HARDEN);
    grp("SECURITY / WIRELESS AUDITING  (Kali-style)", SECURITY);
    println!("\n  name: installs as linux-{kname}; `uname -r` shows -{kname}. Change it with --name NAME.");
}

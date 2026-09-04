// Config gate: verify the ARXOS hardening / DEATHSTROKE delta is present in a built .config (or a
// config-*-arxos file). Used by `akt gate` and by the build engine's fail-fast before compiling —
// this is exactly what stops a kernel that silently lost the delta from ever being shipped.
use std::fs;

const REQUIRE_Y: &[(&str, &str)] = &[
    ("CONFIG_INIT_ON_FREE_DEFAULT_ON", "zero freed memory (protect the in-RAM LUKS key)"),
    ("CONFIG_INIT_ON_ALLOC_DEFAULT_ON", "zero on alloc"),
    ("CONFIG_INTEL_IOMMU_DEFAULT_ON", "IOMMU enforcing (anti-DMA RAM capture)"),
    ("CONFIG_IOMMU_DEFAULT_DMA_STRICT", "IOMMU strict mode"),
    ("CONFIG_SECURITY_LOCKDOWN_LSM_EARLY", "lockdown LSM effective from early boot"),
    ("CONFIG_IMA", "IMA measured boot"),
    ("CONFIG_LIVEPATCH", "live kernel patching (hot CVE fixes, no reboot)"),
    ("CONFIG_SECURITY_DMESG_RESTRICT", "restrict kernel log to root"),
];

/// Returns true when every required option is present. Prints a report unless `json`.
pub fn verify(path: &str, json: bool) -> bool {
    let cfg = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            if json { println!("{{\"ok\":false,\"error\":\"cannot read {}\"}}", crate::json_escape(&e.to_string())); }
            else { eprintln!("akt gate: cannot read {path}: {e}"); }
            return false;
        }
    };
    let has_y = |k: &str| cfg.lines().any(|l| l.trim() == format!("{k}=y"));
    let hibernation_off = cfg.lines().any(|l| l.trim() == "# CONFIG_HIBERNATION is not set");

    let mut missing: Vec<&str> = REQUIRE_Y.iter().filter(|(k, _)| !has_y(k)).map(|(k, _)| *k).collect();
    if !hibernation_off { missing.push("HIBERNATION-off"); }

    if json {
        let m: Vec<String> = missing.iter().map(|s| format!("\"{s}\"")).collect();
        println!("{{\"ok\":{},\"missing\":[{}]}}", missing.is_empty(), m.join(","));
        return missing.is_empty();
    }
    println!("== ARXOS config gate: {path} ==");
    for (k, why) in REQUIRE_Y {
        println!("  {}  {k}  ({why})", if has_y(k) { "PASS" } else { "FAIL" });
    }
    println!("  {}  CONFIG_HIBERNATION off  (no plaintext key in swsusp)", if hibernation_off { "PASS" } else { "FAIL" });
    if missing.is_empty() { println!("== GREEN: ARXOS hardening delta present =="); true }
    else { println!("== FAILED: missing {} ==", missing.join(", ")); false }
}

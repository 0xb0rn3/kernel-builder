<div align="center">

# ARXOS Kernel Tuner

**A tuned, hardened kernel on Arch or Debian, in one tool.**

CachyOS performance tunes · ARXOS hardening / DEATHSTROKE config · Kali wireless patches (Debian) · GUI

<sub>An <a href="https://arxos.uk">ArxOS</a> tool by <a href="https://stingraylabs.pages.dev">Stingray Labs</a>.</sub>

</div>

## What it does

`akt` builds a performance-tuned, security-hardened kernel for whatever host you run it on, and
verifies the hardening actually landed **before** the long compile.

- **Arch family** (Arch, CachyOS, EndeavourOS, ...): forks the CachyOS `linux-cachyos` recipe
  (BORE, Clang ThinLTO, BBRv3, 1000 Hz, x86-64-v3), overlays the ARXOS config fragment + patches,
  and builds a pacman package `linux-arxos` (+ headers).
- **Debian family** (Debian, Ubuntu, Kali): takes the Debian kernel source, applies CachyOS-style
  tunes as a Kconfig fragment, then the same ARXOS hardening delta, then **Kali-style wireless
  injection patches**, and builds `linux-image-*-arxos.deb` (+ headers) so Debian users get the same
  tuned, hardened kernel.

The ARXOS hardening delta (the "our patches" part) is the same one `linux-arxos` ships:
`init_on_free` + `init_on_alloc` (zero freed/allocated memory so keys can't be scavenged),
**hibernation off** (no plaintext key in swsusp), **IOMMU enforcing + strict** (anti-DMA RAM
capture), **lockdown LSM early**, **IMA** measured boot, **livepatch**, **dmesg restrict**. It is the
kernel side of the [deathstr0ke](https://github.com/0xb0rn3/deathstr0ke) anti-forensics toolkit.

## The config gate (why this exists)

`olddefconfig` can silently revert fragment options, which once shipped a "hardened" kernel that
quietly wasn't. So `akt` re-asserts the delta with `scripts/config` and **hard-verifies it in
`.config` at the config stage** — a missing option aborts the build in seconds instead of after a
multi-hour compile. `akt gate <config>` runs the same check standalone.

## Usage

```sh
akt detect                 # host distro + which build path applies
akt explain                # what each option means for your custom kernel
akt plan                   # print the tune plan (base, tunes, patches), no build
akt build                  # build a tuned kernel for this host
akt build --name mykernel  # rename the kernel: builds linux-mykernel (default: linux-arxos)
akt build --flavor rt      # real-time flavor (PREEMPT_RT + BORE)
akt build --dry-run        # show the exact steps without building
akt gate path/to/.config   # verify the hardening delta is present
akt gui                    # local web GUI at http://127.0.0.1:8787
```

Flavors: `default` (BORE + ThinLTO), `rt` (PREEMPT_RT), `hardened` (KSPP + kCFI). Name your kernel
with `--name NAME` (builds `linux-NAME`; default `arxos`), so it is yours, not branded. `akt explain`
describes every performance and hardening option that goes into the build, in plain English, before
you commit to a long compile. Add `--json` to `detect` / `plan` / `explain` / `gate` for scripting.

## GUI

`akt gui` starts a loopback-only web UI (no telemetry, no external calls beyond Google Fonts) that
detects the host, shows the resolved tune plan for the chosen flavor, and runs the build with a live
log. It matches the ArxOS Control Center design language.

## Install

```sh
git clone https://github.com/0xb0rn3/kernel-builder
cd kernel-builder
sudo ./install.sh          # cargo build --release + install binary, tunes, patches, GUI
```

Needs `rust`/`cargo` to build, and the host's kernel-build toolchain (Arch: `base-devel` + `clang`
`lld` `llvm`; Debian: `build-essential fakeroot libncurses-dev bison flex libssl-dev libelf-dev`).

## Adding patches

- `patches/arxos/*.patch` — applied on every host (after the config overlay).
- `patches/kali/*.patch` — applied on the Debian path only (wireless injection, extra HID, ...).
- `config/arxos.fragment` — the Kconfig overlay (the hardening delta). Edit here to change the tune.

Patches are applied `--forward` and skipped (never fatal) if they don't apply on the current base.

## Status

MVP. The core orchestration, the config gate, and the GUI are implemented; the Arch path mirrors the
proven `linux-arxos` build. End-to-end builds are long and, for the Debian path, need validation on a
real Debian/Kali host. Always boot-test a built kernel in a throwaway VM before installing on
hardware.

## License

GPL-3.0-or-later (kernel-adjacent tooling).

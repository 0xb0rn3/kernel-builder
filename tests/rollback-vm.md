# akt rollback — VM test procedure

`cargo test` covers the pure selection logic (version ordering incl. 7.9 < 7.10, downgrade-guard
semantics, absent-package handling). Those are host-safe and reproducible.

The **actual command** — which ends in a real `sudo pacman -U` — must be validated in a disposable
VM, never on the host. Procedure (matches the 2026-09-14 run):

1. Build the exact binary on the host: `cargo build --release`; record its sha256.
2. Boot a throwaway that has `linux-<name>` installed and a populated
   `<AKT_WORK>/cos/linux-cachyos/prev/` (an older `linux-<name>` + headers). Record the guest
   `product_uuid` (`cat /sys/class/dmi/id/product_uuid`).
3. Copy the recorded binary in (verify the in-guest sha256 matches the host).
4. Run `AKT_WORK=<dir> akt rollback` and capture output. Expected: it selects the archived version,
   prints `rolling back <pkg> <installed> -> <older> (downgrade)`, runs `pacman -U -- <pkg> [hdr]`,
   and the package downgrades. Reboot and confirm `uname -r` / `pacman -Q linux-<name>`.
5. Negative cases in the VM: empty `prev/` (clean refusal, no pacman call); an archive whose version
   is >= installed (refused as a non-downgrade); a `.sig` file present (ignored).
6. Shut the VM off afterwards.

Evidence for the shipped run (guest UUID, binary sha256, per-case results) lives with the kernel-gate
evidence in the private ARXOS validation tree, not in this public repo.

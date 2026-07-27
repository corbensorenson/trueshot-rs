# TrueShot — Review Round 2, for Codex

Reviewed at `6500a70d` ("Record full-sensor Apple NEF baseline") plus the
uncommitted working tree, 2026-07-27 ~10:45 CDT.

Since the last review point (`c646c081`): **12 commits, 83 files, +18,575 / −757.**
None of the previous round's findings were available to you, so this document
restates the ones that still stand and adds what the overnight work introduced.

Everything below was verified against the source or measured by running it.

---

## 0. Read this first: the Metal AHD parity gate is red at production resolution

`44c9bf06 "Add qualified Metal AHD demosaicing"` shipped, and 11 commits were
built on top of it. I re-ran the qualification harness against the current tree:

```
$ cargo run --release -p trueshot-core --features gpu \
    --example demosaic_metal_qualification -- 8256 5504 3

  adapter Apple M1        bands 11        scratch_bytes 484,790,336
  cpu    p50 1067.9 ms    metal p50 636.7 ms     speedup 1.68x
  maximum_absolute_error 0.03698188066482544     (tolerance 1e-3)
  values_over_tolerance  10
  measured_cfa_exact     true
  Error: Metal AHD failed the retained parity contract          [exit 1]
```

This is **bit-for-bit the same failure as ~12 hours ago** — same max error to the
last digit, same coordinates, same checksums (`e115f88c6fa7fa03` cpu vs
`e1160f147a20f9c4` metal). Nothing in the overnight work touched the cause.

### Why the gate says "pass"

`scripts/run_apple_metal_ahd_qualification.sh` defaults to:

```sh
WIDTH=1310  HEIGHT=1304       #  1.71 MP
HDR_WIDTH=1536 HDR_HEIGHT=700 #  1.08 MP
```

Both sit just above `MIN_GPU_PIXELS = 1_000_000`. The committed baseline
`docs/benchmarks/apple_metal_ahd_qualification_2026-07-27.json` records exactly
those two sizes, at `values_over_tolerance: 0`.

Meanwhile `docs/benchmarks/apple_nef_full_sensor_qualification_2026-07-27.json`
records the production path:

```json
"configuration": { "expected_width": 8280, "expected_height": 5520 },
"gates":         { "require_metal_ahd_without_fallback": true }
```

**So production mandates the Metal AHD path, with no fallback permitted, at 45.7 MP
— and the only correctness evidence for that path is measured at 1/26th of the
frame area, where the defect does not appear.** The gate is calibrated to the
size that passes, not to the size the product runs.

`docs/FEATURE_MATRIX.md:28` now states the result as fact —
"The retained M1 release gate has exact CFA samples, 123.965 dB CPU parity,
5.29e-4 max normalized error … A private 21-frame Z9 run completed three Metal
bands and atomic exports without fallback". Three bands is a ~1500-row ROI. The
full-sensor run is 11 bands. Those are different regimes and the row reads as if
they are the same one.

### What is actually wrong (unchanged from the last round)

The shader computes XYZ with FMA-contractable float expressions
(`gpu_ahd.rs:630-647`), truncates the result into a 65536-entry LUT, and then
compares homogeneity with **exact integer `<=`** (`gpu_ahd.rs:774`,
`demosaic_ahd.rs:702`). One ULP of difference from the CPU's sequential `f32`
accumulation (`demosaic_ahd.rs:110`) flips a LUT index → shifts Lab by ~1 unit →
flips a homogeneity count → flips the **direction selection**. The two
directional candidates differ by a real interpolation difference, not an epsilon.

The failing pixels prove this is the mechanism. They arrive as **channel-0 /
channel-2 pairs at a single coordinate** — `(2886, 371)`, `(4921, 1166)`,
`(7955, 4080)` — R off by ~0.0314, B off by ~0.0368, **green untouched**. The
horizontal and vertical candidates share their green estimate and differ only in
R and B. That is a direction flip, not rounding. It is also not a band-seam bug:
none of those rows are near the band boundaries at `5 + 512k`.

### Fix

**Make Lab quantization bit-exact across backends.** Pre-scale `xyz_cam` to fixed
point at engine construction and evaluate the 3×3 product in `i32`/`i64` in both
the WGSL and `CieLabConverter::convert`. Once the Lab triples are identical
integers, `build_homogeneity` and `combine_directions` are pure integer
arithmetic, direction selection is identical **by construction**, and the only
residual difference is the final `0.5*(h+v)` average — a genuine 1-ULP concern.
Parity stops being a hope and becomes a property.

Then:
- Set the qualification default to full sensor size (`8256x5504`), or add it as a
  mandatory third case alongside the two small ones. A gate that only runs below
  the resolution the product uses is not a gate.
- Until that is green, either gate the Metal path off above some verified pixel
  count, or stop asserting `require_metal_ahd_without_fallback: true` in the
  production qualification.

### Do not do these

- **Do not widen `HOMOGENEITY_DECISION_MARGIN` further.** Each increment pushes
  more pixels into the unconditional 50/50 average — exactly the blur AHD exists
  to avoid — and there is no margin at which a float-derived integer comparison
  becomes deterministic across two compilers. At `1` it already fails by 37x.
- **Do not raise `BAND_OUTPUT_ROWS` again.** 192 → 512 bought 1.12x → 1.68x, but
  drove admitted scratch to **485 MB for one 45 MP frame**, which the memory-credit
  pool must reserve before decode. The fix for per-band sync cost is overlap
  (double-buffer the bands, one `map_async` in flight), not bigger bands. See §4.

### One documentation claim needs evidence or removal

`docs/FEATURE_MATRIX.md:28` says AHD "uses a robust homogeneity margin that
improves synthetic red/blue PSNR". There is **no test in `demosaic_ahd.rs`
asserting this** — the test list is unchanged: `lab_lookup_retains_neutral_chroma`,
`f32_ahd_restores_hdr_range`, `ahd_preserves_every_measured_cfa_sample_including_borders`,
`ahd_preserves_true_black_and_rejects_invalid_sensor_values`. `HOMOGENEITY_DECISION_MARGIN`
changed CPU output for every user on every platform. Either land the test that
substantiates the PSNR claim, or revert the margin.

---

## 1. Still open from the last round — verified again just now

| # | Finding | Status |
|---|---|---|
| P0.1 | `cargo check -p trueshot-core --examples` fails: `unresolved import trueshot_core::gpu` | **Open — re-verified** |
| P0.2 | `gpu = []` does not enable the `wgpu` dep it needs | **Open** |
| P0.3 | `block_in_place` in auth middleware under `#[actix_web::main]` | **Open** |
| P0.4 | `verify_api_token` returns `Role::Admin` for every token; `require_scope` short-circuits on Admin | **Open** |
| P0.5a | at-rest master key had no non-keyring source | **FIXED** ✅ |
| P0.5b | JWT HMAC secret still keyring-only → Docker still cannot boot | **Open** |
| P1.6 | `rgb_cam` hard-coded to identity at the production call site | **Open** (`main.rs:2656`) |

### P0.1 — 2 minutes, verified failing
```
trueshot-core/examples/demosaic_metal_qualification.rs:7:20:
  error[E0432]: unresolved import `trueshot_core::gpu`
```
`trueshot-core` defaults to `["wgpu"]`, not `["gpu"]`. CI hides this because
`cargo test --workspace` unifies features from `trueshot-cli`/`trueshot-server`.
The README's own `cargo test -p trueshot-core --no-run` is broken.
```toml
[[example]]
name = "demosaic_metal_qualification"
required-features = ["gpu"]
```
Add `cargo check -p trueshot-core --all-targets` (no `--workspace`) to CI.

### P0.2
`gpu = ["dep:wgpu", "dep:pollster"]`, plus a CI entry building
`--no-default-features --features gpu`.

### P0.3 — will panic in production
`auth/mod.rs:869` and `:895` call `tokio::task::block_in_place(|| Handle::current().block_on(…))`
inside `Service::call`. `main.rs:59` is `#[actix_web::main]` → actix-rt
**current-thread** runtime per worker → `block_in_place` panics.

Reachable by any request carrying `X-API-Token:` / `Authorization: Token …`, or a
correct `X-API-Key`. `grep -rn "X-API-Token" trueshot-server` still returns only
the middleware itself — zero test coverage on that path, which is why it has
survived.

Fix: move the store lookup into the returned `Box::pin(async move { … })` instead
of blocking inside `call`. Add an integration test that mints an API token and
uses it.

### P0.4 — API token scopes are decorative
`auth/mod.rs:278` returns `role: Role::Admin` unconditionally, ignoring the
owning user. `require_scope` (`:744`) returns `Ok(())` immediately for Admin. A
token created with `["read"]` has full admin authority.

Fix: resolve the owner's real role from `token.user_id`; drop the Admin
short-circuit in `require_scope`. Tests: guest-owned token rejected on an admin
route; `["read"]`-scoped token rejected on a write route.

### P0.5 — half fixed, and the fixed half is good
`at_rest.rs` now does the right thing:
```
MASTER_KEY_ENV = "TRUESHOT_MASTER_KEY"           at_rest.rs:18
env → privacy.encryption_master_key_path → keyring
"Master key required in production. Set TRUESHOT_MASTER_KEY or
 privacy.encryption_master_key_path"            at_rest.rs:284
```
Env-first, file second, keyring last, **fail-closed in production**. That is the
right shape.

`auth/mod.rs:1216 load_or_create_hmac_secret` did not get the same treatment —
still keyring-only, no env, no file. Since `AuthManager::new` runs at startup,
the container still cannot boot (the runtime image has `libdbus-1-3` but no
secret-service daemon). Apply the identical precedence chain, and refuse to
auto-generate an ephemeral secret in production — a regenerated HMAC key silently
invalidates every session on restart.

Then add a CI job that boots the container and curls `/api/health`.

---

## 2. The overnight work — what is genuinely good

I want to be clear that a lot of this is strong, because the rest of this document
is criticism.

### `trueshot-storage/src/encrypted.rs` (TSE2) — well built

This is the best new code in the batch. Reviewed the full 537 lines:

- **Per-file key derivation.** `Hkdf::<Sha256>::new(Some(file_id), master_key)`
  with a random 16-byte `file_id` per file (`:447`). A nonce-prefix collision
  cannot cause key+nonce reuse unless `file_id` also collides (2⁻¹²⁸).
- **Nonce space is disjoint by construction.** Header uses counter `u32::MAX`
  (`:24`); `chunk_count()` is capped at `u32::MAX` so the highest chunk index is
  `u32::MAX - 1`. No collision. (Worth a comment — it is correct but implicit.)
- **AAD binds position and length.** `chunk_aad` = header AAD ‖ index ‖ plaintext
  length (`:455`), and the header AAD covers `chunk_size`, `plaintext_len`,
  `file_id`, `nonce_prefix`. Chunk swapping, truncation, splicing, and
  cross-file transplant are all rejected. The tests actually exercise these
  (`chunk_swapping_is_rejected_by_position_bound_aad`,
  `truncation_and_append_are_rejected_at_open`).
- **Length is checked at open** against `header.encoded_len()` (`:92-98`) before
  any chunk read.
- **Encrypt-time TOCTOU handled** — `consumed != plaintext_len` and a trailing
  read to detect growth (`:321-329`). Nice.
- `create_new(true)` everywhere, `sync_all()`, `Zeroizing` on plaintext buffers.

Offset arithmetic checks out: only the final chunk is short, so
`HEADER_LEN + index * (chunk_size + TAG_LEN)` is exact for every index.

Two notes, neither urgent:
- **Single-chunk cache with no readahead** (`cached_index`, `:82`). Sequential
  reads are fine, but NEF/TIFF access is header → IFD → strip/tile offsets →
  pixel data, which bounces. Every bounce re-decrypts a full 1 MiB chunk. For a
  feature whose pitch is "a small RAW ROI never requires decrypting the complete
  source file", measure this against the real ROI access pattern — a 2–4 entry
  LRU may be the whole fix.
- **AES-GCM is not key-committing.** Not exploitable in this threat model (the
  attacker does not hold a second key), but if TSE2 ever spans tenants, note it.

### `fusion_revision.rs` — careful subprocess handling

- `resolve_packaged_cli()` (`:625`) canonicalizes `current_exe()`, pins the child
  to `trueshot` in the same directory, and re-verifies parent and filename after
  canonicalization. Good.
- `Command::new(...).arg(...)` — no shell, no injection.
- `kill_on_drop(true)`, `Stdio::piped`/`null` set explicitly.
- Symlinks are rejected in three separate places: `validate_candidate` (`:546`,
  `symlink_metadata` + `is_symlink`), `inspect_raw_inputs` (`:565`,
  `follow_links(false)` + explicit reject), `canonical_real_directory` (`:648`).
- `validate_simple_project_id`, `validate_relative_path`, `validate_sha256` are
  all tight (`:660-701`).
- Bounded input: `MAX_REVISION_INPUT_FILES`.

### New API surface — authz is right

All six new routes carry `require_admin`, verified:
`list_fusion_reports`, `download_fusion_artifact`, `create_fusion_edit`,
`execute_fusion_revision`, `get_fusion_revision`, `cancel_fusion_revision`.
`download_fusion_artifact` additionally gates on
`Feature::AdvancedCaptureAutomation` and sets `no-store` + `nosniff`.

---

## 3. New issues introduced overnight

### 3.1 `fusion-artifact` route inherits the un-canonicalized path resolver

`#[get("/api/projects/{id}/fusion-artifact/{tail:.*}")]` → `is_allowed_fusion_artifact(&tail)`
(a suffix allowlist, `project.rs:2531`) → `resolve_project_child_file(...)`.

`fs_safety.rs:110 resolve_project_child_file` is **purely lexical** — it rejects
`..` components and checks `starts_with(base)`, but never canonicalizes and never
checks `symlink_metadata`. A symlink inside `output/` named `x_source_map.png`
pointing at any readable file passes the allowlist, passes the lexical check, and
is served.

The irony: `fusion_revision.rs` you wrote last night rejects symlinks in three
places. `fs_safety.rs` — the shared helper this new route depends on — still does
not. Make canonicalization uniform there, or reuse `validate_candidate`.

Admin-only and license-gated, so severity is bounded. But it is now a live file
read primitive rather than a theoretical one.

### 3.2 `--quality` is passed to the child process unvalidated

`fusion_revision.rs:159-160`:
```rust
.arg("--quality").arg(&prepared.replay.quality)
```
`quality` comes from a project-local replay profile. It is not shell injection —
but a value beginning with `-` is *argument* injection into clap. Validate it
against the known enum and reject anything starting with `-`. Same treatment for
any other string field that reaches `.arg()`.

### 3.3 Scratch growth is now material

`BAND_OUTPUT_ROWS = 512` and 112 bytes/pixel put admitted demosaic scratch at
**484,790,336 bytes** for one 45 MP frame. That reservation is taken from
`MemoryCreditPool` *before* decode, so it directly reduces how many sequences can
be in flight. Two things to check:
- `scratch_bytes()` still omits the host-side `normalized` clone and the retained
  input `bayer` (the GPU path borrows it; the CPU path consumes it). At 45 MP that
  is another ~360 MB not in the estimate that claims to be an exact upper bound.
- Pack `lab` as `vec2<u32>` instead of `vec4<i32>` and `candidates` as
  `vec4<f16>` instead of `vec4<f32>` — roughly halves 112 B/px, which raises the
  achievable band height *and* lowers the reservation.

---

## 4. If Metal AHD survives §0, make it actually fast

Current: 1.68x at 45 MP. Per band, `gpu_ahd.rs` does
`write_buffer` → `submit` → `map_async` → `poll(Maintain::Wait)` → memcpy → `unmap`.
That is a full CPU↔GPU sync point per band, 11 of them, nothing overlapping. Plus
three full host passes before any GPU work: `is_finite`/`>= 0` validation,
`fold(max)` for `range_scale`, and a full `Vec` clone for `normalized` — ~540 MB
of host traffic at 45 MP just to set up.

In order of value:
1. Hoist `AhdBuffers` into `GpuAhdEngine` — it is currently allocated **per
   `execute()` call**, i.e. once per image.
2. Double-buffer bands: submit band *n+1* before mapping band *n*.
3. Fold validation, max-reduction, and normalization into the first shader pass;
   pass `range_scale` as a uniform and drop the `normalized` clone.
4. Pack the scratch (§3.3).

Target ≥4x with *less* than 485 MB, or conclude the port is not worth a second
implementation of the algorithm.

---

## 5. The strategic question, restated

Two things have not changed and still deserve an explicit decision:

**AHD is from 2005.** It is the baseline modern demosaic methods are measured
against. RCD is cheaper *and* better on high-frequency detail; AMaZE is the
common quality reference; joint demosaic+denoise is where the field actually is.
Implementing RCD behind the same interface is a smaller change than the Metal
port and will move measured quality much further.

**`joint_demosaic.rs` says the current architecture is wrong.** Its module header
states that merging frames into a Bayer mosaic and demosaicing that is precisely
what not to do — and `main.rs:2661` does exactly that. The state of the art for
aligned bursts with sub-pixel offsets is multi-frame merge directly on the CFA
(Wronski et al., SIGGRAPH 2019), which uses real measurements where AHD
interpolates. The architecture for it is already in the tree.

Settle this before more GPU work. It determines whether the Metal AHD path has a
target at all.

Also still true, and larger than any of the above for image quality:
**`rgb_cam` is identity at the production call site** (`main.rs:2656`). AHD's
entire premise is that homogeneity is measured perceptually in CIELab. With an
identity camera matrix, the LUT, the `leps`/`abeps` thresholds, and all of
`build_homogeneity` operate in a fictitious color space. `FEATURE_MATRIX` already
lists the camera-profiled color pipeline as a release blocker — worth stating that
it is a prerequisite for the demosaic being *good*, not only for color accuracy.

---

## 6. Suggested order

1. **§0 — fixed-point Lab.** The parity gate is red at the resolution production
   mandates. Nothing else about Metal AHD matters until this is real. Do not
   widen the margin; do not resize bands.
2. **§0 — raise the qualification default to full sensor size** so the gate
   measures the workload.
3. P0.1, P0.2 — ~10 minutes, unblocks single-crate builds.
4. P0.3, P0.4, P0.5b — the server is not shippable until these land.
5. §1 — substantiate or revert `HOMOGENEITY_DECISION_MARGIN`.
6. §3.1 — make `fs_safety.rs` canonicalize, matching what `fusion_revision.rs`
   already does correctly.
7. §5 — settle `joint_demosaic` vs AHD-on-merged-Bayer; wire the real camera
   matrix.
8. §4 — only if Metal AHD survives step 7.

## Reproducing everything in this document

```bash
cargo check -p trueshot-core --examples                                    # fails
cargo run --release -p trueshot-core --features gpu \
  --example demosaic_metal_qualification -- 8256 5504 3                    # exit 1
cargo run --release -p trueshot-core --features gpu \
  --example demosaic_metal_qualification -- 1310 1304 7                    # passes
grep -n "require_metal_ahd_without_fallback\|expected_width" \
  docs/benchmarks/apple_nef_full_sensor_qualification_2026-07-27.json
```

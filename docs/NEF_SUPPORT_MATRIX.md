# Nikon NEF Support Matrix

Date: 2026-07-26

This matrix is the authoritative boundary for TrueShot's native NEF decoder.
Private validation media is never committed or published.

## Shipping

| Camera | Firmware | RAW layout | Compression | CFA | Host validated | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Nikon Z9 | 5.00 | 8280x5520 encoded RAW, 14-bit | Nikon lossless `34713` | RGGB | Apple Silicon, macOS 15 | TIFF identity/profile extraction; sensor levels 1008/15311 agree with RawSpeed/LibRaw; selective crops are pixel-exact against TrueShot full decode across the local corpus |

The shipping Z9 path supports:

- Sidecar-free forward selective entropy decode into caller-owned `u16` storage.
- One scaled embedded-preview detection and one Bayer-aligned crop plan per HDR/focus group.
- Full decode, detected ROI decode, reusable native group arenas, and in-memory native fusion.
- Metadata-driven black/saturation normalization with explicit expert overrides.

One private firmware 5.00 capture has a complete independent differential:
all 45,705,600 TrueShot samples matched dcraw's unscaled 16-bit document-mode
mosaic with zero mismatches and zero maximum absolute error.

Reproduce locally without publishing the capture:

```bash
dcraw -D -4 -j -t 0 -W -c /path/to/capture.nef > /private/tmp/reference.pgm
cargo run -p trueshot-core --example nef_roi_benchmark -- \
  /path/to/capture.nef --reference-pgm /private/tmp/reference.pgm
```

## Not Yet Advertised

All other Nikon bodies, firmware versions, bit depths, compression variants,
high-efficiency `CONTACT_INTOPIX` layouts, multi-strip variants, and unusual CFA
layouts remain unverified. TrueShot preserves their real TIFF make/model identity
but does not label them as Z9 or claim native selective-decode support.

Promotion to Shipping requires:

1. Legally usable captures covering firmware, bit depth, compression, and strip layout.
2. Randomized and boundary ROI equality against both TrueShot full decode and an independent RawSpeed/LibRaw reference.
3. Corruption, truncation, and allocation-bound tests.
4. Release-mode throughput and memory baselines on supported Apple Silicon hardware.

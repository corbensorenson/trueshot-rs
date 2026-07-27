# Sensor Noise Profiles

TrueShot's archival HDR estimator accepts measured photon-transfer calibration
through `trueshot.sensor-noise.v1` JSON artifacts:

```json
{
  "schema": "trueshot.sensor-noise.v1",
  "camera_make": "NIKON CORPORATION",
  "camera_model": "NIKON Z 9",
  "bits_per_sample": 14,
  "iso_models": [
    {
      "iso": 100,
      "model": {
        "read_noise_dn": [0.0, 0.0, 0.0, 0.0],
        "electrons_per_dn": [0.0, 0.0, 0.0, 0.0],
        "black_drift_dn": [0.0, 0.0, 0.0, 0.0],
        "saturation_margin_dn": 0.0,
        "calibrated": true
      }
    }
  ]
}
```

The zero values above are placeholders and intentionally fail validation.
TrueShot must not ship invented camera constants. A valid profile requires
retained dark/flat photon-transfer captures and positive measured values for
every CFA site and advertised ISO.

Use a profile with native burst processing:

```shell
trueshot process \
  --mode burst \
  --input /path/to/capture \
  --output /path/to/output \
  --sensor-noise-profile /path/to/z9-noise.json
```

Loading is bounded to 1 MiB. The schema, camera identity, bit depth, duplicate
ISO entries, and every numeric value are validated. Fusion requires an exact
ISO entry for every frame; it does not silently interpolate or substitute a
nearby ISO. The runtime calibration identity is the SHA-256 digest of the exact
profile artifact.

Without a profile, TrueShot uses a conservative compatibility model and marks
the resulting pixels with `FUSION_FLAG_UNCALIBRATED_NOISE`. Such output is not
eligible for claims based on calibrated uncertainty coverage.

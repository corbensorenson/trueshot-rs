# Tethered Capture Contract

TrueShot's tethered capture path is local-only. The camera adapter must return
the path of the actual downloaded camera file; a trigger acknowledgement or a
fabricated path is not a successful capture.

## Setting Contract

Requested ISO, shutter, aperture, white balance, and capture target values are
applied through camera-declared gPhoto controls. A request fails when:

- the control is absent or read-only;
- the requested value is not one of a radio control's declared choices;
- the camera rejects the configuration write; or
- immediate readback differs from the requested value.

TrueShot does not silently substitute a nearby exposure setting. Unsupported
PTZ, focus-point, autofocus, and manual-focus operations also fail explicitly
instead of reporting success.

## File Contract

After the shutter is triggered, the gPhoto adapter:

1. obtains the actual folder and filename reported by the camera;
2. sanitizes the basename so it cannot escape the capture directory;
3. downloads to a unique `.part` file;
4. rejects an empty download;
5. syncs file contents;
6. atomically renames the file to its final local name; and
7. returns that real path to the caller.

Failed downloads and pre-publication failures remove the partial file. The
camera-side source is not automatically deleted.

The default directory is the macOS local application-data directory under
`TrueShot/captures`. `TRUESHOT_CAPTURE_DIR` can select another user-owned local
directory.

## Qualification State

Pure path-safety and adapter integration tests run without camera hardware.
Release qualification still requires a supported Nikon body to verify:

- exact RAW/NEF bytes are downloaded for every capture target;
- ISO, shutter, aperture, and white-balance readback across supported values;
- disconnect, full-card, and interrupted-download recovery;
- sustained HDR/focus sequences without leaked `.part` files;
- captured EXIF focus/exposure metadata agrees with planner provenance; and
- measured USB throughput and end-to-end capture latency on macOS.

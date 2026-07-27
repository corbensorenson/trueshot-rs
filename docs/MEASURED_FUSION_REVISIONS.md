# Measured Fusion Revisions

TrueShot revisions correct source selection without turning an archival RAW
workflow into pixel painting. An operation selects a rectangle and one real
frame from the original HDR/focus group. Native fusion reruns that region from
the aligned same-CFA measurement and produces a new immutable result.

## Guarantees

- The base result and report are never overwritten.
- Every edit binds to the SHA-256 of the exact base report, capture-group ID,
  dimensions, crop origin, and frame count.
- Every output pixel has at most one operator-selected source.
- Selected samples must be available, aligned, uncensored, and not classified
  as disoccluded. One invalid pixel rejects the revision before publication.
- The selected source contributes its measured radiance, posterior uncertainty,
  sensor-correction provenance, and physical focus plane.
- No inpainting, generative reconstruction, cross-source interpolation, or
  invented highlight recovery is permitted.
- The edit digest deterministically names a separate revision group, journal
  entry, output filename, provenance report, and exact operator map.

## Authoring

Open **Fusion Inspector**, choose a current base report, and select
**Measured revision**. Draw a non-overlapping region, select the measured source
frame and reason, optionally add a bounded audit note, then save.

The local server validates the request and publishes
`trueshot.fusion.edits.v1` under:

```text
output/.trueshot/fusion_edits/<capture-group>_<edit-digest>.json
```

If project output encryption is enabled, only the authenticated `.enc` artifact
is published. The plaintext document is never written beside it. The Inspector
can download the validated JSON directly from its in-memory response when an
operator needs a clear CLI input; this is an explicit user export rather than a
plaintext project sibling.

## Refusion

Current base reports contain a strict `trueshot.fusion.replay.v1` capsule that
binds all processing controls and the SHA-256 of every project-local calibration
profile. After saving the edit, choose **Run measured revision**. The local
server queues the work, reconstructs arguments only from that capsule, sends the
exact base report and edit through a bounded stdin envelope, and launches only
the packaged sibling `trueshot` processor without a shell. Progress, bounded
failure diagnostics, and cancellation remain in the Inspector. On completion,
the Inspector selects the exact edit-digest-matched result and operator map.

The manual CLI route remains available. Run the same burst input and processing
configuration used for the base. Clear projects expose an exact absolute
argument. For encrypted projects, download the JSON from the Inspector and
provide that downloaded path:

```text
trueshot process --mode burst <existing options> --fusion-edits <revision.json>
```

TrueShot verifies the current base report hash before decoding the group. A
changed base, crop, frame order, or capture identity fails closed. Successful
output adds `_edit_<digest-prefix>` to the filename and emits
`*_fusion_edit_map.png`; value `255` marks every exact operator-selected pixel.

## Security Boundaries

- Project, report, edit, profile, and output paths are canonicalized and cannot
  escape through symbolic links.
- Encrypted report and edit artifacts are authenticated in bounded memory
  without plaintext project siblings.
- Encrypted RAW and digest-bound calibration profiles use authenticated
  fixed-offset `TSE2` reads. The processor receives the project key only through
  its anonymous stdin handoff and does not stage decrypted project files.
- Legacy `TSE1` remains available for bounded whole-artifact reads but fails
  closed for refusion until explicitly migrated to authenticated-seekable
  `TSE2`.
- Output-scope revision artifacts are encrypted immediately after the local
  processor closes them and remain covered by the background stability sweep.
- Manual glare and boundary controls remain unavailable until their edits can
  preserve physical visibility and measured-only archival constraints.

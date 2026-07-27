# Contributing to TrueShot

TrueShot is publicly inspectable source, but it is not an open-source project.
The current code is licensed under the
[PolyForm Noncommercial License 1.0.0](LICENSE).

## Reports and Discussions

Bug reports, security reports, reproducible test cases that do not contain
third-party code, and product feedback are welcome. Report security issues
privately using the process in [SECURITY.md](SECURITY.md).

## Code Contributions

External code contributions are not currently accepted. Do not submit source
code, patches, or pull requests unless Corben Sorenson has first provided a
written contributor agreement for that contribution.

This policy keeps copyright ownership and commercial licensing authority
unambiguous. Opening an issue or making the repository visible does not grant
commercial rights beyond those stated in [LICENSE](LICENSE).

## Local Development

For noncommercial inspection, research, experiment, and testing:

```bash
git clone https://github.com/corbensorenson/trueshot-rs.git
cd trueshot-rs
cargo build
```

Before reporting a regression, run the relevant checks described in
[README.md](README.md).

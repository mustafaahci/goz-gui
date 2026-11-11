# Building goz-gui

Back to the [README](../README.md). Testing and the test hooks are in [TESTING.md](TESTING.md).

## Requirements

- Windows 10 or 11, 64-bit.
- Rust 1.94 or newer with the MSVC toolchain (`x86_64-pc-windows-msvc`) and the Visual Studio Build Tools' C++ linker. [`rust-toolchain.toml`](../rust-toolchain.toml) selects the stable channel with rustfmt and clippy.
- Network access for the first build: Cargo fetches `goz-core` and `goz-winfs` from goz's `v0.7.0` tag on GitHub, so goz-gui speaks exactly the protocol gozd speaks and reads searches with goz's own parser.

No resource compiler is needed: [`build.rs`](../build.rs) embeds the manifest (common controls v6, per-monitor DPI v2, UTF-8), the icon from `assets/goz.ico` and the version information through the linker. [`.cargo/config.toml`](../.cargo/config.toml) links the C runtime statically, so `goz-gui.exe` runs without `vcruntime140.dll`.

## Build

```
cargo build --release
```

This produces `target\release\goz-gui.exe`. It needs gozd running; see [goz's README](https://github.com/mustafaahci/goz#install) for `gozd.exe install`.

The release profile uses fat LTO, one codegen unit, `panic = "abort"` and stripped symbols. For quicker optimized builds while testing, `cargo build --profile fast` (thin LTO, parallel codegen) writes `target\fast\goz-gui.exe`.

## The MSI installer

[`installer/goz-gui.wxs`](../installer/goz-gui.wxs) is a per-user WiX v5 package: it installs to `%LocalAppData%\Programs\goz-gui` with no administrator prompt, adds a Start menu shortcut and an entry in Settings → Apps, and upgrades an older goz-gui in place. To build it after `cargo build --release`, install WiX 5 and its UI extension once:

```
dotnet tool install --global wix --version 5.0.2
wix extension add -g WixToolset.UI.wixext/5.0.2
```

then run [`installer/build-msi.ps1`](../installer/build-msi.ps1) (Windows PowerShell 5.1 or PowerShell 7):

```
./installer/build-msi.ps1 -Validate
```

It takes the version from `Cargo.toml`, makes the license page from `LICENSE`, and writes `target\installer\goz-gui-v<version>-x64.msi`; `-Validate` also runs Windows Installer's ICE validation.

## Releases

Releases are made by [`.github/workflows/release.yml`](../.github/workflows/release.yml) when a version tag is pushed:

1. Set the version in `Cargo.toml` and move the `[Unreleased]` notes in [`CHANGELOG.md`](../CHANGELOG.md) under a section for it.
2. Tag and push: `git tag v0.1.0 && git push origin v0.1.0`.

The workflow checks that the tag matches `Cargo.toml`, builds with `--locked`, and publishes a GitHub Release with `goz-gui-vX.Y.Z-windows-x86_64.zip` (the exe, `LICENSE` and `README.md`), `goz-gui-vX.Y.Z-x64.msi` and `SHA256SUMS.txt`. The release text is that version's section of `CHANGELOG.md` followed by GitHub's generated notes; a tag with a `-` (`v0.2.0-rc.1`) is marked as a pre-release.

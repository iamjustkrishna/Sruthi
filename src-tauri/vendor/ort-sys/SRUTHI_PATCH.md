# Sruthi patch: ort-sys 2.0.0-rc.12

Upstream `build/dynamic_link.rs` located the Cargo profile directory with
`out_dir.ancestors().nth(3)`. That is correct for the older layout

```
target/<profile>/build/<crate>-<hash>/out
```

but newer Cargo versions use

```
target/<profile>/build/<crate>/<hash>/out
```

where the same expression resolves to `target/<profile>/build`. On Windows the
build script then copied `DirectML.dll` there instead of next to the executable,
and because `target/<profile>/build/examples` does not exist, a fresh target
directory made the build script panic.

The patch walks up to the directory named by Cargo's `PROFILE` variable instead,
and creates the profile, `examples`, and `deps` directories before copying. It
also keeps copying (rather than stopping after the first directory) when Windows
symlinks are unavailable, so both the app and its test binaries can load the
runtime DLLs.

No API or runtime behaviour of the crate is changed.

## Windows installer resource

Tauri's NSIS bundler only ships the executable plus configured resources; it
does not scan the profile directory for DLLs the way the MSI bundler does. A
cached build can restore the compiled artifacts without the profile-root
`DirectML.dll`, and `tauri-build` validates `bundle.resources` while compiling
the app, before a cached `ort-sys` build script could rewrite the file.

`src-tauri/DirectML.dll` is therefore checked in as the Windows-only NSIS
resource. It is the `DirectML.dll` from the version-pinned ONNX Runtime archive
`ms@1.24.2/x86_64-pc-windows-msvc.tar.lzma2` (archive SHA256 `b685bfc8...`,
DLL SHA256 `9c9e6d82...e92da1`). The MSI bundler deduplicates this resource
against its automatic DLL scan, so both installers contain exactly one copy.
Refresh the file when the ONNX Runtime version in `build/download/dist.txt`
changes.

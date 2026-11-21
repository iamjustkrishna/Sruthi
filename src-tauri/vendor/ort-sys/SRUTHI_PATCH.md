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

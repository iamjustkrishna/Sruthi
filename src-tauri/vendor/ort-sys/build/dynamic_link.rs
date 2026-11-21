#[cfg(feature = "copy-dylibs")]
use std::path::Path;

use crate::vars;

pub fn prefer_dynamic_linking() -> bool {
	match vars::get(vars::PREFER_DYNAMIC_LINK) {
		Some(val) => val == "1" || val.to_lowercase() == "true",
		None => false
	}
}

// Sruthi patch: Cargo's OUT_DIR layout is either
//   <target>/<profile>/build/<crate>-<hash>/out
// or, on newer Cargo versions,
//   <target>/<profile>/build/<crate>/<hash>/out
// The fixed `ancestors().nth(3)` from upstream resolves to `<target>/<profile>` on
// the first layout but to `<target>/<profile>/build` on the second, which puts the
// runtime DLLs somewhere the linker, the test binaries, and the installer cannot
// see them. Walk to the profile directory instead.
#[cfg(feature = "copy-dylibs")]
pub fn copy_dylibs(lib_dir: &Path, out_dir: &Path) {
	use std::fs;

	let profile = std::env::var("PROFILE").unwrap_or_else(|_| String::from("release"));
	let profile_dir = out_dir
		.ancestors()
		.find(|path| path.file_name().is_some_and(|name| name == profile.as_str()))
		.or_else(|| out_dir.ancestors().nth(3))
		.unwrap_or(out_dir);

	#[cfg(windows)]
	let mut copy_fallback = false;
	#[cfg(not(windows))]
	let copy_fallback = false;

	// Place the DLLs next to the executable so the app and its test binaries can
	// load them. `deps` is where Cargo puts the test executables.
	for bin_dir in [profile_dir.to_path_buf(), profile_dir.join("examples"), profile_dir.join("deps")] {
		fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("failed to create `{}`: {e}", bin_dir.display()));

		let lib_files = fs::read_dir(lib_dir).unwrap_or_else(|_| panic!("Failed to read contents of `{}` (does it exist?)", lib_dir.display()));
		for lib_file in lib_files.filter(|e| {
			e.as_ref().ok().is_some_and(|e| {
				e.file_type().is_ok_and(|e| !e.is_dir()) && [".dll", ".so", ".dylib"].into_iter().any(|v| e.path().to_string_lossy().contains(v))
			})
		}) {
			let lib_file = lib_file.unwrap();
			let lib_path = lib_file.path();
			let lib_name = lib_path.file_name().unwrap();
			let out_path = bin_dir.join(lib_name);
			if out_path.is_symlink() {
				fs::remove_file(&out_path).unwrap();
			}
			if !out_path.exists() {
				#[cfg(windows)]
				if std::os::windows::fs::symlink_file(&lib_path, &out_path).is_err() {
					copy_fallback = true;
					fs::copy(&lib_path, &out_path).unwrap();
				}
				#[cfg(unix)]
				std::os::unix::fs::symlink(&lib_path, &out_path).unwrap();
			}
			if !copy_fallback {
				println!("cargo:rerun-if-changed={}", out_path.to_str().unwrap());
			}
		}
	}

	if copy_fallback {
		crate::log::warning!("had to copy dylibs because Windows Developer Mode is not enabled, or the cache dir is on a different drive");
	}
}

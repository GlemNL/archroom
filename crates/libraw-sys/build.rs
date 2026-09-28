// Build scripts have no better error-reporting path than panicking with a
// clear message; that's the standard idiom, not a lapse.
#![allow(clippy::expect_used)]

use std::env;
use std::path::PathBuf;

fn main() {
    let lib = pkg_config::Config::new()
        .atleast_version("0.20")
        .probe("libraw")
        .expect("system libraw not found (pkg-config libraw); install the `libraw` package");

    let mut builder = bindgen::Builder::default()
        .header_contents("wrapper.h", "#include <libraw/libraw.h>\n")
        .allowlist_function("libraw_.*")
        .allowlist_type("libraw_.*")
        .allowlist_var("LIBRAW_.*")
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: false,
        })
        .derive_default(true)
        .generate_comments(false);

    for path in &lib.include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }

    let bindings = builder
        .generate()
        .expect("failed to generate libraw bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("failed to write libraw bindings");
}

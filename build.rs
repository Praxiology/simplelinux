//! Build script for the runner crate: compiles the kernel (via the `kernel`
//! artifact dependency) and wraps the resulting ELF into a bootable BIOS image.
use std::path::PathBuf;

/// The `CARGO_BIN_FILE_*` env var name depends on the dependency alias and the
/// binary name; resolve it robustly instead of hard-coding a single spelling.
fn kernel_bin_path() -> PathBuf {
    const CANDIDATES: &[&str] = &[
        "CARGO_BIN_FILE_KERNEL_kernel",
        "CARGO_BIN_FILE_KERNEL_simplelinux_kernel",
        "CARGO_BIN_FILE_KERNEL",
    ];
    for key in CANDIDATES {
        if let Some(v) = std::env::var_os(key) {
            return PathBuf::from(v);
        }
    }
    // Last resort: find any CARGO_BIN_FILE_KERNEL_* variable.
    for (k, v) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("CARGO_BIN_FILE_KERNEL") {
            return PathBuf::from(v);
        }
    }
    panic!("could not locate the built kernel ELF via CARGO_BIN_FILE_KERNEL* env vars");
}

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let kernel = kernel_bin_path();

    let bios_path = out_dir.join("bios.img");
    bootloader::BiosBoot::new(&kernel)
        .create_disk_image(&bios_path)
        .unwrap_or_else(|e| panic!("failed to create BIOS disk image: {e}"));

    // Expose the image path to `src/main.rs` via a compile-time env var.
    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
    println!("cargo:rerun-if-changed={}", kernel.display());
}

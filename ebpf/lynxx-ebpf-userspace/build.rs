//! Build script for the lynxx-ebpf userspace daemon.
//!
//! Behaviour:
//!
//! * If the `embed-bpf-object` cargo feature is NOT enabled, this
//!   script is effectively a no-op — it writes an empty placeholder
//!   `lynxx-ebpf-program.o` into `OUT_DIR` so `include_bytes_aligned!`
//!   compiles. This is the path used by `cargo check` on macOS, where
//!   we have no Linux kernel to load against anyway.
//!
//! * If the feature IS enabled, we attempt to invoke
//!   `cargo build --release --target=bpfel-unknown-none -p lynxx-ebpf-program`
//!   from the parent workspace. We require `bpf-linker` to be on PATH
//!   and emit `cargo:warning=` with install instructions if it isn't.
//!
//! We do NOT invoke `cargo` from within `cargo` recursively when the
//! feature is off, to keep `cargo check` fast and side-effect-free.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../lynxx-ebpf-program/src/main.rs");
    println!("cargo:rerun-if-changed=../lynxx-ebpf-program/Cargo.toml");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set by cargo"));
    let dest = out_dir.join("lynxx-ebpf-program.o");

    let embed = env::var("CARGO_FEATURE_EMBED_BPF_OBJECT").is_ok();

    if !embed {
        // Write a minimal placeholder so `include_bytes_aligned!`
        // compiles. The userspace loader checks for this sentinel and
        // refuses to start at runtime with a friendly error.
        let placeholder = b"LYNXX_EBPF_PLACEHOLDER_NOT_A_REAL_BPF_OBJECT";
        fs::write(&dest, placeholder).expect("failed to write placeholder BPF object");
        println!(
            "cargo:warning=lynxx-ebpf: built without `embed-bpf-object` feature; the resulting binary cannot load any BPF program. Rebuild with `--features embed-bpf-object` on Linux."
        );
        return;
    }

    // Verify bpf-linker is installed before attempting to invoke
    // cargo recursively — its error message is otherwise inscrutable.
    if which::which("bpf-linker").is_err() {
        panic!(
            "bpf-linker not found on PATH. Install it with:\n\
             \n    cargo install bpf-linker\n\n\
             See ebpf/README.md for full toolchain requirements."
        );
    }

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let workspace_root = manifest_dir
        .parent()
        .expect("userspace crate must live inside ebpf/ workspace")
        .to_path_buf();

    let status = Command::new("cargo")
        .current_dir(&workspace_root)
        .args([
            "build",
            "--release",
            "--package",
            "lynxx-ebpf-program",
            "--target",
            "bpfel-unknown-none",
            "-Z",
            "build-std=core",
        ])
        .env("RUSTFLAGS", "-C debuginfo=2")
        .status()
        .expect("failed to spawn cargo for lynxx-ebpf-program");

    if !status.success() {
        panic!(
            "cargo build of lynxx-ebpf-program failed (status={status}). \
             Check that you are using the pinned nightly toolchain in \
             ebpf/rust-toolchain.toml and that bpf-linker is installed."
        );
    }

    let built = workspace_root
        .join("target")
        .join("bpfel-unknown-none")
        .join("release")
        .join("lynxx-ebpf-program");

    if !built.exists() {
        panic!(
            "expected BPF object at {} but it does not exist",
            built.display()
        );
    }

    fs::copy(&built, &dest).expect("failed to copy BPF object into OUT_DIR");
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

fn main() {
    tauri_build::build();
    // Tauri embeds its manifest in the app binary, but independent examples
    // also link dialog code that imports TaskDialogIndirect (common-controls v6).
    if std::env::var("TARGET").is_ok_and(|target| target.ends_with("windows-msvc")) {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-e2e.manifest");
        println!("cargo:rerun-if-changed=windows-e2e.manifest");
        println!("cargo:rustc-link-arg-examples=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-examples=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}

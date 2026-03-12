// SPDX-License-Identifier: AGPL-3.0-or-later

//! Build script for kyomi-embed: downloads BGE-small-en-v1.5 ONNX model files
//! from HuggingFace at compile time so they can be embedded via `include_bytes!()`.

use std::path::Path;
use std::process::Command;

const BASE_URL: &str = "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main";

/// Files to download: (remote_path, local_filename)
const MODEL_FILES: &[(&str, &str)] = &[
    ("onnx/model.onnx", "model.onnx"),
    ("tokenizer.json", "tokenizer.json"),
    ("config.json", "config.json"),
    ("special_tokens_map.json", "special_tokens_map.json"),
    ("tokenizer_config.json", "tokenizer_config.json"),
];

fn main() {
    // Only re-run if build.rs itself changes — model files don't change
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");

    for &(remote_path, local_name) in MODEL_FILES {
        let dest = Path::new(&out_dir).join(local_name);

        // Skip if already cached from a previous incremental build
        if dest.exists() {
            println!("cargo:warning=Model file already cached: {local_name}");
            continue;
        }

        let url = format!("{BASE_URL}/{remote_path}");
        println!("cargo:warning=Downloading {local_name} from {url}");

        let status = Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--output",
                dest.to_str().expect("invalid path"),
                &url,
            ])
            .status()
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to run curl for {local_name}: {e}. \
                     Install curl and retry."
                )
            });

        if !status.success() {
            panic!(
                "Failed to download {local_name} from {url} (exit code: {})",
                status.code().unwrap_or(-1)
            );
        }

        println!("cargo:warning=Downloaded {local_name} successfully");
    }
}

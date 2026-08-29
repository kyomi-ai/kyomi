// SPDX-License-Identifier: AGPL-3.0-or-later

//! Build script for kyomi-embed: downloads BGE-small-en-v1.5 model files
//! from HuggingFace at compile time so they can be embedded via `include_bytes!()`.
//!
//! Uses safetensors weights (for Candle pure-Rust inference) + tokenizer files.

use std::path::Path;
use std::process::Command;

// `curl_args` and its retry/timeout constants live in `build_support.rs`,
// included here (rather than defined in this file) so the exact same
// source is also compiled into `src/lib.rs`'s `#[cfg(test)]` module and
// actually exercised by `cargo test` — see that file's module doc for why.
include!("build_support.rs");

const DEFAULT_BASE_URL: &str = "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main";

fn base_url() -> String {
    std::env::var("KYOMI_MODEL_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into())
}

/// Files to download: (remote_path, local_filename).
///
/// HuggingFace also serves `special_tokens_map.json` and
/// `tokenizer_config.json` for this model, but they are not downloaded
/// here: `kyomi_embed::EmbeddingService` loads the tokenizer with
/// `Tokenizer::from_bytes` against `tokenizer.json` alone (the Rust
/// `tokenizers` crate's self-contained "fast tokenizer" format). Those two
/// extra files are only consulted by Python's `transformers`
/// `AutoTokenizer.from_pretrained`, which this crate does not use. Fetching
/// them bought nothing but two more network round trips, and two more
/// chances for a build to fail on a file nothing reads (KYO-510).
const MODEL_FILES: &[(&str, &str)] = &[
    ("model.safetensors", "model.safetensors"),
    ("tokenizer.json", "tokenizer.json"),
    ("config.json", "config.json"),
];

fn main() {
    // Only re-run if build.rs itself changes — model files don't change
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support.rs");
    println!("cargo:rerun-if-env-changed=KYOMI_MODEL_BASE_URL");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let base = base_url();

    for &(remote_path, local_name) in MODEL_FILES {
        let dest = Path::new(&out_dir).join(local_name);

        // Skip if already cached from a previous incremental build
        if dest.exists() {
            println!("cargo:warning=Model file already cached: {local_name}");
            continue;
        }

        let url = format!("{base}/{remote_path}");
        let dest_str = dest.to_str().expect("invalid path");
        println!("cargo:warning=Downloading {local_name} from {url}");

        let status = Command::new("curl")
            .args(curl_args(&url, dest_str))
            .status()
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to run curl for {local_name}: {e}. \
                     Install curl and retry."
                )
            });

        if !status.success() {
            panic!(
                "Failed to download {local_name} from {url} after \
                 {CURL_RETRY_COUNT} retries (exit code: {}). This is a \
                 network problem talking to HuggingFace, not a code \
                 problem — re-run the job, or set KYOMI_MODEL_BASE_URL to \
                 a mirror if it keeps failing.",
                status.code().unwrap_or(-1)
            );
        }

        println!("cargo:warning=Downloaded {local_name} successfully");
    }
}

//! Names the code that decides how a change reads, so what was worked out with it can be kept
//! until that code changes, and not lost whenever the pages or the server around it do.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// The server, its pages, and how journeys group what was read, which is applied afterwards.
/// Everything else in src/ can change what a commit reads as.
const AROUND_THE_READER: &[&str] = &["auth.rs", "journeys.rs", "main.rs", "pulls.rs", "reports.rs", "runs.rs", "serve.html", "serve.rs", "soba.rs"];

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.lock");
    let mut files: Vec<_> = std::fs::read_dir("src").unwrap().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.retain(|p| !AROUND_THE_READER.contains(&p.file_name().unwrap().to_str().unwrap()));
    files.push("Cargo.lock".into());
    files.sort();
    let mut h = DefaultHasher::new();
    for f in files {
        std::fs::read(f).unwrap().hash(&mut h);
    }
    println!("cargo:rustc-env=FLOW_READER={:x}", h.finish());
}

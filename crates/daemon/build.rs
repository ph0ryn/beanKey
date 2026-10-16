use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(not(test))]
fn main() {
    // Only materialized build inputs: no Git checkout, Nix metadata, or absolute
    // source paths are needed for the reproducible (non-cryptographic) identity.
    let inputs = [
        "Cargo.toml",
        "Cargo.lock",
        "crates/converter/Cargo.toml",
        "crates/converter/build.rs",
        "crates/converter/src",
        "crates/daemon/Cargo.toml",
        "crates/daemon/build.rs",
        "crates/daemon/src",
        "crates/llama/Cargo.toml",
        "crates/llama/build.rs",
        "crates/llama/src",
        "proto",
    ];
    for input in inputs {
        // Watching directories also detects additions and removals.
        println!("cargo:rerun-if-changed=../../{input}");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fingerprint = source_fingerprint(&root, &inputs)
        .expect("beanKey source fingerprint could not read its build inputs");
    println!("cargo:rustc-env=BEANKEY_INTERNAL_SOURCE_FINGERPRINT={fingerprint}");
    for name in ["TARGET", "PROFILE"] {
        let value = std::env::var(name).expect("Cargo build identity is unavailable");
        println!("cargo:rustc-env=BEANKEY_INTERNAL_{name}={value}");
    }
    let compiler =
        std::process::Command::new(std::env::var_os("RUSTC").expect("Cargo RUSTC missing"))
            .arg("--version")
            .output()
            .expect("beanKey compiler identity could not be read");
    assert!(
        compiler.status.success(),
        "beanKey compiler identity failed"
    );
    let compiler = String::from_utf8(compiler.stdout).expect("rustc version is not UTF-8");
    println!("cargo:rustc-env=BEANKEY_INTERNAL_RUSTC={}", compiler.trim());

    println!("cargo:rerun-if-changed=../../proto/beankey.proto");
    prost_build::compile_protos(&["../../proto/beankey.proto"], &["../../proto"])
        .expect("beanKey protocol generation failed");
}

/// Stable FNV-1a over sorted, length-framed relative names and contents.
/// This is source-change provenance, not a cryptographic integrity guarantee.
pub(crate) fn source_fingerprint(root: &Path, inputs: &[&str]) -> io::Result<String> {
    let mut files = Vec::new();
    for input in inputs {
        collect_files(&root.join(input), &mut files)?;
    }
    let mut named = files
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(root).expect("input is under source root");
            let name = relative
                .components()
                .map(|component| component.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 source name"))?
                .join("/");
            Ok((name, path))
        })
        .collect::<io::Result<Vec<_>>>()?;
    named.sort_by(|left, right| left.0.cmp(&right.0));
    named.dedup_by(|left, right| left.0 == right.0);

    let mut hash = 0xcbf29ce484222325_u64;
    let mut append = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    };
    append(b"beanKey-source-fingerprint-v1");
    for (name, path) in named {
        append(name.as_bytes());
        append(&fs::read(path)?);
    }
    Ok(format!("fnv1a64-v1:{hash:016x}"))
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    if fs::metadata(path)?.is_dir() {
        for entry in fs::read_dir(path)? {
            collect_files(&entry?.path(), files)?;
        }
    } else {
        files.push(path.to_path_buf());
    }
    Ok(())
}

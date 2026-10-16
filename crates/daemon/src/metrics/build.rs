use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BuildInfo {
    pub version: String,
    pub algorithm_version: u32,
    pub source_fingerprint: String,
    pub target: String,
    pub profile: String,
    pub rustc: String,
    pub executable_store_id: Option<String>,
}

impl BuildInfo {
    pub(crate) fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            algorithm_version: 1,
            source_fingerprint: env!("BEANKEY_INTERNAL_SOURCE_FINGERPRINT").into(),
            target: env!("BEANKEY_INTERNAL_TARGET").into(),
            profile: env!("BEANKEY_INTERNAL_PROFILE").into(),
            rustc: env!("BEANKEY_INTERNAL_RUSTC").into(),
            executable_store_id: std::env::current_exe()
                .ok()
                .and_then(|path| store_id(&path)),
        }
    }
}

/// Extracts a lexical Nix store object ID, never its name or an input path.
/// `None` means unknown; this does not hash a file or verify its contents/revision.
pub(crate) fn store_id(path: &Path) -> Option<String> {
    let suffix = path.to_str()?.strip_prefix("/nix/store/")?;
    let suffix = suffix.strip_suffix('/').unwrap_or(suffix);
    let mut parts = suffix.split('/');
    let (hash, name) = parts.next()?.split_once('-')?;
    if hash.len() != 32
        || !hash
            .bytes()
            .all(|byte| b"0123456789abcdfghijklmnpqrsvwxyz".contains(&byte))
        || name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+-._?=".contains(&byte))
        || parts.any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(hash.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{BuildInfo, store_id};
    use std::{fs, path::Path};
    use tempfile::TempDir;

    mod generator {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"));
    }

    const HASH: &str = "0123456789abcdfghijklmnpqrsvwxyz";

    fn fixture(root: &Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), "version = '0.4.5'\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    }

    #[test]
    fn source_fingerprint_is_reproducible_without_checkout_paths() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        fixture(first.path());
        fixture(second.path());
        let fingerprint =
            generator::source_fingerprint(first.path(), &["Cargo.toml", "src"]).unwrap();
        assert_eq!(
            fingerprint,
            generator::source_fingerprint(first.path(), &["src", "Cargo.toml"]).unwrap()
        );
        assert_eq!(
            fingerprint,
            generator::source_fingerprint(second.path(), &["Cargo.toml", "src"]).unwrap()
        );
        let digest = fingerprint.strip_prefix("fnv1a64-v1:").unwrap();
        assert_eq!(digest.len(), 16);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!fingerprint.contains(first.path().to_str().unwrap()));
        assert!(!fingerprint.contains("main.rs"));
    }

    #[test]
    fn source_fingerprint_changes_with_content_names_and_new_inputs() {
        let directory = TempDir::new().unwrap();
        fixture(directory.path());
        let fingerprint =
            || generator::source_fingerprint(directory.path(), &["Cargo.toml", "src"]).unwrap();
        let original = fingerprint();
        fs::write(
            directory.path().join("src/main.rs"),
            "fn main() { exit(); }\n",
        )
        .unwrap();
        let changed = fingerprint();
        assert_ne!(original, changed);
        fs::write(directory.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        assert_eq!(original, fingerprint());
        fs::write(directory.path().join("src/new.rs"), "").unwrap();
        assert_ne!(original, fingerprint());
        assert!(generator::source_fingerprint(directory.path(), &["missing.rs"]).is_err());
        assert_ne!(
            generator::source_fingerprint(directory.path(), &["src/main.rs"]).unwrap(),
            {
                fs::write(directory.path().join("src/renamed.rs"), "fn main() {}\n").unwrap();
                generator::source_fingerprint(directory.path(), &["src/renamed.rs"]).unwrap()
            }
        );
    }

    #[test]
    fn store_identity_returns_only_a_strict_nix_hash() {
        for suffix in ["", "/", "/bin/private-daemon", "/share/private-model.gguf"] {
            let path = format!("/nix/store/{HASH}-private-package{suffix}");
            assert_eq!(store_id(Path::new(&path)).as_deref(), Some(HASH));
        }
        for path in [
            format!("relative/nix/store/{HASH}-private-package/model.gguf"),
            format!("/home/private-user/nix/store/{HASH}-private-package/model.gguf"),
            format!("/nix/storex/{HASH}-private-package/model.gguf"),
            format!("/nix/store/{HASH}-private-package/../private-model.gguf"),
            format!("/nix/store/{HASH}-private-package/./model.gguf"),
            format!("/nix/store/{HASH}-private-package//model.gguf"),
            format!("/nix/store/{HASH}-"),
            format!("/nix/store/{HASH}-private package/model.gguf"),
            format!("/nix/store/{HASH}0-private-package/model.gguf"),
            "/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-private-package/model.gguf".into(),
            "/nix/store/0123-private-package/model.gguf".into(),
        ] {
            assert_eq!(store_id(Path::new(&path)), None);
        }
    }

    #[test]
    fn serialized_build_info_contains_identity_but_no_checkout_paths() {
        let info = BuildInfo::current();
        let value = serde_json::to_value(&info).unwrap();
        let serialized = value.to_string();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(info.algorithm_version > 0);
        assert!(info.source_fingerprint.starts_with("fnv1a64-v1:"));
        assert!(!info.target.is_empty());
        assert!(!info.profile.is_empty());
        assert!(info.rustc.starts_with("rustc "));
        assert!(!serialized.contains(env!("CARGO_MANIFEST_DIR")));
        assert!(
            info.executable_store_id
                .as_ref()
                .is_none_or(|id| { id.len() == 32 && !id.contains('/') && !id.contains('-') })
        );
        let decoded: BuildInfo = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.source_fingerprint, info.source_fingerprint);
    }
}

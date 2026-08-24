use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Result},
    path::{Path, PathBuf},
};

fn main() {
    if (std::env::var_os("CARGO_FEATURE_WEBDRIVER").is_some()
        || std::env::var_os("CARGO_FEATURE_E2E").is_some())
        && std::env::var("DEBUG").as_deref() != Ok("true")
    {
        panic!("WebDriver is forbidden in release builds");
    }
    embed_pi_runtime_tree_digest();
    embed_marketplace_bundle();
    tauri_build::build()
}

fn embed_marketplace_bundle() {
    let root = PathBuf::from("../marketplace");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect_marketplace_files(&root, &root, &mut files).unwrap_or_else(|error| {
        panic!("bundled Marketplace tree is unavailable: {error}");
    });
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut json = String::from("{\"files\":{");
    for (index, (path, contents)) in files.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push('"');
        json.push_str(&json_escape(path));
        json.push_str("\":\"");
        json.push_str(&json_escape(contents));
        json.push('"');
    }
    json.push_str("}}");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("marketplace_bundle.json");
    fs::write(out, json).expect("write Marketplace bundle");
}

fn collect_marketplace_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if name == "kis-read-api-v1.json" {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Marketplace contains a symlink",
            ));
        }
        if metadata.is_dir() {
            collect_marketplace_files(root, &path, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Marketplace contains an unsupported entry",
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| std::io::ErrorKind::InvalidData)?
            .to_str()
            .ok_or(std::io::ErrorKind::InvalidData)?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if !relative.ends_with(".json") {
            continue;
        }
        files.push((relative, fs::read_to_string(path)?));
    }
    Ok(())
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

fn embed_pi_runtime_tree_digest() {
    let root = PathBuf::from("resources/pi-runtime");
    println!("cargo:rerun-if-changed={}", root.display());
    match runtime_tree_digest(&root) {
        Ok(digest) => println!("cargo:rustc-env=GURUTERMINAL_PI_RUNTIME_TREE_SHA256={digest}"),
        Err(error) if std::env::var("DEBUG").as_deref() == Ok("true") => {
            println!("cargo:warning=Pi runtime tree digest unavailable: {error}");
            println!("cargo:rustc-env=GURUTERMINAL_PI_RUNTIME_TREE_SHA256=unavailable");
        }
        Err(error) => panic!("release Pi runtime tree is unavailable: {error}"),
    }
}

fn runtime_tree_digest(root: &Path) -> Result<String> {
    let mut records = Vec::new();
    collect_runtime_records(root, root, &mut records)?;
    records.sort_by(|left, right| left.0.cmp(&right.0));
    let mut tree = Sha256::new();
    for (relative, kind, size, digest) in records {
        tree.update([kind]);
        tree.update((relative.len() as u64).to_be_bytes());
        tree.update(relative.as_bytes());
        tree.update(size.to_be_bytes());
        tree.update(digest);
    }
    Ok(hex_digest(tree.finalize().as_slice()))
}

fn collect_runtime_records(
    root: &Path,
    directory: &Path,
    records: &mut Vec<(String, u8, u64, [u8; 32])>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Pi runtime contains a symlink",
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| std::io::ErrorKind::InvalidData)?
            .to_str()
            .ok_or(std::io::ErrorKind::InvalidData)?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if metadata.is_dir() {
            records.push((relative, b'd', 0, [0; 32]));
            collect_runtime_records(root, &path, records)?;
        } else if metadata.is_file() {
            let mut file = fs::File::open(&path)?;
            let mut digest = Sha256::new();
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
            records.push((relative, b'f', metadata.len(), digest.finalize().into()));
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Pi runtime contains an unsupported entry",
            ));
        }
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 0x0f) as usize] as char);
    }
    value
}

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn git(root: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).current_dir(root).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=src");
    let mut paths = vec![PathBuf::from("Cargo.toml"), PathBuf::from("Cargo.lock"), PathBuf::from("build.rs")];
    paths.extend(fs::read_dir(root.join("src")).unwrap().map(|entry| {
        PathBuf::from("src").join(entry.unwrap().file_name())
    }).filter(|path| path.extension().and_then(|s| s.to_str()) == Some("rs")));
    paths.sort();
    let mut source = Vec::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        source.extend_from_slice(path.to_string_lossy().as_bytes());
        source.push(0);
        source.extend_from_slice(&fs::read(root.join(path)).unwrap());
        source.push(0);
    }
    let mut revision = git(&root, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    if git(&root, &["diff", "--quiet", "HEAD"]).is_none() { revision.push_str("-dirty"); }
    if let Some(directory) = git(&root, &["rev-parse", "--absolute-git-dir"]) {
        for path in ["HEAD".to_string(), "index".to_string(), "packed-refs".to_string(),
            git(&root, &["symbolic-ref", "-q", "HEAD"]).unwrap_or_default()] {
            let file = Path::new(&directory).join(path);
            if file.is_file() { println!("cargo:rerun-if-changed={}", file.display()); }
        }
    }
    let fingerprint = (|| -> Option<String> {
        let mut child = Command::new("git").args(["hash-object", "--stdin"]).current_dir(&root)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        child.stdin.take()?.write_all(&source).ok()?;
        let output = child.wait_with_output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    })().unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=REC_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=REC_BUILD_SOURCE={fingerprint}");
}

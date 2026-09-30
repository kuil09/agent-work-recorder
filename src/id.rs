use anyhow::{bail, Result};
use std::fs;
use std::path::Path;

pub fn generate_run_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = DefaultHasher::new();
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    let n = hasher.finish();
    format!("{:04X}", (n & 0xFFFF) as u16)
}

pub fn format_step_id(run_id: &str, step: u32) -> String {
    format!("{run_id}:{step:03}")
}

pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.chars().count() > 40 {
        out.chars()
            .take(40)
            .collect::<String>()
            .trim_end_matches('-')
            .to_string()
    } else {
        out
    }
}

pub fn output_filename(run_id: &str, title: &Option<String>) -> String {
    match title {
        Some(t) => {
            let slug = slugify(t);
            if slug.is_empty() {
                format!("{run_id}.mp4")
            } else {
                format!("{run_id}-{slug}.mp4")
            }
        }
        None => format!("{run_id}.mp4"),
    }
}

pub fn ensure_unique_run_id(existing_dir: &Path) -> Result<String> {
    for _ in 0..16 {
        let id = generate_run_id();
        if !existing_dir.join(&id).exists() {
            return Ok(id);
        }
    }
    bail!("could not allocate a unique run id")
}

pub fn write_json_atomic(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_is_4_hex() {
        let id = generate_run_id();
        assert_eq!(id.len(), 4);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(id.chars().all(|c| !c.is_ascii_lowercase()));
    }

    #[test]
    fn step_id_pads() {
        assert_eq!(format_step_id("7F32", 1), "7F32:001");
        assert_eq!(format_step_id("7F32", 21), "7F32:021");
    }

    #[test]
    fn slug_korean_and_ascii() {
        assert_eq!(slugify("로그인 오류 UI 수정"), "로그인-오류-ui-수정");
        assert_eq!(slugify("Checkout validation"), "checkout-validation");
        assert_eq!(slugify("  --  "), "");
    }

    #[test]
    fn output_name_with_and_without_title() {
        assert_eq!(output_filename("7F32", &None), "7F32.mp4");
        assert_eq!(
            output_filename("7F32", &Some("Login error UI".into())),
            "7F32-login-error-ui.mp4"
        );
    }
}

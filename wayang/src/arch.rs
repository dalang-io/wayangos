//! Host architecture → channel arch token (`x86_64`, `arm64`).

pub fn host_arch() -> &'static str {
    // Testing / cross-staging override.
    if let Ok(v) = std::env::var("WAYANG_ARCH") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Box::leak(v.into_boxed_str());
        }
    }
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known() {
        assert_eq!(host_arch(), match std::env::consts::ARCH {
            "x86_64" => "x86_64",
            "aarch64" => "arm64",
            other => other,
        });
    }
}

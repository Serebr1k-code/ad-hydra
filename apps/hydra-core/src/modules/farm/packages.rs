/// Registry package names, optionally pinned to an exact version. Never uv options,
/// local paths, URLs or requirement-file directives.
pub fn validate(packages: &[String]) -> anyhow::Result<()> {
    anyhow::ensure!(
        packages.len() <= 32,
        "at most 32 Python dependencies allowed"
    );
    let mut names = std::collections::HashSet::new();
    for package in packages {
        let (name, version) = package
            .split_once("==")
            .map_or((package.as_str(), None), |(n, v)| (n, Some(v)));
        anyhow::ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name.as_bytes()[0].is_ascii_alphanumeric()
                && name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)),
            "dependency must be a package name or name==version"
        );
        if let Some(version) = version {
            anyhow::ensure!(
                !version.is_empty()
                    && version.len() <= 48
                    && version.as_bytes()[0].is_ascii_digit()
                    && version
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._+!".contains(&c)),
                "invalid dependency version"
            );
        }
        let normalized = name.to_ascii_lowercase().replace(['_', '.'], "-");
        anyhow::ensure!(names.insert(normalized), "duplicate dependency");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn registry_names_and_pins_only() {
        assert!(super::validate(&["requests".into(), "pwntools==4.15.0".into()]).is_ok());
        for value in [
            "--index-url=x",
            "/tmp/code",
            "https://example.org/x",
            "a>=1",
            "a==",
            "a==1\n--option",
            "a @ URL",
        ] {
            assert!(super::validate(&[value.into()]).is_err(), "{value}");
        }
    }
}

//! Trusted, bounded wheel provisioning. Uploads are never mounted in this phase.
use super::{Config, Job, Result, RuntimeDirectory, SandboxError, cgroup};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub fn validate_python_packages(packages: &[String]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    if packages.len() > 32 {
        return Err(SandboxError::Invalid(
            "at most 32 Python package pins allowed".into(),
        ));
    }
    for package in packages {
        let Some((name, version)) = package.split_once("==") else {
            return Err(SandboxError::Invalid(
                "Python packages must be exact name==version pins".into(),
            ));
        };
        if name.is_empty()
            || name.len() > 64
            || version.is_empty()
            || version.len() > 48
            || !name.as_bytes()[0].is_ascii_alphanumeric()
            || !name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            || !version.as_bytes()[0].is_ascii_digit()
            || !version
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._+!".contains(&c))
            || !names.insert(
                name.to_ascii_lowercase()
                    .split(['_', '.', '-'])
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("-"),
            )
        {
            return Err(SandboxError::Invalid(
                "invalid or duplicate Python package pin".into(),
            ));
        }
    }
    Ok(())
}

pub(super) async fn prepare(
    config: &Config,
    job: &Job,
    cancel: &CancellationToken,
    group: &cgroup::Group,
    directory: &Path,
) -> Result<PathBuf> {
    validate_python_packages(&job.python_packages)?;
    if !config.uv.is_absolute() || !config.uv.is_file() {
        return Err(SandboxError::Unavailable(
            "Python execution requires an absolute operator-configured uv binary".into(),
        ));
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(&job.python_environment_key, &job.python_packages))
                .map_err(|e| SandboxError::Invalid(e.to_string()))?
        )
    );
    let environment = config.runtime_dir.join(format!("python-{hash}"));
    if environment.join(".hydra-ready").is_file() {
        if std::fs::read(environment.join(".hydra-ready"))? != hash.as_bytes() {
            return Err(SandboxError::Unavailable(
                "Python environment readiness record invalid".into(),
            ));
        }
        return Ok(environment);
    }
    if tree_size(&config.runtime_dir)? > 2 * 1024 * 1024 * 1024 {
        return Err(SandboxError::Unavailable(
            "Python environment quota exhausted; operator cleanup required".into(),
        ));
    }
    let staging = directory.join("python");
    std::fs::create_dir(&staging)?;
    let cleanup = RuntimeDirectory(staging.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    provision(
        config,
        group,
        &staging,
        &[
            "venv",
            "--offline",
            "--no-python-downloads",
            "--python",
            "/usr/bin/python3",
            "/python",
        ]
        .map(str::to_owned),
        cancel,
        deadline,
        false,
    )
    .await?;
    if !job.python_packages.is_empty() {
        let mut args = [
            "pip",
            "install",
            "--python",
            "/python/bin/python",
            "--no-python-downloads",
            "--only-binary",
            ":all:",
            "--index-url",
            "https://pypi.org/simple",
        ]
        .map(str::to_owned)
        .to_vec();
        args.extend(job.python_packages.clone());
        provision(config, group, &staging, &args, cancel, deadline, true).await?;
    }
    // Inspect distribution metadata without starting a Python that could load .pth files.
    std::fs::write(
        staging.join(".hydra-resolved.txt"),
        resolved_packages(&staging)?,
    )?;
    std::fs::write(staging.join(".hydra-ready"), hash)?;
    std::fs::rename(&staging, &environment)?;
    drop(cleanup);
    Ok(environment)
}

fn resolved_packages(path: &Path) -> Result<String> {
    let mut stack = vec![path.to_owned()];
    let mut pins = Vec::new();
    let mut count = 0;
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            count += 1;
            if count > 100_000 {
                return Err(SandboxError::Unavailable(
                    "Python metadata file quota exhausted".into(),
                ));
            }
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let directory = entry.path();
            if entry.file_name().to_string_lossy().ends_with(".dist-info") {
                let metadata = directory.join("METADATA");
                let attributes = std::fs::symlink_metadata(&metadata)?;
                if !attributes.is_file() || attributes.len() > 65536 {
                    return Err(SandboxError::Unavailable(
                        "invalid Python distribution metadata".into(),
                    ));
                }
                let contents = std::fs::read_to_string(metadata)?;
                let name = contents
                    .lines()
                    .find_map(|line| line.strip_prefix("Name: "))
                    .ok_or_else(|| {
                        SandboxError::Unavailable("Python distribution name absent".into())
                    })?;
                let version = contents
                    .lines()
                    .find_map(|line| line.strip_prefix("Version: "))
                    .ok_or_else(|| {
                        SandboxError::Unavailable("Python distribution version absent".into())
                    })?;
                let pin = format!("{name}=={version}");
                validate_python_packages(std::slice::from_ref(&pin))?;
                pins.push(pin);
                if pins.len() > 1024 {
                    return Err(SandboxError::Unavailable(
                        "Python distribution quota exhausted".into(),
                    ));
                }
            } else {
                stack.push(directory);
            }
        }
    }
    pins.sort();
    let resolved = pins.join("\n");
    if resolved.len() > 32768 {
        return Err(SandboxError::Unavailable(
            "resolved Python metadata exceeds 32 KiB".into(),
        ));
    }
    Ok(resolved)
}

fn tree_size(path: &Path) -> Result<u64> {
    let mut size = 0u64;
    let mut stack = vec![path.to_owned()];
    let mut count = 0;
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            count += 1;
            if count > 100_000 {
                return Err(SandboxError::Unavailable(
                    "Python environment file quota exhausted".into(),
                ));
            }
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            } else {
                size = size.saturating_add(metadata.len());
            }
        }
    }
    Ok(size)
}

async fn provision(
    config: &Config,
    group: &cgroup::Group,
    staging: &Path,
    uv_args: &[String],
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
    network: bool,
) -> Result<Vec<u8>> {
    let mut args: Vec<OsString> = [
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/bin",
        "/bin",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/lib",
        "/lib64",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--clearenv",
        "--setenv",
        "PATH",
        "/usr/bin",
        "--setenv",
        "HOME",
        "/tmp",
        "--setenv",
        "UV_CONCURRENT_DOWNLOADS",
        "1",
        "--setenv",
        "UV_CONCURRENT_INSTALLS",
        "1",
        "--ro-bind",
    ]
    .map(OsString::from)
    .to_vec();
    args.extend([
        config.uv.as_os_str().to_owned(),
        "/uv".into(),
        "--bind".into(),
        staging.as_os_str().to_owned(),
        "/python".into(),
        "--chdir".into(),
        "/tmp".into(),
    ]);
    if network {
        args.extend(
            [
                "--share-net",
                "--ro-bind",
                "/etc/resolv.conf",
                "/etc/resolv.conf",
                "--ro-bind",
                "/etc/ssl/certs",
                "/etc/ssl/certs",
            ]
            .map(OsString::from),
        );
    }
    args.extend(
        [
            "--",
            "/uv",
            "--no-config",
            "--no-cache",
            "--no-progress",
            "--native-tls",
        ]
        .map(OsString::from),
    );
    args.extend(uv_args.iter().map(OsString::from));
    let mut child = tokio::process::Command::new(&config.helper)
        .arg("sandbox-child")
        .arg(group.path())
        .arg(&config.bwrap)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SandboxError::Unavailable("missing uv output stream".into()))?;
    let output = tokio::spawn(super::capture(stdout));
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| SandboxError::Unavailable("missing uv diagnostic stream".into()))?;
    let capture = tokio::spawn(super::capture(stderr));
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let status = loop {
        tokio::select! {
            status = child.wait() => break status?,
            () = cancel.cancelled() => {group.kill()?; return Err(SandboxError::Unavailable("Python provisioning cancelled".into()));},
            () = tokio::time::sleep_until(deadline) => {group.kill()?; return Err(SandboxError::Unavailable("Python provisioning timed out (120s)".into()));},
            _ = interval.tick() => {if tree_size(staging)? > 256 * 1024 * 1024 {group.kill()?;return Err(SandboxError::Unavailable("Python environment exceeds 256 MiB".into()));}}
        }
    };
    let (stdout, lost) = output
        .await
        .map_err(|_| SandboxError::Unavailable("uv output unavailable".into()))??;
    if lost != 0 {
        return Err(SandboxError::Unavailable("uv output exceeds 32 KiB".into()));
    }
    let (stderr, _) = capture
        .await
        .map_err(|_| SandboxError::Unavailable("uv diagnostics unavailable".into()))??;
    if !status.success() {
        return Err(SandboxError::Unavailable(format!(
            "uv environment preparation failed (compatible wheels required): {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    if tree_size(staging)? > 256 * 1024 * 1024 {
        return Err(SandboxError::Unavailable(
            "Python environment exceeds 256 MiB".into(),
        ));
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pins_accept_registry_names_and_reject_options_paths_urls_and_duplicates() {
        assert!(
            validate_python_packages(&[
                "requests==2.32.5".into(),
                "charset-normalizer==3.4.3".into()
            ])
            .is_ok()
        );
        for pin in [
            "requests",
            "requests>=2",
            "-r==1",
            "./local==1",
            "https://host/pkg==1",
            "x @ https://host",
            "a==1;os_name",
            "a==1\n--index-url=evil",
            "a==*",
        ] {
            assert!(validate_python_packages(&[pin.into()]).is_err(), "{pin}");
        }
        assert!(validate_python_packages(&["A_b==1".into(), "a.b==2".into()]).is_err());
        assert!(validate_python_packages(&vec!["a==1".into(); 33]).is_err());
    }
}

use super::{Config, Job, Result, Runtime, SandboxError};
use std::{
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    path::{Component, Path},
};

pub(super) fn validate_job(job: &Job, config: &Config) -> Result<()> {
    if job
        .endpoints
        .first()
        .is_none_or(|endpoint| endpoint.address != job.target)
        || job.endpoints.is_empty()
        || job.endpoints.len() > 256
        || job.endpoints.iter().any(|e| {
            !config.allowlist.contains(&e.address)
                || !["tcp", "udp"].contains(&e.transport.as_str())
                || super::validate_target(e.address).is_err()
        })
    {
        return Err(SandboxError::Invalid(
            "target absent from explicit allowlist".into(),
        ));
    }
    let mut endpoints = std::collections::HashSet::new();
    if job
        .endpoints
        .iter()
        .any(|endpoint| !endpoints.insert((endpoint.address, endpoint.transport.as_str())))
    {
        return Err(SandboxError::Invalid("duplicate batch target endpoint: selected target aliases share an address and transport".into()));
    }
    if job.timeout.is_zero()
        || job.timeout.as_secs() > 3600
        || !(16 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&job.memory_bytes)
        || !(10..=2000).contains(&job.cpu_millis_per_second)
        || !(8..=256).contains(&job.pids)
    {
        return Err(SandboxError::Invalid(
            "invalid worker resource limits".into(),
        ));
    }
    let entry = Path::new(&job.entrypoint);
    if entry.as_os_str().is_empty()
        || entry
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(SandboxError::Invalid(
            "entrypoint must be a relative file path".into(),
        ));
    }
    if !job.source_dir.is_absolute()
        || !job.source_dir.is_dir()
        || job.source_dir.canonicalize()? != job.source_dir
    {
        return Err(SandboxError::Invalid(
            "upload snapshot directory missing".into(),
        ));
    }
    validate_tree(&job.source_dir, 0, &mut 0, &mut 0)?;
    if !job.source_dir.join(entry).is_file() {
        return Err(SandboxError::Invalid("entrypoint absent".into()));
    }
    Ok(())
}
fn validate_tree(path: &Path, depth: usize, count: &mut usize, total: &mut u64) -> Result<()> {
    if depth > 8 {
        return Err(SandboxError::Invalid("upload tree too deep".into()));
    }
    for entry in std::fs::read_dir(path)? {
        *count += 1;
        if *count > 1024 {
            return Err(SandboxError::Invalid("upload tree too large".into()));
        }
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
            return Err(SandboxError::Invalid(
                "upload contains special file or symlink".into(),
            ));
        }
        if kind.is_file() {
            *total = total.saturating_add(entry.metadata()?.len());
            if *total > 32 * 1024 * 1024 {
                return Err(SandboxError::Invalid(
                    "upload snapshot exceeds 32 MiB".into(),
                ));
            }
        }
        if kind.is_dir() {
            validate_tree(&entry.path(), depth + 1, count, total)?;
        }
    }
    Ok(())
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
pub(super) fn outer_args(
    config: &Config,
    job: &Job,
    directory: &Path,
    json: &str,
) -> Vec<OsString> {
    let mut output = args(&[
        "--unshare-all",
        "--uid",
        "0",
        "--gid",
        "0",
        "--die-with-parent",
        "--new-session",
        "--cap-add",
        "CAP_NET_ADMIN",
        "--cap-add",
        "CAP_SYS_ADMIN",
        "--cap-add",
        "CAP_NET_BIND_SERVICE",
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
        "LANG",
        "C.UTF-8",
        "--ro-bind",
    ]);
    output.extend([
        config.helper.as_os_str().to_owned(),
        OsString::from("/hydra-helper"),
        OsString::from("--ro-bind"),
        config.bwrap.as_os_str().to_owned(),
        OsString::from("/hydra-bwrap"),
        OsString::from("--ro-bind"),
        config.ip.as_os_str().to_owned(),
        OsString::from("/hydra-ip"),
        OsString::from("--ro-bind"),
        job.source_dir.as_os_str().to_owned(),
        OsString::from("/work"),
        OsString::from("--bind"),
        directory.as_os_str().to_owned(),
        OsString::from("/broker"),
        OsString::from("--"),
        OsString::from("/hydra-helper"),
        OsString::from("sandbox-bridge"),
        OsString::from(json),
    ]);
    output
}
pub(super) fn inner_command(config: &super::BridgeConfig) -> std::process::Command {
    let mut command = std::process::Command::new("/hydra-bwrap");
    command.args([
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
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
        "--ro-bind",
        "/work",
        "/work",
        "--chdir",
        "/work",
        "--clearenv",
        "--setenv",
        "PATH",
        "/usr/bin",
        "--setenv",
        "LANG",
        "C.UTF-8",
        "--setenv",
        "TARGET_HOST",
        &config.target.ip().to_string(),
        "--setenv",
        "TARGET_PORT",
        &config.target.port().to_string(),
        "--setenv",
        "TARGETS_JSON",
        &serde_json::to_string(&config.endpoints).expect("endpoint JSON"),
        "--",
    ]);
    let entry = format!("/work/{}", config.entrypoint);
    match config.runtime {
        Runtime::Python => {
            command.args(["/usr/bin/python3", "-I", &entry]);
        }
        Runtime::Shell => {
            command.args(["/usr/bin/sh", &entry]);
        }
        Runtime::Elf => {
            command.arg(entry);
        }
    }
    command.env_clear();
    command
}
/// Hidden CLI entrypoint. Must run before Core loads DB/config/secrets or starts threads.
/// Joins the prepared cgroup before exec; no shell and no inherited environment.
pub fn worker_child(cgroup: &Path, bwrap: &Path, arguments: &[OsString]) -> Result<()> {
    let path = cgroup.canonicalize()?;
    if !path.starts_with("/sys/fs/cgroup")
        || !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("hydra-"))
        || !bwrap.is_absolute()
    {
        return Err(SandboxError::Invalid(
            "invalid sandbox child invocation".into(),
        ));
    }
    std::fs::write(path.join("cgroup.procs"), std::process::id().to_string())?;
    let error = std::process::Command::new(bwrap)
        .args(arguments)
        .env_clear()
        .env("PATH", OsStr::new("/usr/bin"))
        .exec();
    Err(error.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_never_shares_host_network_or_mounts_core_data() {
        let config = Config {
            delegated_cgroup: "/delegation".into(),
            runtime_dir: "/runtime".into(),
            bwrap: "/usr/bin/bwrap".into(),
            ip: "/usr/bin/ip".into(),
            helper: "/core".into(),
            allowlist: vec![],
        };
        let job = Job {
            source_dir: "/upload".into(),
            entrypoint: "run.py".into(),
            runtime: Runtime::Python,
            target: "192.0.2.1:80".parse().unwrap(),
            endpoints: vec![],
            timeout: std::time::Duration::from_secs(1),
            memory_bytes: 32 * 1024 * 1024,
            cpu_millis_per_second: 100,
            pids: 16,
        };
        let values = outer_args(&config, &job, Path::new("/runtime/job"), "{}");
        assert!(values.contains(&OsString::from("--unshare-all")));
        assert!(values.contains(&OsString::from("--clearenv")));
        assert!(!values.contains(&OsString::from("--share-net")));
        assert!(!values.contains(&OsString::from("/home")));
        assert!(!values.contains(&OsString::from("/sys")));
    }
    #[test]
    fn duplicate_batch_endpoints_rejected_before_filesystem_or_launch() {
        let address = "192.0.2.1:80".parse().unwrap();
        let config = Config {
            delegated_cgroup: "/missing".into(),
            runtime_dir: "/missing".into(),
            bwrap: "/missing".into(),
            ip: "/missing".into(),
            helper: "/missing".into(),
            allowlist: vec![address],
        };
        let job = Job {
            source_dir: "/missing".into(),
            entrypoint: "run".into(),
            runtime: Runtime::Elf,
            target: address,
            endpoints: vec![
                super::super::Endpoint {
                    id: "a".into(),
                    address,
                    transport: "udp".into(),
                },
                super::super::Endpoint {
                    id: "alias".into(),
                    address,
                    transport: "udp".into(),
                },
            ],
            timeout: std::time::Duration::from_secs(1),
            memory_bytes: 32 * 1024 * 1024,
            cpu_millis_per_second: 100,
            pids: 16,
        };
        assert!(
            matches!(validate_job(&job,&config),Err(SandboxError::Invalid(message)) if message.contains("duplicate batch target endpoint"))
        );
    }
    #[test]
    fn arbitrary_target_rejected_before_filesystem_or_launch() {
        let config = Config {
            delegated_cgroup: "/missing".into(),
            runtime_dir: "/missing".into(),
            bwrap: "/missing".into(),
            ip: "/missing".into(),
            helper: "/missing".into(),
            allowlist: vec!["192.0.2.1:80".parse().unwrap()],
        };
        let job = Job {
            source_dir: "/missing".into(),
            entrypoint: "../../etc/shadow".into(),
            runtime: Runtime::Shell,
            target: "192.0.2.2:80".parse().unwrap(),
            endpoints: vec![],
            timeout: std::time::Duration::from_secs(1),
            memory_bytes: 32 * 1024 * 1024,
            cpu_millis_per_second: 100,
            pids: 16,
        };
        assert!(
            matches!(validate_job(&job, &config), Err(SandboxError::Invalid(message)) if message.contains("allowlist"))
        );
    }
}

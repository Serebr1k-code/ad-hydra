use super::{Job, Result, SandboxError};
use std::path::{Path, PathBuf};

pub(super) struct Group {
    path: PathBuf,
}
impl Group {
    pub fn create(parent: &Path, job: &Job) -> Result<Self> {
        // Never enable controllers or move existing processes in the supplied subtree.
        let parent = parent
            .canonicalize()
            .map_err(|e| SandboxError::Unavailable(format!("delegated cgroup unavailable: {e}")))?;
        if !parent.starts_with("/sys/fs/cgroup")
            || parent == Path::new("/sys/fs/cgroup")
            || !parent.join("cgroup.controllers").is_file()
        {
            return Err(SandboxError::Unavailable(
                "an explicit delegated cgroup v2 subtree is required".into(),
            ));
        }
        let path = parent.join(format!("hydra-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path)
            .map_err(|e| SandboxError::Unavailable(format!("cannot create sandbox cgroup: {e}")))?;
        let group = Self { path };
        for control in [
            "cpu.max",
            "memory.max",
            "pids.max",
            "cgroup.kill",
            "cgroup.procs",
        ] {
            if !group.path.join(control).is_file() {
                return Err(SandboxError::Unavailable(format!(
                    "delegation lacks {control}"
                )));
            }
        }
        std::fs::write(
            group.path.join("cpu.max"),
            format!("{} 100000", job.cpu_millis_per_second * 100),
        )?;
        std::fs::write(group.path.join("memory.max"), job.memory_bytes.to_string())?;
        std::fs::write(group.path.join("memory.swap.max"), "0")?;
        std::fs::write(group.path.join("memory.oom.group"), "1")?;
        std::fs::write(group.path.join("pids.max"), job.pids.to_string())?;
        group.kill()?; // Prove kill permission before even starting the trusted helper.
        Ok(group)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn kill(&self) -> Result<()> {
        std::fs::write(self.path.join("cgroup.kill"), "1").map_err(Into::into)
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        let _ = self.kill();
        // Reaping belongs to the owner; cleanup can race kernel cgroup exit processing.
        let _ = std::fs::remove_dir(&self.path);
    }
}

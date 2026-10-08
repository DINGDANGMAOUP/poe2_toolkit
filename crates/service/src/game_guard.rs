//! Read-only process checks, supplemented by file handles during a transaction.
use anyhow::{Result, ensure};
use std::path::Path;
pub(crate) fn ensure_stopped(root: &Path) -> Result<()> {
    ensure!(
        !is_running(root)?,
        "游戏或安装目录内的程序正在运行，请关闭后重试"
    );
    Ok(())
}
pub(crate) fn is_running(root: &Path) -> Result<bool> {
    let system = sysinfo::System::new_all();
    let root = std::fs::canonicalize(root)?;
    for process in system.processes().values() {
        let name = process.name().to_string_lossy().to_ascii_lowercase();
        let known = name.starts_with("pathofexile") || name.starts_with("pathofexile2");
        let executable = process.exe().and_then(|p| std::fs::canonicalize(p).ok());
        let in_installation = executable.as_ref().is_some_and(|p| p.starts_with(&root));
        if in_installation || (known && executable.is_none()) {
            return Ok(true);
        }
    }
    Ok(false)
}

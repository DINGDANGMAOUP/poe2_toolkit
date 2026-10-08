//! One maintenance gate shared by CLI and desktop using the same data store.
use crate::Store;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};

pub(crate) fn acquire(store: &Store) -> Result<File> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(store.root.join("maintenance.lock"))?;
    lock.try_lock_exclusive()
        .context("另一个写入、恢复或程序升级任务正在执行")?;
    Ok(lock)
}

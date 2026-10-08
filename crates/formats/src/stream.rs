use anyhow::{Result, ensure};
use poe2_core::{StreamPiece, StreamRecipe};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub fn reconstruct(
    source: &Path,
    recipe: &StreamRecipe,
    output: &mut impl Write,
) -> Result<String> {
    let expected = recipe
        .output_len()
        .ok_or_else(|| anyhow::anyhow!("流式重建范围非法"))?;
    let mut file = File::open(source)?;
    ensure!(
        file.metadata()?.len() == recipe.source_len,
        "容器长度在规划后发生变化"
    );
    let mut hash = Sha256::new();
    let mut written = 0u64;
    let mut buffer = vec![0; 1024 * 1024];
    for piece in &recipe.pieces {
        match piece {
            StreamPiece::Copy { offset, length } => {
                file.seek(SeekFrom::Start(*offset))?;
                let mut remaining = *length;
                while remaining > 0 {
                    let n = buffer.len().min(remaining as usize);
                    file.read_exact(&mut buffer[..n])?;
                    output.write_all(&buffer[..n])?;
                    hash.update(&buffer[..n]);
                    remaining -= n as u64;
                    written += n as u64;
                }
            }
            StreamPiece::Bytes { data } => {
                output.write_all(data)?;
                hash.update(data);
                written += data.len() as u64;
            }
        }
    }
    ensure!(written == expected, "容器重建长度不一致");
    Ok(hex::encode(hash.finalize()))
}

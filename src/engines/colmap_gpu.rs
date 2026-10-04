//! Bounded, real CUDA SIFT probe. Device 0 is a CUDA-visible ordinal, so
//! CUDA_VISIBLE_DEVICES is respected without guessing nvidia-smi index mappings.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    engines::colmap,
    error::{Result, SplatError},
    process::ProcessManager,
};

struct ProbeDirectory(PathBuf);

impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) async fn probe(executable: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let root = std::env::temp_dir().join(format!("ooosplat-cuda-{}", uuid::Uuid::new_v4()));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let directory = ProbeDirectory(root);
    let images = directory.0.join("images");
    std::fs::create_dir(&images)?;
    // Textured, identical images supply real descriptors for the GPU matcher.
    let image = image::GrayImage::from_fn(128, 128, |x, y| {
        image::Luma([((x.wrapping_mul(73) ^ y.wrapping_mul(151) ^ (x * y * 7)) % 256) as u8])
    });
    for name in ["1.png", "2.png"] {
        image
            .save(images.join(name))
            .map_err(|error| SplatError::Process(error.to_string()))?;
    }
    let manager = ProcessManager::new();
    let operation = async {
        colmap::extract_features_quality_v2(
            executable,
            &directory.0.join("probe.db"),
            &images,
            None,
            None,
            128,
            256,
            directory.0.join("probe.log"),
            &manager,
            None,
            Some(0),
        )
        .await?;
        colmap::match_exhaustive(
            executable,
            &directory.0.join("probe.db"),
            directory.0.join("probe.log"),
            &manager,
            None,
            Some(0),
        )
        .await
    };
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => result,
        _ = tokio::time::sleep(Duration::from_secs(30)) => {
            manager.cancel();
            // Let ProcessManager reap the process group before deleting files.
            let _ = operation.await;
            Err(SplatError::Process("COLMAP CUDA 探测超过 30 秒".into()))
        }
    }
}

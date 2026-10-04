//! Video frame extraction orchestration lives in `engines::ffmpeg`.
//!
//! This module intentionally contains no frame filtering stage. `frames/` is the
//! final image set handed directly to COLMAP.

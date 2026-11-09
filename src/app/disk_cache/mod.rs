//! The loop files of the disk cache. The thumbnail part of the disk cache lives in
//! `crate::pictures::jobs::disk_cache`.

mod loop_file;

pub(super) use self::loop_file::{try_load_loop, try_save_loop};

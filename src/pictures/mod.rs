//! The picture module: the decode, the resize, the shape, the jobs and the Kitty output of the
//! pictures. phewr holds the master copy, and `just pictures-sync` copies it into other projects.
//!
//! The module imports no app type. It refers to itself through `super::`, `self::` or
//! `crate::pictures::`, and to no other module of the crate.

pub(crate) mod decode;
pub(crate) mod jobs;
pub(crate) mod kitty;
pub(crate) mod names;
pub(crate) mod resize;
pub(crate) mod shape;
#[cfg(test)]
pub(crate) mod test_support;

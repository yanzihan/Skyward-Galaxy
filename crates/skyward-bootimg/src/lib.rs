//! Android boot images with the ramdisk-only layout of header versions 3 and
//! 4 and the legacy layout of versions 0 to 2 (with the dtb of version 2,
//! which Samsung's recovery carries), and the newc archives their ramdisks
//! are: read, changed and written again, by the image packer on a computer
//! and by the recovery on the phone.

mod image;
mod ramdisk;

pub use image::{BootHeader, BootImage};
pub use ramdisk::{detect_format, lz4_legacy, Entry, Ramdisk, RamdiskFormat};

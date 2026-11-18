//! A newer build of Skyward, as its release zips it: the image in the zip,
//! checked to be Skyward for this phone, and the recovery partitions it
//! goes into, those of the slot that starts next and of any other slot
//! that holds Skyward.

use crate::image;
use anyhow::{ensure, Context, Result};
use skyward_vab::Slot;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

const PARTITION: &str = "recovery";

/// The image the zip at `path` holds, its only entry, read whole.
pub fn unzip(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).with_context(|| format!("read {} as a zip", path.display()))?;
    ensure!(zip.len() == 1, "{} holds {} entries, and a release's zip holds its image alone", path.display(), zip.len());
    let mut entry = zip.by_index(0).context("open the image in the zip")?;
    ensure!(entry.name().ends_with(".img"), "the zip holds {}, which is not an image", entry.name());
    let mut image = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut image).context("unzip the image")?;
    Ok(image)
}

/// Check that `image` is Skyward, packed for the phone `codename` names.
pub fn check(image: &[u8], codename: &str) -> Result<()> {
    let config = image::device_config(&mut Cursor::new(image)).context("look into the image")?.context("the image is not Skyward")?;
    let config = std::str::from_utf8(&config).context("the image's device configuration is not text")?;
    let named = skyward_config::codename(config).context("read the image's device configuration")?;
    ensure!(named == codename, "the image is Skyward for {named}, not for {codename}");
    Ok(())
}

/// The slots whose recovery partition, among the links in `dir`, the newer
/// build goes into, slot A first: `next`, the slot that starts next, and
/// any other whose partition holds Skyward now, any build of it. A phone
/// without slots names its one recovery partition without a suffix, which
/// both slots come to, and it is written once.
pub fn slots(dir: &Path, next: Slot) -> Result<Vec<Slot>> {
    let mut slots = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for slot in [Slot::A, Slot::B] {
        let path = image::device(dir, PARTITION, slot);
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        image::drop_cache(&file).with_context(|| format!("drop the cached pages of {}", path.display()))?;
        if slot == next || image::is_skyward(&mut file).with_context(|| format!("look into {}", path.display()))? {
            slots.push(slot);
        }
    }
    Ok(slots)
}

/// Write `image` into the recovery partitions of `slots`, among the links
/// in `dir`, and read each back; `progress` hears the bytes so far, of all.
pub fn write(dir: &Path, image: &[u8], slots: &[Slot], progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let devices: Vec<PathBuf> = slots.iter().map(|&slot| image::device(dir, PARTITION, slot)).collect();
    image::flash(&mut Cursor::new(image), image.len() as u64, &devices, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::samples::{recovery, skyward_for};
    use std::io::Write;

    /// A zip of `entries`, each a name and what it holds.
    fn zipped(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        file
    }

    #[test]
    fn the_image_in_a_releases_zip_is_taken_whole() {
        let image = skyward_for("p3q");
        let zip = zipped(&[("skyward-v0.1.4-p3q-G9980ZHSGHZB1.img", &image)]);
        assert_eq!(unzip(zip.path()).unwrap(), image);
        let two = zipped(&[("a.img", &image), ("b.img", &image)]);
        assert!(unzip(two.path()).is_err(), "a zip with more than the image");
        let other = zipped(&[("notes.txt", b"not an image")]);
        assert!(unzip(other.path()).is_err());
        let unzipped = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(unzipped.path(), &image).unwrap();
        assert!(unzip(unzipped.path()).is_err(), "an image that is not zipped");
    }

    #[test]
    fn only_skyward_for_this_phone_goes_in() {
        assert!(check(&skyward_for("p3q"), "p3q").is_ok());
        let e = check(&skyward_for("y2q"), "p3q").unwrap_err();
        assert_eq!(e.to_string(), "the image is Skyward for y2q, not for p3q");
        assert_eq!(check(&recovery(false), "p3q").unwrap_err().to_string(), "the image is not Skyward");
        assert!(check(&recovery(true), "p3q").is_err(), "a device configuration that names no phone");
    }

    /// Recovery partitions of `size` bytes holding `a` and `b`.
    fn partitions(a: &[u8], b: &[u8], size: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, image) in [("recovery_a", a), ("recovery_b", b)] {
            let mut bytes = image.to_vec();
            bytes.resize(size, 0);
            std::fs::write(dir.path().join(name), bytes).unwrap();
        }
        dir
    }

    #[test]
    fn the_build_goes_where_the_phone_starts_next_and_where_skyward_is() {
        let (skyward, stock) = (skyward_for("p3q"), recovery(false));
        let size = skyward.len().max(stock.len()) + 4096;
        assert_eq!(slots(partitions(&skyward, &stock, size).path(), Slot::A).unwrap(), [Slot::A]);
        assert_eq!(slots(partitions(&skyward, &skyward, size).path(), Slot::A).unwrap(), [Slot::A, Slot::B], "an older build in the other slot");
        assert_eq!(slots(partitions(&stock, &skyward, size).path(), Slot::A).unwrap(), [Slot::A, Slot::B], "Skyward started without being flashed");
        assert_eq!(slots(partitions(&stock, &stock, size).path(), Slot::B).unwrap(), [Slot::B]);
    }

    #[test]
    fn a_phone_without_slots_has_its_one_recovery_partition_written_once() {
        let skyward = skyward_for("p3q");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("recovery"), &skyward).unwrap();
        assert_eq!(slots(dir.path(), Slot::A).unwrap(), [Slot::A], "the plain partition, not both slots over it");
    }

    #[test]
    fn the_build_is_written_and_read_back() {
        let (older, newer, stock) = (recovery(true), skyward_for("p3q"), recovery(false));
        let size = older.len().max(newer.len()) + 4096;
        let dir = partitions(&older, &stock, size);
        let mut told = Vec::new();
        write(dir.path(), &newer, &[Slot::A], &mut |done, total| told.push((done, total))).unwrap();
        let total = 2 * newer.len() as u64;
        assert_eq!(told.last(), Some(&(total, total)), "written and read back");
        assert_eq!(&std::fs::read(dir.path().join("recovery_a")).unwrap()[..newer.len()], &newer[..]);
        assert_eq!(&std::fs::read(dir.path().join("recovery_b")).unwrap()[..stock.len()], &stock[..], "the other slot left alone");
    }
}

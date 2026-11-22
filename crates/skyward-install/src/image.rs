//! Partition images that come over sideload: what an image is from its
//! header and ramdisk, which partitions it can go to, and writing it to
//! them the way fastboot does.

use abootimg_oxide::binrw::io::Cursor;
use abootimg_oxide::binrw::BinRead;
use abootimg_oxide::{Header, VendorHeader};
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use skyward_vab::Slot;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const BOOT_MAGIC: &[u8] = b"ANDROID!";
const VENDOR_BOOT_MAGIC: &[u8] = b"VNDRBOOT";
/// A dtbo image starts with this, big-endian.
const DTBO_MAGIC: &[u8] = &[0xd7, 0xb7, 0xab, 0x1e];
const VBMETA_MAGIC: &[u8] = b"AVB0";

/// How a ramdisk is packed, by its first bytes: LZ4 in the legacy framing
/// Android uses or the standard one, gzip, or a bare newc archive.
const LZ4_LEGACY: &[u8] = &[0x02, 0x21, 0x4c, 0x18];
const LZ4_FRAME: &[u8] = &[0x04, 0x22, 0x4d, 0x18];
const GZIP: &[u8] = &[0x1f, 0x8b];
const NEWC: &[u8] = b"070701";

/// Where a recovery's ramdisk keeps its program, now and in older layouts.
const RECOVERY_PROGRAMS: [&str; 2] = ["system/bin/recovery", "sbin/recovery"];

/// A signed image ends with a footer that locates its signature, and the
/// bootloader looks for it in the last bytes of the partition.
const FOOTER_MAGIC: &[u8] = b"AVBf";
const FOOTER_LEN: u64 = 64;

const CHUNK: usize = 1 << 20;

/// An Android sparse image starts with this, little-endian.
const SPARSE_MAGIC: [u8; 4] = [0x3a, 0xff, 0x26, 0xed];
/// A super partition's metadata geometry starts with this, and the header
/// that follows the geometry carries the partitions, both little-endian.
const LP_GEOMETRY_MAGIC: u32 = 0x616c_4470;
const LP_HEADER_MAGIC: u32 = 0x414c_5030;
/// The magic of the filesystems a logical partition's image holds, at the
/// place each keeps it: ext4 at 0x438, f2fs at 0x400.
const EXT4_MAGIC: [u8; 2] = [0x53, 0xef];
const F2FS_MAGIC: [u8; 4] = [0x10, 0x20, 0xf5, 0xf2];
/// EROFS keeps its magic at the same place f2fs keeps its own.
const EROFS_MAGIC: [u8; 4] = [0xe2, 0xe1, 0xf5, 0xe0];
const EXT4_MAGIC_AT: usize = 0x438;
const F2FS_MAGIC_AT: usize = 0x400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// A boot image; what it carries decides where it can go.
    Boot { kernel: bool, ramdisk: Ramdisk },
    VendorBoot,
    Dtbo,
    Vbmeta,
    /// The whole super partition, its metadata and all its partitions.
    Super,
    /// One of the partitions super holds: system, vendor and their like.
    Logical,
}

/// The ramdisk a boot image carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ramdisk {
    None,
    /// The one that starts Android.
    Generic,
    /// A recovery's, which holds the recovery program.
    Recovery,
}

impl ImageKind {
    /// The partitions an image of this kind can go to, the likeliest first.
    /// A recovery that brings its own kernel can also be the boot image of
    /// a phone whose recovery lives in boot; without one it needs boot's.
    pub fn partitions(self) -> &'static [&'static str] {
        match self {
            ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Recovery } => &["recovery", "boot"],
            ImageKind::Boot { kernel: false, ramdisk: Ramdisk::Recovery } => &["recovery"],
            ImageKind::Boot { kernel: true, .. } => &["boot"],
            ImageKind::Boot { kernel: false, .. } => &["init_boot"],
            ImageKind::VendorBoot => &["vendor_boot"],
            ImageKind::Dtbo => &["dtbo"],
            ImageKind::Vbmeta => &["vbmeta", "vbmeta_system", "vbmeta_vendor"],
            ImageKind::Super => &["super"],
            // Filled with the partitions this phone's super holds, which
            // only the phone knows.
            ImageKind::Logical => &[],
        }
    }
}

/// The image `file` holds, from `head`, its first bytes, and for a boot
/// image its ramdisk; `None` when it is no image this recovery writes.
pub fn detect<R: Read + Seek>(file: &mut R, head: &[u8]) -> Result<Option<ImageKind>> {
    if head.starts_with(BOOT_MAGIC) {
        let header = Header::parse(&mut Cursor::new(head)).context("read the boot image header")?;
        let kernel = header.kernel_size() > 0;
        let ramdisk = match header.ramdisk_size() {
            0 => Ramdisk::None,
            size => {
                file.seek(SeekFrom::Start(header.ramdisk_position() as u64)).context("seek to the ramdisk")?;
                ramdisk(file.take(u64::from(size))).context("read the ramdisk")?
            }
        };
        ensure!(kernel || ramdisk != Ramdisk::None, "the boot image holds neither a kernel nor a ramdisk");
        return Ok(Some(ImageKind::Boot { kernel, ramdisk }));
    }
    if head.starts_with(VENDOR_BOOT_MAGIC) {
        VendorHeader::read(&mut Cursor::new(head)).context("read the vendor boot image header")?;
        return Ok(Some(ImageKind::VendorBoot));
    }
    if head.starts_with(DTBO_MAGIC) {
        return Ok(Some(ImageKind::Dtbo));
    }
    if head.starts_with(VBMETA_MAGIC) {
        return Ok(Some(ImageKind::Vbmeta));
    }
    // A sparse image says what it holds in its decoded form; a head too
    // short to hold its header is nothing this recovery writes.
    if head.len() >= 28 && head.starts_with(&SPARSE_MAGIC) {
        let mut decoded = Sparse::new(file, head)?;
        let mut inner = Vec::with_capacity(4096);
        (&mut decoded).take(4096).read_to_end(&mut inner).context("read the start of a sparse image")?;
        if let Some(kind) = detect(&mut io::Cursor::new(&inner[..]), &inner)? {
            return Ok(Some(kind));
        }
        return Ok(classified(&inner));
    }
    if classified(head).is_some() {
        return Ok(classified(head));
    }
    Ok(None)
}

/// What an image whose first bytes `head` are not a boot, vendor_boot, dtbo
/// or vbmeta image holds: the whole super, or one partition of it.
fn classified(head: &[u8]) -> Option<ImageKind> {
    if head.len() >= 4 && u32::from_le_bytes(head[0..4].try_into().expect("four bytes")) == LP_GEOMETRY_MAGIC {
        return Some(ImageKind::Super);
    }
    if head.len() >= EXT4_MAGIC_AT + 2 && head[EXT4_MAGIC_AT..EXT4_MAGIC_AT + 2] == EXT4_MAGIC {
        return Some(ImageKind::Logical);
    }
    if head.len() >= F2FS_MAGIC_AT + 4 && head[F2FS_MAGIC_AT..F2FS_MAGIC_AT + 4] == F2FS_MAGIC {
        return Some(ImageKind::Logical);
    }
    // Samsung keeps some partitions EROFS, whose magic stands where f2fs
    // keeps its own.
    if head.len() >= F2FS_MAGIC_AT + 4 && head[F2FS_MAGIC_AT..F2FS_MAGIC_AT + 4] == EROFS_MAGIC {
        return Some(ImageKind::Logical);
    }
    None
}

/// The partitions the super a phone holds carries, in the order it lists
/// them, without their slot's suffix where the phone is A/B.
pub fn logical_partitions() -> Result<Vec<String>> {
    let slot = skyward_vab::booted_slot()?;
    let mut out = Vec::new();
    for (base, size) in skyward_vab::slot_sizes(slot)? {
        if size > 0 {
            out.push(base);
        }
    }
    Ok(out)
}

/// The partition names a whole super's own metadata lists, read from the
/// image, so that a super made for another layout is refused before
/// anything is written. Every name must be bare: a phone that is not A/B
/// names its partitions without a slot's suffix, and a super that carries
/// `_a` and `_b` names is made for one that is.
pub fn super_partitions<R: Read>(image: &mut R, len: u64) -> Result<Vec<String>> {
    ensure!(len >= 8192, "the image is {len} bytes, too few to hold super's metadata");
    let mut geometry = [0u8; 4096];
    image.read_exact(&mut geometry).context("read the geometry of super's metadata")?;
    let magic = u32::from_le_bytes(geometry[0..4].try_into().expect("four bytes"));
    ensure!(magic == LP_GEOMETRY_MAGIC, "the image is no super: its metadata starts {magic:#x}");
    let max_size = u32::from_le_bytes(geometry[12..16].try_into().expect("four bytes")) as usize;
    ensure!((4096..=1 << 20).contains(&max_size), "super's metadata says its size is {max_size}");
    let mut header = vec![0u8; 4096.min(max_size)];
    image.read_exact(&mut header).context("read the header of super's metadata")?;
    let magic = u32::from_le_bytes(header[0..4].try_into().expect("four bytes"));
    ensure!(magic == LP_HEADER_MAGIC, "super's metadata header starts {magic:#x}");
    // The header: magic, the version, the header's size and checksum, then
    // the size and checksum of the tables, then their place and count: the
    // partitions' first, then the extents', the groups' and the block
    // devices', each with where it starts, how many entries and how long
    // each is.
    let header_size = u32::from_le_bytes(header[8..12].try_into().expect("four bytes")) as usize;
    ensure!(header_size >= 80 && header_size <= header.len(), "super's metadata header is {header_size} bytes");
    let field = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("four bytes")) as usize;
    let (first, partitions, entry) = (field(24), field(28), field(32));
    ensure!(partitions < 4096 && (36..=256).contains(&entry), "super's metadata names {partitions} partitions of {entry} bytes");
    let end = first.checked_add(partitions.saturating_mul(entry)).context("super's partition table has no end")?;
    ensure!(end <= header.len(), "super's partition table ends at {end}, past the {} bytes its header holds", header.len());
    let mut out = Vec::with_capacity(partitions);
    for i in 0..partitions {
        let at = first + i * entry;
        let raw = &header[at..at + entry];
        let name = raw[0..36].split(|b| *b == 0).next().unwrap_or(&[]);
        let name = String::from_utf8_lossy(name).into_owned();
        ensure!(!name.is_empty(), "a partition of super's metadata has no name");
        out.push(name);
    }
    Ok(out)
}

/// Whether a packed ramdisk is a recovery's, by the names in its archive.
fn ramdisk(packed: impl Read) -> Result<Ramdisk> {
    Ok(match holds(packed, |name| RECOVERY_PROGRAMS.contains(&name))? {
        true => Ramdisk::Recovery,
        false => Ramdisk::Generic,
    })
}

/// Whether a packed ramdisk has an entry whose name `wanted` picks.
fn holds(packed: impl Read, wanted: impl Fn(&str) -> bool) -> Result<bool> {
    Ok(find(packed, wanted, |_| Ok(()))?.is_some())
}

/// What `take` makes of the first entry of a packed ramdisk whose name
/// `wanted` picks, read from its start; none when no entry's name is
/// picked.
fn find<T>(packed: impl Read, wanted: impl Fn(&str) -> bool, take: impl FnOnce(&mut dyn Read) -> Result<T>) -> Result<Option<T>> {
    let mut packed = BufReader::new(packed);
    let start = packed.fill_buf().context("read the ramdisk's first bytes")?;
    let mut rest: Box<dyn Read + '_> = if start.starts_with(LZ4_LEGACY) || start.starts_with(LZ4_FRAME) {
        Box::new(lz4_flex::frame::FrameDecoder::new(packed))
    } else if start.starts_with(GZIP) {
        Box::new(flate2::bufread::MultiGzDecoder::new(packed))
    } else if start.starts_with(NEWC) {
        Box::new(packed)
    } else {
        bail!("a ramdisk packed in an unknown way, starting {:02x?}", &start[..start.len().min(4)]);
    };
    loop {
        let mut entry = cpio::NewcReader::new(rest).context("read a ramdisk entry")?;
        if entry.entry().is_trailer() {
            return Ok(None);
        }
        if wanted(entry.entry().name().trim_start_matches("./")) {
            return take(&mut entry).map(Some);
        }
        rest = entry.finish().context("read past a ramdisk entry")?;
    }
}

/// Whether `file` is a Skyward recovery: a boot image whose ramdisk carries
/// the device configuration Skyward is packed with, packed in one of the
/// ways read here, as Skyward packs it.
pub fn is_skyward<R: Read + Seek>(file: &mut R) -> Result<bool> {
    Ok(device_config(file)?.is_some())
}

/// The device configuration a Skyward recovery carries in its ramdisk;
/// none when `file` is no Skyward recovery.
pub fn device_config<R: Read + Seek>(file: &mut R) -> Result<Option<Vec<u8>>> {
    let mut head = Vec::with_capacity(4096);
    file.take(4096).read_to_end(&mut head).context("read the image's first bytes")?;
    if !head.starts_with(BOOT_MAGIC) {
        return Ok(None);
    }
    let header = Header::parse(&mut Cursor::new(&head)).context("read the boot image header")?;
    if header.ramdisk_size() == 0 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(header.ramdisk_position() as u64)).context("seek to the ramdisk")?;
    let mut ramdisk = BufReader::new(file.take(u64::from(header.ramdisk_size())));
    let start = ramdisk.fill_buf().context("read the ramdisk's first bytes")?;
    if ![LZ4_LEGACY, LZ4_FRAME, GZIP, NEWC].iter().any(|magic| start.starts_with(magic)) {
        return Ok(None);
    }
    let read = |entry: &mut dyn Read| {
        let mut config = Vec::new();
        entry.read_to_end(&mut config).context("read the device configuration")?;
        Ok(config)
    };
    find(ramdisk, |name| name == skyward_config::ramdisk::CONFIG, read).context("read the ramdisk")
}

/// The path a partition has here: on a phone with slots, the slot's; on
/// one without, the plain name the partition table gives it.
pub fn device(dir: &Path, name: &str, slot: Slot) -> PathBuf {
    let plain = dir.join(name);
    if plain.exists() {
        return plain;
    }
    dir.join(format!("{name}{}", slot.suffix()))
}

/// The partitions `kind` fits that this phone has, among the links in
/// `dir`, the likeliest first. A phone with slots has each in both; one
/// without has the plain names.
pub fn partitions(kind: ImageKind, dir: &Path) -> Result<Vec<&'static str>> {
    let mut found = Vec::new();
    for name in kind.partitions() {
        let mut has = true;
        for slot in [Slot::A, Slot::B] {
            let path = device(dir, name, slot);
            has &= path.try_exists().with_context(|| format!("look for {}", path.display()))?;
        }
        if has {
            found.push(*name);
        }
    }
    Ok(found)
}

/// Write `len` bytes of `image` to all of `devices` in one pass, then read
/// each back and compare. A signed image smaller than its partition gets
/// its footer moved to the partition's end, with zeros between, as
/// fastboot does. `progress` hears the bytes written and read back so far,
/// of all.
pub fn flash<R: Read + Seek>(image: &mut R, len: u64, devices: &[PathBuf], progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    ensure!(!devices.is_empty(), "no partition to write");
    let mut outs = Vec::with_capacity(devices.len());
    for path in devices {
        let mut f = OpenOptions::new().read(true).write(true).open(path).with_context(|| format!("open {}", path.display()))?;
        let size = f.seek(SeekFrom::End(0)).with_context(|| format!("find the size of {}", path.display()))?;
        f.seek(SeekFrom::Start(0)).with_context(|| format!("rewind {}", path.display()))?;
        outs.push((path, f, size));
    }
    let size = outs[0].2;
    ensure!(outs.iter().all(|(_, _, s)| *s == size), "the partition differs in size between slots");
    ensure!(len <= size, "the image is {len} bytes, more than the {size} its partition holds");

    let mut footer = [0u8; FOOTER_LEN as usize];
    let moved = len >= FOOTER_LEN && len < size && {
        image.seek(SeekFrom::Start(len - FOOTER_LEN)).context("seek to the image's end")?;
        image.read_exact(&mut footer).context("read the image's end")?;
        footer.starts_with(FOOTER_MAGIC)
    };
    image.seek(SeekFrom::Start(0)).context("rewind the image")?;
    let (gap, tail): (u64, &[u8]) = if moved { (size - len - FOOTER_LEN, &footer) } else { (0, &[]) };
    let end = len + gap + tail.len() as u64;
    let source = image.take(len).chain(io::repeat(0).take(gap)).chain(tail);

    write_and_check(source, end, outs, progress)
}

/// Write `end` bytes of `source` to every partition of `outs`, then read
/// each back and compare, telling the bytes written and read back so far.
fn write_and_check(
    mut source: impl Read,
    end: u64,
    mut outs: Vec<(&PathBuf, std::fs::File, u64)>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let total = end * outs.len() as u64 * 2;
    let mut buf = vec![0u8; CHUNK];
    let mut written = Sha256::new();
    let mut done = 0;
    let mut at = 0;
    while at < end {
        let n = (end - at).min(CHUNK as u64) as usize;
        source.read_exact(&mut buf[..n]).context("read the image")?;
        written.update(&buf[..n]);
        for (path, f, _) in &mut outs {
            f.write_all(&buf[..n]).with_context(|| format!("write {}", path.display()))?;
            done += n as u64;
            progress(done, total);
        }
        at += n as u64;
    }
    let written = written.finalize();
    for (path, f, _) in &mut outs {
        f.sync_all().with_context(|| format!("flush {}", path.display()))?;
        drop_cache(f).with_context(|| format!("drop the cached pages of {}", path.display()))?;
    }
    for (path, f, _) in &mut outs {
        f.seek(SeekFrom::Start(0)).with_context(|| format!("rewind {}", path.display()))?;
        let mut back = Sha256::new();
        let mut left = end;
        while left > 0 {
            let n = left.min(CHUNK as u64) as usize;
            f.read_exact(&mut buf[..n]).with_context(|| format!("read back {}", path.display()))?;
            back.update(&buf[..n]);
            left -= n as u64;
            done += n as u64;
            progress(done, total);
        }
        ensure!(back.finalize() == written, "{} reads back different from what was written", path.display());
    }
    Ok(())
}

/// Write `len` bytes of the already opened images to `devices` as they are,
/// with no footer to move: what a logical partition, a whole super or a
/// decoded sparse image goes in as.
pub fn flash_plain(image: impl Read, len: u64, devices: &[PathBuf], progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    ensure!(!devices.is_empty(), "no partition to write");
    let mut outs = Vec::with_capacity(devices.len());
    for path in devices {
        let mut f = OpenOptions::new().read(true).write(true).open(path).with_context(|| format!("open {}", path.display()))?;
        let size = f.seek(SeekFrom::End(0)).with_context(|| format!("find the size of {}", path.display()))?;
        f.seek(SeekFrom::Start(0)).with_context(|| format!("rewind {}", path.display()))?;
        ensure!(len <= size, "the image is {len} bytes, more than the {size} its partition holds");
        outs.push((path, f, size));
    }
    write_and_check(image, len, outs, progress)
}

/// A sparse image's chunks decoded into the stream its contents form.
pub struct Sparse<R> {
    inner: R,
    block: u32,
    left: u64,
    /// What the current chunk gives: bytes of the image, one byte repeated,
    /// or zeros.
    fill: [u8; 4],
    kind: ChunkKind,
    left_in_chunk: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkKind {
    Raw,
    Fill,
    Zero,
}

impl<R: Read + Seek> Sparse<R> {
    /// The image `inner`, whose first bytes are `head`, as the bytes it
    /// spells out.
    pub fn new(mut inner: R, head: &[u8]) -> Result<Sparse<R>> {
        ensure!(head.len() >= 28, "a sparse image's header is {} bytes", head.len());
        let u16_at = |at: usize| u16::from_le_bytes(head[at..at + 2].try_into().expect("two bytes"));
        let u32_at = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().expect("four bytes"));
        ensure!(head.starts_with(&SPARSE_MAGIC), "the image does not start as a sparse image");
        let file_header = u16_at(8) as u64;
        let block = u32_at(12);
        let blocks = u32_at(16) as u64;
        ensure!(file_header >= 28 && block.is_power_of_two(), "a sparse image's header says {file_header} bytes and {block}-byte blocks");
        inner.seek(SeekFrom::Start(file_header)).context("seek past a sparse image's header")?;
        Ok(Sparse { inner, block, left: blocks * u64::from(block), fill: [0; 4], kind: ChunkKind::Zero, left_in_chunk: 0 })
    }
}

impl<R> Sparse<R> {
    /// How many bytes the image spells out.
    fn spelled(&self) -> u64 {
        self.left
    }

}

impl<R: Read> Sparse<R> {
    /// Read the header of the next chunk, stepping over the ones that carry
    /// nothing of the image.
    fn next_chunk(&mut self) -> Result<()> {
        loop {
            let mut header = [0u8; 12];
            self.inner.read_exact(&mut header).context("read a sparse chunk's header")?;
            let u16_at = |at: usize| u16::from_le_bytes(header[at..at + 2].try_into().expect("two bytes"));
            let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("four bytes"));
            let (kind, blocks, bytes) = (u16_at(0), u64::from(u32_at(4)), u64::from(u32_at(8)));
            let data = blocks * u64::from(self.block);
            match kind {
                // Raw: the bytes of the image follow.
                0xcac1 => {
                    ensure!(bytes == 12 + data, "a raw chunk of {data} bytes says it takes {bytes}");
                    self.kind = ChunkKind::Raw;
                    self.left_in_chunk = data;
                    return Ok(());
                }
                // Fill: one four-byte pattern repeated.
                0xcac2 => {
                    ensure!(bytes == 16, "a fill chunk says it takes {bytes} bytes");
                    self.inner.read_exact(&mut self.fill)?;
                    self.kind = ChunkKind::Fill;
                    self.left_in_chunk = data;
                    return Ok(());
                }
                // Care: zeros.
                0xcac3 => {
                    ensure!(bytes == 12, "a don't-care chunk says it takes {bytes} bytes");
                    self.kind = ChunkKind::Zero;
                    self.left_in_chunk = data;
                    return Ok(());
                }
                // A checksum of the chunk before: nothing of the image.
                0xcac4 => {
                    ensure!(bytes == 16, "a crc chunk says it takes {bytes} bytes");
                    let mut crc = [0u8; 4];
                    self.inner.read_exact(&mut crc)?;
                    continue;
                }
                other => bail!("a sparse chunk of unknown kind {other:#x}"),
            }
        }
    }
}

impl<R: Read> Read for Sparse<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        if self.left_in_chunk == 0 {
            self.next_chunk().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")))?;
        }
        let n = (out.len() as u64).min(self.left_in_chunk).min(self.left) as usize;
        match self.kind {
            ChunkKind::Raw => self.inner.read_exact(&mut out[..n])?,
            ChunkKind::Fill => {
                for (i, b) in out[..n].iter_mut().enumerate() {
                    *b = self.fill[i % 4];
                }
            }
            ChunkKind::Zero => out[..n].fill(0),
        }
        self.left_in_chunk -= n as u64;
        self.left -= n as u64;
        Ok(n)
    }
}

/// The reader an image's bytes come from, decoding a sparse one: the
/// length of what it spells out, and the reader itself.
pub fn opened<R: Read + Seek + 'static>(mut inner: R) -> Result<(u64, Box<dyn Read>)> {
    let mut head = Vec::with_capacity(28);
    (&mut inner).take(28).read_to_end(&mut head).context("read an image's first bytes")?;
    if head.starts_with(&SPARSE_MAGIC) {
        let sparse = Sparse::new(inner, &head)?;
        // What is left is the whole of it, the reader not yet having read
        // anything.
        let len = sparse.spelled();
        return Ok((len, Box::new(sparse)));
    }
    let len = inner.seek(SeekFrom::End(0)).context("find the image's size")?;
    inner.seek(SeekFrom::Start(0)).context("rewind the image")?;
    Ok((len, Box::new(inner)))
}

pub(crate) use skyward_block::drop_cache;

/// Images made up for the tests here and in the modules that look into
/// partitions.
#[cfg(test)]
pub(crate) mod samples {
    use super::*;
    use abootimg_oxide::{HeaderV3, OsVersionPatch};

    /// A version 4 boot image with a kernel of `kernel` bytes and `ramdisk`
    /// as its ramdisk, each padded to the page.
    pub fn boot_image(kernel: u32, ramdisk: &[u8]) -> Vec<u8> {
        let header = Header::V3(HeaderV3 {
            kernel_size: kernel,
            ramdisk_size: ramdisk.len() as u32,
            osversionpatch: OsVersionPatch(0),
            cmdline: Box::new([0; 1536]),
            v4_signature_size: Some(0),
        });
        let mut out = Vec::new();
        header.write(&mut out).unwrap();
        out.resize(4096, 0);
        out.resize(4096 + (kernel as usize).div_ceil(4096) * 4096, 0x4b);
        out.extend_from_slice(ramdisk);
        out.resize(out.len().div_ceil(4096) * 4096, 0);
        out
    }

    /// A newc archive of empty files named `names`.
    pub fn archive(names: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for name in names {
            out = cpio::NewcBuilder::new(name).mode(0o100644).write(out, 0).finish().unwrap();
        }
        cpio::newc::trailer(out).unwrap()
    }

    pub use skyward_bootimg::lz4_legacy;

    /// A recovery image, Skyward's or the stock one it is packed on.
    pub fn recovery(skyward: bool) -> Vec<u8> {
        let mut names = vec!["init", "system/bin/recovery", "prop.default"];
        if skyward {
            names.extend(["skyward", "skyward/device.toml", "skyward/MiSansVF.ttf"]);
        }
        boot_image(0, &lz4_legacy(&archive(&names)))
    }

    /// Skyward's recovery for the phone `codename` names, its device
    /// configuration naming it.
    pub fn skyward_for(codename: &str) -> Vec<u8> {
        let config = format!("[device]\ncodename = \"{codename}\"\n");
        let mut out = Vec::new();
        for (name, data) in [("init", &b""[..]), ("system/bin/recovery", b""), ("skyward/device.toml", config.as_bytes())] {
            let mut entry = cpio::NewcBuilder::new(name).mode(0o100644).write(out, data.len() as u32);
            entry.write_all(data).unwrap();
            out = entry.finish().unwrap();
        }
        boot_image(0, &lz4_legacy(&cpio::newc::trailer(out).unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::samples::{archive, boot_image, lz4_legacy, recovery, skyward_for};
    use super::*;
    use abootimg_oxide::binrw::BinWrite;
    use abootimg_oxide::VendorHeaderV4;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut w = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        w.write_all(data).unwrap();
        w.finish().unwrap()
    }

    fn kind_of(image: &[u8]) -> Result<Option<ImageKind>> {
        detect(&mut Cursor::new(image), &image[..image.len().min(4096)])
    }

    #[test]
    fn boot_images_go_where_what_they_carry_fits() {
        let recovery = archive(&["init", "system", "system/bin", "system/bin/recovery", "sbin"]);
        let generic = archive(&["init", "first_stage_ramdisk", "system/bin/snapuserd"]);

        let kind = kind_of(&boot_image(0, &lz4_legacy(&recovery))).unwrap().unwrap();
        assert_eq!(kind, ImageKind::Boot { kernel: false, ramdisk: Ramdisk::Recovery });
        assert_eq!(kind.partitions(), ["recovery"]);
        let kind = kind_of(&boot_image(0, &gzip(&generic))).unwrap().unwrap();
        assert_eq!(kind, ImageKind::Boot { kernel: false, ramdisk: Ramdisk::Generic });
        assert_eq!(kind.partitions(), ["init_boot"]);
        let kind = kind_of(&boot_image(5000, &recovery)).unwrap().unwrap();
        assert_eq!(kind.partitions(), ["recovery", "boot"]);
        let kind = kind_of(&boot_image(5000, &lz4_legacy(&generic))).unwrap().unwrap();
        assert_eq!(kind, ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Generic });
        assert_eq!(kind.partitions(), ["boot"]);
        assert_eq!(kind_of(&boot_image(5000, &[])).unwrap(), Some(ImageKind::Boot { kernel: true, ramdisk: Ramdisk::None }));

        assert!(kind_of(&boot_image(0, &[])).is_err());
        assert!(kind_of(&boot_image(0, b"BZh91AY&SY")).is_err(), "a ramdisk packed some other way");
        assert!(kind_of(b"ANDROID!").is_err());
    }

    #[test]
    fn skyward_is_told_from_the_stock_recovery_it_is_packed_on() {
        assert!(is_skyward(&mut Cursor::new(recovery(true))).unwrap());
        assert!(!is_skyward(&mut Cursor::new(recovery(false))).unwrap());
        assert!(!is_skyward(&mut Cursor::new(boot_image(5000, &[]))).unwrap(), "a boot image with no ramdisk");
        assert!(!is_skyward(&mut Cursor::new(boot_image(0, b"\xfd7zXZ\0"))).unwrap(), "a ramdisk packed as Skyward never packs it");
        assert!(!is_skyward(&mut Cursor::new(vec![0u8; 8192])).unwrap(), "an erased partition");
    }

    #[test]
    fn skyward_carries_its_device_configuration() {
        let config = device_config(&mut Cursor::new(skyward_for("p3q"))).unwrap().expect("Skyward carries one");
        assert_eq!(config, b"[device]\ncodename = \"p3q\"\n");
        assert!(is_skyward(&mut Cursor::new(skyward_for("p3q"))).unwrap());
        assert_eq!(device_config(&mut Cursor::new(recovery(false))).unwrap(), None);
    }

    #[test]
    fn other_images_are_told_by_their_magic() {
        let vendor = VendorHeader {
            page_size: 4096,
            kernel_addr: 0,
            ramdisk_addr: 0,
            vendor_ramdisk_size: 0,
            cmdline: Box::new([0; 2048]),
            tags_addr: 0,
            board_name: [0; 16],
            dtb_size: 0,
            dtb_addr: 0,
            v4: Some(VendorHeaderV4 {
                vendor_ramdisk_table_size: 0,
                vendor_ramdisk_table_entry_num: 0,
                vendor_ramdisk_table_entry_size: 0,
                bootconfig_size: 0,
            }),
        };
        let mut bytes = Cursor::new(Vec::new());
        vendor.write(&mut bytes).unwrap();
        assert_eq!(kind_of(bytes.get_ref()).unwrap(), Some(ImageKind::VendorBoot));
        assert_eq!(kind_of(&[0xd7, 0xb7, 0xab, 0x1e, 0, 0, 0, 0x40]).unwrap(), Some(ImageKind::Dtbo));
        assert_eq!(kind_of(b"AVB0\0\0\0\x01").unwrap(), Some(ImageKind::Vbmeta));
        assert_eq!(kind_of(b"\x3a\xff\x26\xed sparse").unwrap(), None);
    }

    #[test]
    fn only_partitions_the_phone_has_in_both_slots_are_offered() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["boot_a", "boot_b", "recovery_a", "recovery_b", "init_boot_a", "init_boot_b", "vbmeta_a", "vbmeta_b", "vbmeta_system_a"] {
            std::fs::File::create(dir.path().join(name)).unwrap();
        }
        let both = ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Recovery };
        assert_eq!(partitions(both, dir.path()).unwrap(), ["recovery", "boot"]);
        assert_eq!(partitions(ImageKind::Vbmeta, dir.path()).unwrap(), ["vbmeta"]);
        assert!(partitions(ImageKind::Dtbo, dir.path()).unwrap().is_empty());
    }

    #[test]
    fn a_phone_without_slots_is_offered_its_plain_partitions() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["boot", "recovery"] {
            std::fs::File::create(dir.path().join(name)).unwrap();
        }
        let both = ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Recovery };
        assert_eq!(partitions(both, dir.path()).unwrap(), ["recovery", "boot"]);
        assert_eq!(device(dir.path(), "recovery", Slot::A), dir.path().join("recovery"));
        assert_eq!(device(dir.path(), "recovery", Slot::B), dir.path().join("recovery"));
    }

    /// A partition of `size` bytes filled with 0xAA, so untouched bytes
    /// show.
    fn partition(dir: &Path, name: &str, size: usize) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, vec![0xaa; size]).unwrap();
        path
    }

    fn flash_all(image: &[u8], devices: &[PathBuf]) -> Result<Vec<(u64, u64)>> {
        let mut seen = Vec::new();
        flash(&mut Cursor::new(image), image.len() as u64, devices, &mut |done, total| seen.push((done, total)))?;
        Ok(seen)
    }

    #[test]
    fn an_unsigned_image_leaves_the_rest_of_the_partition_alone() {
        let dir = tempfile::tempdir().unwrap();
        let slots = [partition(dir.path(), "dtbo_a", 3 << 20), partition(dir.path(), "dtbo_b", 3 << 20)];
        let image: Vec<u8> = (0..(1 << 20) + 300).map(|i| (i % 251) as u8).collect();
        let seen = flash_all(&image, &slots).unwrap();
        for slot in &slots {
            let got = std::fs::read(slot).unwrap();
            assert_eq!(&got[..image.len()], &image[..]);
            assert!(got[image.len()..].iter().all(|&b| b == 0xaa));
        }
        let total = image.len() as u64 * 4;
        assert_eq!(seen.last(), Some(&(total, total)));
        assert!(seen.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn a_signed_image_has_its_footer_moved_to_the_partition_end() {
        let dir = tempfile::tempdir().unwrap();
        let size = 2 << 20;
        let slots = [partition(dir.path(), "boot_a", size)];
        let mut image = vec![0x11u8; 100_000];
        let mut footer = b"AVBf".to_vec();
        footer.resize(FOOTER_LEN as usize, 0x22);
        image.extend_from_slice(&footer);
        flash_all(&image, &slots).unwrap();
        let got = std::fs::read(&slots[0]).unwrap();
        assert_eq!(&got[..image.len()], &image[..]);
        assert!(got[image.len()..size - 64].iter().all(|&b| b == 0));
        assert_eq!(&got[size - 64..], &footer[..]);
    }

    #[test]
    fn an_image_the_size_of_its_partition_is_written_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let slots = [partition(dir.path(), "vbmeta_a", 8192)];
        let mut image = vec![0x33u8; 8192 - 64];
        image.extend_from_slice(b"AVBf");
        image.resize(8192, 0x44);
        flash_all(&image, &slots).unwrap();
        assert_eq!(std::fs::read(&slots[0]).unwrap(), image);
    }

    #[test]
    fn an_image_larger_than_its_partition_is_refused_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let slots = [partition(dir.path(), "dtbo_a", 4096)];
        let err = flash_all(&[1u8; 4097], &slots).unwrap_err();
        assert!(err.to_string().contains("4097"), "{err}");
        assert!(std::fs::read(&slots[0]).unwrap().iter().all(|&b| b == 0xaa));
    }

    /// A sparse image of four blocks: one raw, one filled, two don't-care,
    /// with a crc chunk between them, spells out the same bytes as an
    /// image written plainly.
    #[test]
    fn a_sparse_image_spells_out_its_blocks() {
        let block = 4u32;
        let raw = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut sparse = Vec::new();
        sparse.extend_from_slice(&SPARSE_MAGIC);
        sparse.extend_from_slice(&0u16.to_le_bytes()); // major
        sparse.extend_from_slice(&0u16.to_le_bytes()); // minor
        sparse.extend_from_slice(&28u16.to_le_bytes()); // file header
        sparse.extend_from_slice(&12u16.to_le_bytes()); // chunk header
        sparse.extend_from_slice(&block.to_le_bytes());
        sparse.extend_from_slice(&4u32.to_le_bytes()); // total blocks
        sparse.extend_from_slice(&4u32.to_le_bytes()); // total chunks
        sparse.extend_from_slice(&0u32.to_le_bytes()); // checksum
        let chunk = |kind: u16, blocks: u32, bytes: u32| {
            let mut out = Vec::new();
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&blocks.to_le_bytes());
            out.extend_from_slice(&bytes.to_le_bytes());
            out
        };
        sparse.extend_from_slice(&chunk(0xcac1, 2, 12 + 8));
        let raw_chunk = raw;
        sparse.extend_from_slice(&raw_chunk);
        sparse.extend_from_slice(&chunk(0xcac4, 0, 16));
        sparse.extend_from_slice(&0u32.to_le_bytes()); // crc
        sparse.extend_from_slice(&chunk(0xcac2, 1, 16));
        sparse.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        sparse.extend_from_slice(&chunk(0xcac3, 1, 12));

        let head = sparse[..28].to_vec();
        let mut decoded = Sparse::new(io::Cursor::new(sparse), &head).unwrap();
        let mut out = Vec::new();
        decoded.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 16);
        assert_eq!(&out[0..2], &[1, 2]);
        assert_eq!(&out[2..8], &[3, 4, 5, 6, 7, 8]);
        assert_eq!(&out[8..12], &[0xaa, 0xbb, 0xcc, 0xdd]);
        assert_eq!(&out[12..16], &[0, 0, 0, 0]);
    }

    /// A super's metadata names its partitions; one named as an A/B phone
    /// names them is told apart.
    #[test]
    fn a_supers_metadata_names_its_partitions() {
        let mut image = vec![0u8; 8192];
        image[0..4].copy_from_slice(&LP_GEOMETRY_MAGIC.to_le_bytes());
        image[12..16].copy_from_slice(&4096u32.to_le_bytes()); // metadata size
        let header = 4096;
        image[header..header + 4].copy_from_slice(&LP_HEADER_MAGIC.to_le_bytes());
        image[header + 8..header + 12].copy_from_slice(&80u32.to_le_bytes()); // header size
        image[header + 24..header + 28].copy_from_slice(&80u32.to_le_bytes()); // partitions' place
        image[header + 28..header + 32].copy_from_slice(&2u32.to_le_bytes()); // how many
        image[header + 32..header + 36].copy_from_slice(&52u32.to_le_bytes()); // each length
        for (i, name) in ["system", "vendor_a"].iter().enumerate() {
            let at = header + 80 + i * 52;
            image[at..at + name.len()].copy_from_slice(name.as_bytes());
        }
        let names = super_partitions(&mut io::Cursor::new(image), 8192).unwrap();
        assert_eq!(names, vec!["system".to_string(), "vendor_a".to_string()]);
        assert!(names.iter().any(|n| n.ends_with("_a")), "a name with the slot's suffix is found");
    }
}

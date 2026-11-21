use crate::snapshots::{self, Merge, SnapshotStatus, Snapshots};
use crate::{snapshot_state, MergeStatus, Slot, SnapshotState, METADATA};
use anyhow::{ensure, Context, Result};
use skyward_block::{devnum, dm, mount, BY_NAME};
use skyward_lp::{Extent, Metadata, Partition, ATTR_DISABLED};
use std::fs;
use std::path::{Path, PathBuf};

/// A logical partition of super, mapped and mounted read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mounted {
    /// The name in super, such as vendor_dlkm_b; also the device-mapper name.
    pub partition: String,
    pub device: PathBuf,
    pub mount_point: PathBuf,
    pub filesystem: String,
}

#[derive(Debug, Clone, Default)]
pub struct Mounts {
    pub partitions: Vec<Mounted>,
    /// Damaged metadata copies in super that their backups stood in for.
    pub damaged: Vec<String>,
}

/// Where a logical partition goes in recovery. System goes to /mnt/system,
/// since /system holds recovery itself; a partition whose mount point
/// directory is in the ramdisk root uses it, as /vendor or /odm; any other
/// goes under /mnt.
pub fn mount_point(base: &str) -> PathBuf {
    let root = Path::new("/").join(base);
    if base != "system" && fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir()) {
        root
    } else {
        Path::new("/mnt").join(base)
    }
}

fn super_device() -> PathBuf {
    Path::new(BY_NAME).join("super")
}

/// The device numbers of super and its extra block devices, in the order
/// the metadata lists them, so an extent's device index resolves to one.
fn block_devices(md: &Metadata, super_dev: &Path) -> Result<Vec<(u32, u32)>> {
    md.block_devices
        .iter()
        .enumerate()
        .map(|(i, d)| if i == 0 { devnum(super_dev) } else { devnum(Path::new(BY_NAME).join(&d.partition_name)) })
        .collect()
}

/// The device-mapper targets that lay partition `p` out over super.
pub(crate) fn targets(p: &Partition, devices: &[(u32, u32)]) -> Vec<dm::Target> {
    p.extents
        .iter()
        .map(|e| match *e {
            Extent::Linear { sectors, device, sector } => dm::Target::Linear { sectors, device: devices[device], offset: sector },
            Extent::Zero { sectors } => dm::Target::Zero { sectors },
        })
        .collect()
}

/// How many bytes partition `p` holds.
fn bytes(p: &Partition) -> u64 {
    p.sectors() * 512
}

/// Map partition `p` from super read-only with device-mapper and mount it
/// over its point. A device that does not mount is removed again.
fn map_and_mount(p: &Partition, devices: &[(u32, u32)], slot: Slot) -> Result<Mounted> {
    // A third-party recovery's init maps super's partitions as it starts,
    // before any recovery program runs; a mapping already there is used as
    // it is, since making it again would fail with the device in use.
    let device = match dm::status(&p.name)? {
        Some((_, minor)) => PathBuf::from(format!("/dev/block/dm-{minor}")),
        None => dm::create(&p.name, &targets(p, devices), true)?,
    };
    let mount_point = mount_point(base(p, slot));
    let mounted = (|| -> Result<String> {
        fs::create_dir_all(&mount_point).with_context(|| format!("create {}", mount_point.display()))?;
        Ok(mount::mount_ro_over(&device, &mount_point).with_context(|| format!("mount {}", p.name))?.to_string())
    })();
    match mounted {
        Ok(filesystem) => Ok(Mounted { partition: p.name.clone(), device, mount_point, filesystem }),
        Err(e) => Err(match dm::remove(&p.name) {
            Ok(()) => e,
            Err(r) => e.context(format!("removing {} afterwards failed too: {r:#}", p.name)),
        }),
    }
}

/// The slot's partitions that hold data. Copy-on-write partitions such as
/// odm_b-cow end in -cow and stay out. On a phone that is not A/B the
/// partitions carry no suffix at all, and the plain names are the slot's.
fn slot_partitions(md: &Metadata, slot: Slot) -> Vec<&Partition> {
    let usable = |p: &&Partition| !p.extents.is_empty() && p.attributes & ATTR_DISABLED == 0;
    let suffixed: Vec<&Partition> = md.partitions.iter().filter(|p| p.name.ends_with(slot.suffix())).filter(usable).collect();
    if !suffixed.is_empty() {
        return suffixed;
    }
    md.partitions
        .iter()
        .filter(|p| !p.name.ends_with("_a") && !p.name.ends_with("_b") && !p.name.ends_with("-cow"))
        .filter(usable)
        .collect()
}

fn base(p: &Partition, slot: Slot) -> &str {
    p.name.strip_suffix(slot.suffix()).unwrap_or(&p.name)
}

/// Partitions are mapped only while no update snapshot is outstanding,
/// since a snapshotted partition reads correctly only through its snapshot.
fn ensure_no_snapshot() -> Result<()> {
    let state = snapshot_state()?;
    ensure!(state == SnapshotState::None, "an update snapshot is outstanding ({state:?})");
    Ok(())
}

/// Mount /metadata, then map the slot's logical partitions from super and
/// mount them, all read-only. Where an update went to the slot and is not
/// merged yet, each partition it has a snapshot of is mapped through that
/// snapshot, as the system's own first stage maps it, merging where the
/// merge is under way. A partition an earlier run of the recovery service
/// mounted, and that still shows, is taken as it is.
pub fn mount_partitions(slot: Slot) -> Result<Mounts> {
    let metadata = Path::new(METADATA);
    if !mount::is_mount_point(metadata)? {
        // A third-party recovery's ramdisk may have no /metadata to mount on.
        fs::create_dir_all(metadata).with_context(|| format!("make {}", metadata.display()))?;
        mount::mount_ro(&Path::new(BY_NAME).join("metadata"), metadata)?;
    }
    let super_dev = super_device();
    let md = skyward_lp::read(&super_dev, slot.number())?;
    let devices = block_devices(&md, &super_dev)?;
    let snaps = Snapshots::lock(true)?;
    let through = match &snaps {
        Some(s) if s.needed(slot)? => Some(s),
        _ => None,
    };

    let shown = mounted_partitions(slot)?;
    let mut mounts = Mounts { partitions: Vec::new(), damaged: md.damaged.clone() };
    for p in slot_partitions(&md, slot) {
        if let Some(m) = shown.iter().find(|m| m.partition == p.name) {
            mounts.partitions.push(m.clone());
            continue;
        }
        let mounted = match through.map(|s| Ok::<_, anyhow::Error>((s, s.live(p)?))).transpose()? {
            Some((s, Some(status))) => mount_snapshot(s, &md, &devices, p, &status, slot)?,
            _ => map_and_mount(p, &devices, slot)?,
        };
        mounts.partitions.push(mounted);
    }
    // The firmware the drivers and the key services read lives on partitions
    // of its own, which the phone's table mounts as it starts: the modem's,
    // what the wireless driver asks for, and the one the wireless chip keeps
    // its calibration on, which it waits for before it makes an interface.
    // Mount them here too, so programs started later find what they ask for.
    // A phone that keeps none, or keeps them elsewhere, is left as it was.
    for (point, name) in [
        ("/vendor/firmware_mnt", "firmware_mnt"),
        ("/vendor/firmware-modem", "firmware-modem"),
        ("/mnt/vendor/persist", "persist"),
    ] {
        let at = Path::new(point);
        if mount::is_mount_point(at)? {
            continue;
        }
        let Ok(entries) = skyward_block::fstab::read(point) else { continue };
        let Some(entry) = entries.first() else { continue };
        let Ok(device) = skyward_block::fstab::device(entry) else { continue };
        if !Path::new(&device).exists() {
            continue;
        }
        fs::create_dir_all(at).with_context(|| format!("make {}", at.display()))?;
        // A third-party recovery's policy may not know the context the
        // phone's table asks for, and the kernel refuses the mount outright
        // when it does not; the files are read by the recovery's own
        // programs, which run permissively or as root.
        let options: Vec<String> = entry.options.iter().filter(|o| !o.starts_with("context=")).cloned().collect();
        if mount::mount(Path::new(&device), at, &entry.filesystem, &options).is_err() {
            continue;
        }
        mounts.partitions.push(Mounted {
            partition: name.to_string(),
            device: PathBuf::from(device),
            mount_point: at.to_path_buf(),
            filesystem: entry.filesystem.clone(),
        });
    }
    Ok(mounts)
}

/// Start merging the update into `slot`, the slot it went to and the phone
/// runs from, its partitions mapped through their snapshots; see
/// [`snapshots::initiate`]. `tell` tells the bootloader where the update
/// stands.
pub fn start_merge(slot: Slot, tell: &dyn Fn(MergeStatus) -> Result<()>) -> Result<()> {
    let md = skyward_lp::read(&super_device(), slot.number())?;
    let snaps = Snapshots::lock(true)?.context("no update was ever applied")?;
    snapshots::initiate(&snaps, slot, &md, tell)
}

/// Whether every snapshot the slot reads through is mapped as a kernel
/// snapshot, as following a merge needs; `false` where it reads through
/// none.
pub fn snapshots_mapped(slot: Slot) -> Result<bool> {
    let md = skyward_lp::read(&super_device(), slot.number())?;
    let Some(snaps) = Snapshots::lock(false)? else { return Ok(false) };
    if !snaps.needed(slot)? {
        return Ok(false);
    }
    let mut any = false;
    for p in slot_partitions(&md, slot) {
        if snaps.live(p)?.is_some() {
            if !snapshots::is_snapshot(&p.name)? {
                return Ok(false);
            }
            any = true;
        }
    }
    Ok(any)
}

/// Look at the merge into `slot`, cleaning up after each snapshot merged;
/// see [`snapshots::check`].
pub fn check_merge(slot: Slot, tell: &dyn Fn(MergeStatus) -> Result<()>) -> Result<Merge> {
    let super_dev = super_device();
    let md = skyward_lp::read(&super_dev, slot.number())?;
    let devices = block_devices(&md, &super_dev)?;
    let snaps = Snapshots::lock(true)?.context("no update was ever applied")?;
    snapshots::check(&snaps, slot, &md, &devices, tell)
}

/// Map partition `p` through its snapshot `status` and mount it read-only;
/// a snapshot that does not mount is taken apart again.
fn mount_snapshot(snaps: &Snapshots, md: &Metadata, devices: &[(u32, u32)], p: &Partition, status: &SnapshotStatus, slot: Slot) -> Result<Mounted> {
    let device = snapshots::map_snapshot(snaps, md, devices, p, status)?;
    mount_mapped(p, device, slot)
}

/// Mount partition `p`, mapped as its snapshot at `device`, read-only over
/// its point; the snapshot is taken apart again if it does not mount.
fn mount_mapped(p: &Partition, device: PathBuf, slot: Slot) -> Result<Mounted> {
    let mount_point = mount_point(base(p, slot));
    let mounted = (|| -> Result<String> {
        fs::create_dir_all(&mount_point).with_context(|| format!("create {}", mount_point.display()))?;
        Ok(mount::mount_ro_over(&device, &mount_point).with_context(|| format!("mount {}", p.name))?.to_string())
    })();
    match mounted {
        Ok(filesystem) => Ok(Mounted { partition: p.name.clone(), device, mount_point, filesystem }),
        Err(e) => Err(match snapshots::unmap_snapshot(&p.name) {
            Ok(()) => e,
            Err(r) => e.context(format!("taking {} apart afterwards failed too: {r:#}", p.name)),
        }),
    }
}

/// The base names of the slot's logical partitions (such as `system`,
/// `vendor`), in the order super lists them, whether or not they are mounted.
pub fn slot_bases(slot: Slot) -> Result<Vec<String>> {
    let md = skyward_lp::read(&super_device(), slot.number())?;
    Ok(slot_partitions(&md, slot).into_iter().map(|p| base(p, slot).to_string()).collect())
}

/// The slot's logical partitions by base name with the bytes each holds, in
/// the order super lists them.
pub fn slot_sizes(slot: Slot) -> Result<Vec<(String, u64)>> {
    let md = skyward_lp::read(&super_device(), slot.number())?;
    Ok(slot_partitions(&md, slot).into_iter().map(|p| (base(p, slot).to_string(), bytes(p))).collect())
}

/// One of the slot's logical partitions mapped whole under a name of its
/// own, beside the mapping the recovery mounts, for a backup to read it or
/// a restore to write it.
#[derive(Debug)]
pub struct Whole {
    pub name: String,
    pub device: PathBuf,
    pub size: u64,
}

/// Map the slot's logical partition `base_name` whole as `name`, read-only
/// or not, while no update snapshot is outstanding.
pub fn map_whole(slot: Slot, base_name: &str, name: &str, readonly: bool) -> Result<Whole> {
    ensure_no_snapshot()?;
    let super_dev = super_device();
    let md = skyward_lp::read(&super_dev, slot.number())?;
    let devices = block_devices(&md, &super_dev)?;
    let full = format!("{base_name}{}", slot.suffix());
    let p = slot_partitions(&md, slot).into_iter().find(|p| p.name == full).with_context(|| format!("super holds no {full}"))?;
    let device = dm::create(name, &targets(p, &devices), readonly)?;
    Ok(Whole { name: name.into(), device, size: bytes(p) })
}

/// Remove a mapping [`map_whole`] made.
pub fn unmap_whole(whole: &Whole) -> Result<()> {
    dm::remove(&whole.name)
}

/// What [`map_slot`] did to the slot's logical partitions, for
/// [`unmap_slot`] to undo: those it mapped, and those the recovery had
/// mapped read-only already, and still uses, that it made writable.
#[derive(Debug, Default)]
pub struct SlotMaps {
    made: Vec<String>,
    opened: Vec<(String, Vec<dm::Target>)>,
}

/// Map each of the slot's logical partitions writable and under its own
/// name, as recoveries map them for install scripts, which look for them
/// at /dev/block/mapper/<name>; while no update snapshot is outstanding.
/// One the recovery has mapped read-only already is made writable where
/// it is, in use or not.
pub fn map_slot(slot: Slot) -> Result<SlotMaps> {
    ensure_no_snapshot()?;
    let super_dev = super_device();
    let md = skyward_lp::read(&super_dev, slot.number())?;
    let devices = block_devices(&md, &super_dev)?;
    let mut maps = SlotMaps::default();
    for p in slot_partitions(&md, slot) {
        if let Err(e) = map_writable(p, targets(p, &devices), &mut maps) {
            return Err(match unmap_slot(&maps) {
                Ok(()) => e,
                Err(r) => e.context(format!("undoing what was mapped before failed too: {r:#}")),
            });
        }
    }
    Ok(maps)
}

/// Map partition `p` writable with `table`, or make the read-only mapping
/// of it writable, keeping in `maps` what was done.
fn map_writable(p: &Partition, table: Vec<dm::Target>, maps: &mut SlotMaps) -> Result<()> {
    if dm::status(&p.name)?.is_none() {
        dm::create(&p.name, &table, false)?;
        maps.made.push(p.name.clone());
    } else if dm::readonly(&p.name)? {
        dm::reload(&p.name, &table, false)?;
        maps.opened.push((p.name.clone(), table));
    }
    Ok(())
}

/// Undo [`map_slot`]: remove the mappings it made, and make those it made
/// writable read-only again.
pub fn unmap_slot(maps: &SlotMaps) -> Result<()> {
    for name in &maps.made {
        dm::remove(name)?;
    }
    for (name, table) in &maps.opened {
        dm::reload(name, table, true)?;
    }
    Ok(())
}

/// Map and mount one of the slot's logical partitions read-only by its base
/// name (such as `system` or `vendor`): through its snapshot where the slot
/// reads through an update's snapshots, from the snapshot already mapped
/// where it was only unmounted.
pub fn mount_one(slot: Slot, base_name: &str) -> Result<Mounted> {
    let super_dev = super_device();
    let md = skyward_lp::read(&super_dev, slot.number())?;
    let devices = block_devices(&md, &super_dev)?;
    let name = format!("{base_name}{}", slot.suffix());
    let p = slot_partitions(&md, slot)
        .into_iter()
        .find(|p| p.name == name)
        .with_context(|| format!("super holds no {name}"))?;
    let snaps = Snapshots::lock(true)?;
    let status = match &snaps {
        Some(s) if s.needed(slot)? => s.live(p)?.map(|status| (s, status)),
        _ => None,
    };
    match status {
        Some(_) if dm::status(&name)?.is_some() => mount_mapped(p, Path::new("/dev/block").join(dm_node(&name)?), slot),
        Some((s, status)) => mount_snapshot(s, &md, &devices, p, &status, slot),
        None => map_and_mount(p, &devices, slot),
    }
}

/// The node device-mapper device `name` has under /dev/block, as dm-N.
fn dm_node(name: &str) -> Result<String> {
    let (_, minor) = dm::status(name)?.with_context(|| format!("{name} is not mapped"))?;
    Ok(format!("dm-{minor}"))
}

/// Unmount the partition mapped as `name` where its own device is what
/// shows on `point`, and take it apart: its device-mapper device, and where
/// it reads through a snapshot, the snapshot's own devices too. A partition
/// that is not mapped is left as it is.
fn unmap(name: &str, point: &Path) -> Result<()> {
    let Some(dev) = dm::status(name)? else { return Ok(()) };
    if mount::visible(point)?.is_some_and(|v| v.device == dev) {
        mount::umount_over(point)?;
    }
    snapshots::unmap_snapshot(name)
}

/// Unmount one of the slot's logical partitions by its base name and remove
/// its device-mapper device. One that reads through a snapshot is only
/// unmounted: its snapshot stays mapped, for its merge to go on and for it
/// to be mounted again.
pub fn unmount_one(slot: Slot, base_name: &str) -> Result<()> {
    let name = format!("{base_name}{}", slot.suffix());
    let point = mount_point(base_name);
    if !snapshots::is_snapshot(&name)? {
        return unmap(&name, &point);
    }
    let dev = dm::status(&name)?.with_context(|| format!("{name} is not mapped"))?;
    if mount::visible(&point)?.is_some_and(|v| v.device == dev) {
        mount::umount_over(&point)?;
    }
    Ok(())
}

/// The slot's partitions that are mapped and show on their mount points, in
/// the order they were mapped.
pub fn mounted_partitions(slot: Slot) -> Result<Vec<Mounted>> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/sys/block").context("read /sys/block")? {
        let entry = entry?;
        let node = entry.file_name().to_string_lossy().into_owned();
        let Some(minor) = node.strip_prefix("dm-") else { continue };
        let name_file = entry.path().join("dm/name");
        let name = fs::read_to_string(&name_file).with_context(|| format!("read {}", name_file.display()))?;
        let name = name.trim();
        // A phone that is not A/B names its mappings with no suffix at all.
        let Some(base) = name
            .strip_suffix(slot.suffix())
            .or_else(|| (!name.ends_with("_a") && !name.ends_with("_b")).then_some(name))
        else {
            continue;
        };
        let device = Path::new("/dev/block").join(&node);
        let mount_point = mount_point(base);
        let Some(shown) = mount::visible(&mount_point)? else { continue };
        if shown.device != devnum(&device)? {
            continue;
        }
        let m = Mounted { partition: name.to_string(), device, mount_point, filesystem: shown.filesystem };
        found.push((minor.parse::<u32>()?, m));
    }
    found.sort_by_key(|(minor, _)| *minor);
    Ok(found.into_iter().map(|(_, m)| m).collect())
}

/// Take the slot's mounted partitions away before an update writes over
/// them: unmount them and remove their device-mapper devices. Anything
/// still running from a partition (the recovery's key services keep files
/// there) is stopped first, so the partition is really gone rather than
/// left mapped under a detached mount. /metadata stays mounted for the
/// update.
pub fn release_partitions(slot: Slot, busy: &[PathBuf]) -> Result<()> {
    let _ = busy;
    for m in mounted_partitions(slot)?.into_iter().rev() {
        stop_users(&m.mount_point);
        // A recovery that starts a library from the installed system's
        // APEX mounts a loop device backed by a file on the partition,
        // which holds the partition busy until the loop goes.
        let loop_busy = release_loops_on(&m.mount_point)?;
        if loop_busy {
            mount::umount_detach_over(&m.mount_point)?;
            continue;
        }
        let mut unmounted = false;
        for _ in 0..20 {
            match mount::umount_over(&m.mount_point) {
                Ok(()) => {
                    unmounted = true;
                    break;
                }
                Err(e) if is_busy(&e) => {
                    stop_users(&m.mount_point);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
        if unmounted {
            snapshots::unmap_snapshot(&m.partition)?;
        } else {
            mount::umount_detach_over(&m.mount_point)?;
        }
    }
    Ok(())
}

/// Unmount the loop mounts whose backing file lies on `mount_point`, and
/// let their loop devices go: they hold the partition they are backed by
/// busy. A loop whose mount still has a program's linker mapped cannot go
/// until that program does; such programs are stopped first, so the
/// partition can come off cleanly instead of the updater writing over a
/// live device-mapper mapping. Whether anything had to be left detached.
fn release_loops_on(mount_point: &Path) -> Result<bool> {
    /// From linux/loop.h: clear the loop's backing file.
    const LOOP_CLR_FD: libc::c_ulong = 0x4c01;
    let mut busy = false;
    for entry in fs::read_dir("/sys/block").context("read /sys/block")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("loop") {
            continue;
        }
        let backing_file = entry.path().join("loop/backing_file");
        let Ok(backing) = fs::read_to_string(&backing_file) else { continue };
        let backing = backing.trim();
        // A recovery's init makes these loops in its own mount namespace,
        // where the installed system is /system rather than /mnt/system; a
        // backing path that does not exist here is one of those as well.
        let ours = Path::new(backing).starts_with(mount_point) || backing.starts_with("/system") || !Path::new(backing).exists();
        if backing.is_empty() || !ours {
            continue;
        }
        let device = Path::new("/dev/block").join(&name);
        let Ok(device_number) = devnum(&device) else { continue };
        for point in mount::points_of(device_number).with_context(|| format!("find what {name} is mounted at"))? {
            if mount::umount(&point).is_ok() {
                continue;
            }
            // Programs still run from the loop's filesystem: the recovery's
            // services map the installed system's linker from its APEX.
            // Stop them, then unmount again, so the loop and the partition's
            // device-mapper device can go before the updater rewrites it.
            stop_users(&point);
            let mut gone = false;
            for _ in 0..20 {
                match mount::umount(&point) {
                    Ok(()) => {
                        gone = true;
                        break;
                    }
                    Err(e) if is_busy(&e) => {
                        stop_users(&point);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(e) => return Err(e).with_context(|| format!("unmount {} (holds {backing})", point.display())),
                }
            }
            if !gone {
                // Some program could not be stopped; leave the mount out of
                // the tree and the partition mapped, as before.
                mount::umount_detach(&point)
                    .with_context(|| format!("detach {} (holds {backing}, and a program runs from it)", point.display()))?;
                busy = true;
            }
        }
        // A loop the kernel would not clear itself keeps holding the file.
        if let Ok(file) = fs::File::open(&device) {
            let _ = unsafe { libc::ioctl(std::os::fd::AsRawFd::as_raw_fd(&file), LOOP_CLR_FD as _, 0) };
        }
        // A program still running from the loop's filesystem keeps the
        // loop, and so the file on the partition, in use.
        if Path::new("/sys/block").join(&name).exists() {
            busy = true;
        }
    }
    Ok(busy)
}

/// Whether an unmount failed because something still uses the mount.
fn is_busy(e: &anyhow::Error) -> bool {
    e.root_cause().downcast_ref::<std::io::Error>().is_some_and(|e| e.raw_os_error() == Some(libc::EBUSY))
}

/// Stop every process that has a file mapped or open from `mount_point`,
/// except init and this process. A recovery's services run the installed
/// system's linker from its APEX and keep files from its partitions open;
/// they have to go before those can be unmounted and the partition written.
fn stop_users(mount_point: &Path) {
    let prefix = format!("{}/", mount_point.display());
    let me = std::process::id();
    let Ok(entries) = fs::read_dir("/proc") else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if pid == 1 || pid == me {
            continue;
        }
        let proc = entry.path();
        let mapped = fs::read_to_string(proc.join("maps")).is_ok_and(|maps| {
            maps.lines().any(|line| line.split_whitespace().last().is_some_and(|path| path.starts_with(&prefix)))
        });
        let opened = !mapped
            && fs::read_dir(proc.join("fd")).is_ok_and(|fds| {
                fds.flatten().any(|fd| {
                    fs::read_link(fd.path()).is_ok_and(|target| target.to_string_lossy().starts_with(&prefix))
                })
            });
        if mapped || opened {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
    }
}

/// Undo `mount_partitions`: unmount the slot's partitions where their own
/// device is what shows, remove their device-mapper devices, and unmount
/// /metadata.
pub fn unmount_partitions(slot: Slot) -> Result<()> {
    let md = skyward_lp::read(&super_device(), slot.number())?;
    for p in slot_partitions(&md, slot).into_iter().rev() {
        unmap(&p.name, &mount_point(base(p, slot)))?;
    }
    let metadata = Path::new(METADATA);
    let metadata_dev = devnum(Path::new(BY_NAME).join("metadata"))?;
    if mount::visible(metadata)?.is_some_and(|v| v.device == metadata_dev) {
        mount::umount(metadata)?;
    }
    Ok(())
}

//! The super partition of an SM-G9980, dumped from the phone: an A-only
//! Samsung whose partitions are its own, not one per slot.

use skyward_lp::{read, Extent, Geometry, ATTR_READONLY, HEADER_FLAG_OVERLAYS_ACTIVE};
use std::path::{Path, PathBuf};

/// The first 274432 bytes of the super partition: both geometry copies and
/// the two metadata slots with their backups.
fn fixture() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/p3q-super-metadata.bin"))
}

fn damaged(name: &str, offsets: &[usize]) -> PathBuf {
    let mut data = std::fs::read(fixture()).unwrap();
    for &at in offsets {
        data[at] ^= 0xff;
    }
    let path = std::env::temp_dir().join(format!("skyward-lp-{name}-{}", std::process::id()));
    std::fs::write(&path, data).unwrap();
    path
}

const PRIMARY_GEOMETRY: usize = 4096;
const BACKUP_GEOMETRY: usize = 8192;

#[test]
fn reads_the_partitions_samsung_ships() {
    let md = read(fixture(), 0).unwrap();
    assert_eq!(md.geometry, Geometry { metadata_max_size: 65536, metadata_slot_count: 2, logical_block_size: 4096 });
    assert_eq!((md.major_version, md.minor_version), (10, 2));
    assert_eq!(md.header_flags, HEADER_FLAG_OVERLAYS_ACTIVE);
    assert!(md.damaged.is_empty());

    let names: Vec<&str> = md.partitions.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["system", "vendor", "product", "odm"], "an A-only phone names no slot");
    let groups: Vec<&str> = md.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(groups, ["default", "qti_dynamic_partitions"]);
    assert_eq!(md.groups[1].maximum_size, 12_570_329_088);

    let super_dev = &md.block_devices[0];
    assert_eq!(super_dev.partition_name, "super");
    assert_eq!((super_dev.first_logical_sector, super_dev.size), (2048, 12_574_523_392));

    let system = md.partition("system").unwrap();
    assert_eq!(system.attributes, ATTR_READONLY);
    assert_eq!(system.bytes(), 6_765_576_192);
    assert_eq!(md.partition("odm").unwrap().bytes(), 618_496);
    assert_eq!(
        md.partition("product").unwrap().extents,
        [Extent::Linear { sectors: 1_386_496, device: 0, sector: 15_992_832 }, Extent::Linear { sectors: 1_989_504, device: 0, sector: 17_381_376 }]
    );
}

#[test]
fn a_slot_the_super_does_not_keep_is_refused() {
    let err = read(fixture(), 3).unwrap_err();
    assert!(format!("{err:#}").contains("slot 3"));
}

#[test]
fn a_damaged_primary_geometry_is_passed_over_for_the_backup() {
    let path = damaged("geometry", &[PRIMARY_GEOMETRY + 10]);
    let md = read(&path, 0).unwrap();
    assert_eq!(md.geometry.metadata_slot_count, 2);
    let _ = BACKUP_GEOMETRY;
}

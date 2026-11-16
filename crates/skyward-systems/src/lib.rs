//! A second system beside the one installed: its root in a fixed area of
//! the data partition, and the boot sets of both, the kernel and ramdisk
//! the bootloader starts, kept in a store area of their own. Switching
//! writes one or the other into the boot and init_boot partitions of the
//! slot that starts next; nothing else of the phone changes.

use std::collections::HashMap;

pub mod boot;
pub mod install;
pub mod package;
pub mod state;
pub mod store;
pub mod switch;

pub use state::{State, Who};
pub use switch::Held;

/// What the pages show of the systems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    /// The system installed in the phone's own partitions, as it names
    /// itself.
    pub installed: Named,
    pub other: Option<Other>,
    /// The other system's boot set is in the boot partitions, so it is the
    /// one that starts.
    pub other_starts: bool,
    /// The installed system's kernel side has changed since the other
    /// system was installed: its vendor_boot or its dtbo, which the other
    /// system starts with too.
    pub changed: bool,
    /// What the boot partitions hold, set against what is kept of them.
    pub held: Held,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Other {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The bytes its root takes in the data partition.
    pub size: u64,
}

/// What the installed system calls itself, by its properties: One UI by the
/// version Samsung gives it, any other by its Android version; with the
/// build it is, where it says.
pub fn installed_name(props: &HashMap<String, String>, version_prop: &str) -> Named {
    let name = match props.get("ro.build.version.oneui").and_then(|code| code.parse::<u32>().ok()) {
        Some(code) => {
            let (major, minor, patch) = (code / 10000, (code / 100) % 100, code % 100);
            match patch {
                0 => format!("One UI {major}.{minor}"),
                _ => format!("One UI {major}.{minor}.{patch}"),
            }
        }
        None => match props.get("ro.build.version.release") {
            Some(release) => format!("Android {release}"),
            None => "Android".into(),
        },
    };
    let version = props.get(version_prop).or_else(|| props.get("ro.build.display.id")).cloned();
    Named { name, version }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_installed_system_is_named_as_it_names_itself() {
        let oneui = props(&[("ro.build.version.oneui", "60100"), ("ro.build.version.release", "14")]);
        assert_eq!(installed_name(&oneui, "ro.build.display.id"), Named { name: "One UI 6.1".into(), version: None });
        let patched = props(&[("ro.build.version.oneui", "60110"), ("ro.build.version.release", "14")]);
        assert_eq!(installed_name(&patched, "ro.build.display.id").name, "One UI 6.1.10");
        let custom = props(&[("ro.build.version.release", "16"), ("ro.build.display.id", "p3q-userdebug 16 BP2A")]);
        assert_eq!(
            installed_name(&custom, "ro.build.display.id"),
            Named { name: "Android 16".into(), version: Some("p3q-userdebug 16 BP2A".into()) }
        );
    }
}

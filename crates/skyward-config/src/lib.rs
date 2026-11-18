use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Recovery ramdisk files Skyward reads, adds or changes, as archive paths
/// (they sit under `/` once the kernel unpacks the ramdisk).
pub mod ramdisk {
    /// The device configuration, copied from `devices/<codename>.toml`.
    pub const CONFIG: &str = "skyward/device.toml";
    /// The UI typeface.
    pub const FONT: &str = "skyward/MiSansVF.ttf";
    /// The terminal's monospaced typeface.
    pub const MONO: &str = "skyward/KirkMono-Regular.ttf";
    /// The monospaced typeface's licence, which travels with every copy of
    /// it.
    pub const MONO_LICENSE: &str = "skyward/KirkMono-OFL.txt";
    /// The recovery service binary init starts.
    pub const RECOVERY: &str = "system/bin/recovery";
    /// The first program of a system installed beside the installed one,
    /// which Skyward puts in that system's init_boot image: linked
    /// statically, since it runs before any system's libraries are there.
    pub const SYSTEM_INIT: &str = "skyward/system-init";
    /// The commands install scripts written for other recoveries run that
    /// Skyward answers itself: 7za, avbctl, resetprop and zstd, which a
    /// stock recovery does not carry, and unzip, whose stock version holds
    /// all that `-p` takes out in memory. Each is a link to the recovery
    /// service binary beside it, which runs the command it is started as.
    pub const COMMANDS: [&str; 5] =
        ["system/bin/7za", "system/bin/avbctl", "system/bin/resetprop", "system/bin/unzip", "system/bin/zstd"];
    /// The first property file init loads in recovery mode.
    pub const PROP_DEFAULT: &str = "prop.default";
    /// The scheduling profiles libprocessgroup reads.
    pub const TASK_PROFILES: &str = "system/etc/task_profiles.json";
    /// The init rules that set up file transfer's USB function.
    pub const MTP_RC: &str = "system/etc/init/skyward-mtp.rc";

    /// Whether the running root is Skyward's recovery ramdisk.
    pub fn in_recovery() -> bool {
        std::path::Path::new("/").join(CONFIG).exists()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceConfig {
    pub device: Device,
    #[serde(default)]
    pub display: Display,
    #[serde(default)]
    pub modules: Modules,
    #[serde(default)]
    pub touch: Touch,
    #[serde(default)]
    pub partitions: Partitions,
    /// Where the firmware the phone's drivers ask for is, on phones whose
    /// recovery leaves it out of their reach.
    #[serde(default)]
    pub firmware: Option<FirmwarePartition>,
    #[serde(default, rename = "service")]
    pub services: Vec<Service>,
    /// The vibration motor's click, on phones that have one.
    #[serde(default)]
    pub haptics: Option<Haptics>,
    /// The recovery's own logd, on phones whose recovery leaves out
    /// libraries it needs.
    #[serde(default)]
    pub logd: Option<Logd>,
    /// Files the phone's own init hands to an account as it boots, before
    /// the programs that open them start; a recovery runs none of init's
    /// boot actions.
    #[serde(default)]
    pub owners: Vec<Owners>,
    #[serde(default)]
    pub wipe: Wipe,
    #[serde(default)]
    pub backup: Backup,
    /// The phone's WLAN, on phones whose driver the recovery brings up.
    #[serde(default)]
    pub wlan: Option<Wlan>,
    /// The stock build the recovery is made on.
    #[serde(default)]
    pub stock: Option<Stock>,
}

/// The stock build a recovery is made on: the phone maker's full update
/// package of it, whose recovery partition the recovery is built on top
/// of, and that partition as it should come out of the package.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Stock {
    /// The build's version, such as G9980ZHSGHZB1.
    pub version: String,
    /// Where the package is downloaded from, over HTTPS.
    pub package: String,
    /// The recovery partition's SHA-256, in lowercase hex.
    pub recovery: String,
}

/// The place an entry in a WLAN's `libraries` holds for the folder of the
/// libraries of the vendor interface the phone was built against.
pub const BUNDLE_PLACEHOLDER: &str = "$bundle";

/// What brings the phone's WLAN up inside a recovery, which loads none of
/// it: the driver, what it reads as it starts, and the phone's own
/// wpa_supplicant, which joins networks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Wlan {
    /// The driver's kernel modules; what they depend on loads first.
    pub modules: Vec<String>,
    /// What is written once they are in, each value to its file, such as
    /// what tells a platform driver it may start the chip.
    #[serde(default)]
    pub writes: Vec<Write>,
    /// Files the driver asks the kernel's firmware loader for that are not
    /// where the loader looks in a recovery.
    #[serde(default)]
    pub firmware: Vec<Firmware>,
    /// The network interface the driver makes.
    pub interface: String,
    /// The phone's own wpa_supplicant.
    pub supplicant: String,
    /// Directories for its LD_LIBRARY_PATH, searched in the order listed.
    /// The entry `$bundle` stands for the folder of the libraries of the
    /// vendor interface the phone was built against, where in the order it
    /// is to be searched: a phone whose own programs need the recovery's
    /// libraries to win lists them before it, and one whose programs need
    /// the folder to win lists it there. When no entry names it, it is
    /// searched last.
    #[serde(default)]
    pub libraries: Vec<String>,
    /// Libraries to load ahead of the ones the program asks for, each by
    /// path, such as a newer C++ runtime than the bundle carries, where
    /// what the recovery carries of the system's logs is built against it.
    #[serde(default)]
    pub preload: Vec<String>,
    /// The service it registers with the service directory as it starts,
    /// which the directory takes only from a program the running system
    /// describes. A phone whose supplicant registers with the directory
    /// for the older kind of hardware service names none here: that one is
    /// told of it from the phone's own descriptions.
    #[serde(default)]
    pub service: Option<String>,
    /// The page the stock system asks over plain HTTP to tell whether a
    /// network reaches the internet, which answers 204 with nothing.
    pub probe: String,
}

/// A value written to a file, such as a switch in sysfs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Write {
    pub path: String,
    pub value: String,
}

/// The partition holding the firmware images of a phone's processors and
/// radios, which their drivers ask the kernel's firmware loader for, and of
/// the apps its secure world runs. It is mounted where and as the phone's
/// own table mounts it, and shown as well at the folder whose image folder
/// the recovery's own ueventd looks in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FirmwarePartition {
    /// The partition's name, without the slot it is of.
    pub partition: String,
}

/// A file a driver asks the firmware loader for, and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Firmware {
    /// The name the driver asks for, from the top of the loader's folders.
    pub name: String,
    /// Where the file is: a path in the running recovery, or from the top
    /// of `partition`.
    pub path: String,
    /// A partition the recovery does not keep mounted, which is mounted
    /// read-only while the file is read.
    #[serde(default)]
    pub partition: Option<String>,
}

/// What a backup takes in and leaves out beyond what every Android phone
/// keeps the same.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Backup {
    /// The partitions that keep the baseband's settings, with the phone's
    /// own identity, and the sensors' calibration.
    #[serde(default)]
    pub efs: Vec<String>,
    /// Folders in the data partition, from its top, that the phone's own
    /// secure world keeps in step with itself, as its own storage and the
    /// fingerprint templates: an older copy written back would not open.
    /// A `*` stands for any one name.
    #[serde(default)]
    pub data_skipped: Vec<String>,
}

/// What erasing on the wipe page takes in beyond the places every Android
/// phone keeps the same.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Wipe {
    /// Folders in the data partition where the phone's own programs keep
    /// their logs and dumps: their files go with the logs and crash
    /// records, the folders staying.
    #[serde(default)]
    pub logs: Vec<String>,
}

/// The data partition, which the folders erasing takes in are in.
const DATA: &str = "/data";

/// The vibration motor, played through the kernel's force-feedback events:
/// its click, a waveform its driver keeps or one made from samples at the
/// motor's resonant frequency, which the driver plays as they are given,
/// and the ticks of the screen going off by the power key, made too.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Haptics {
    /// The driver's number for a click waveform it keeps, on phones whose
    /// driver plays the waveforms it keeps.
    #[serde(default)]
    pub click: Option<i16>,
    /// The file the driver says the motor's resonant frequency in, in
    /// tenths of a hertz whatever its label; given where the click is made
    /// from samples.
    #[serde(default)]
    pub resonance: Option<String>,
    pub strength: Strength,
    /// The files a driver is told how long to play, on phones whose motor
    /// the kernel plays from sysfs rather than through force-feedback.
    #[serde(default)]
    pub timed: Option<Timed>,
}

/// A motor the kernel plays for a length written to one file, as some
/// makers' haptics drivers do.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Timed {
    /// The file the length in milliseconds is written to.
    pub duration: String,
    /// The file written to play it, where the driver asks for one.
    #[serde(default)]
    pub activate: Option<String>,
    /// The file the strength is written to, where the driver takes one.
    #[serde(default)]
    pub intensity: Option<String>,
    /// The value the strength file takes as the strongest.
    #[serde(default)]
    pub intensity_max: Option<u32>,
    /// How long a click plays, and how long the ticks of the screen going
    /// off play, in milliseconds.
    pub click: u32,
    pub ticks: u32,
}

/// How strongly the click plays, each as a share of the most its waveform
/// drives the motor with. The strength slider goes from the lightest to the
/// strongest, each step the same factor stronger.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Strength {
    pub lightest: f32,
    pub strongest: f32,
}

/// What the recovery's own logd, which keeps what programs log for
/// logcat, needs from the installed system before it can start.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Logd {
    /// Libraries of the installed system it loads that the recovery's own
    /// library folder lacks, put there before it is started.
    pub libraries: Vec<String>,
}

/// Files the phone's own init hands to one account as it boots, each given
/// the same mode, as a run of its chown and chmod lines does: device nodes,
/// and the kernel's switches that the phone's own programs open.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Owners {
    pub user: String,
    pub group: String,
    /// The mode init's chmod lines give them, written as an octal number.
    pub mode: u32,
    pub paths: Vec<String>,
}

/// A name as each language the pages speak writes it: Chinese in
/// simplified and in traditional characters, and English.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Names {
    pub hans: String,
    pub hant: String,
    pub en: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Device {
    pub codename: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_true")]
    pub ab: bool,
    /// SoC marketing name shown on the About page.
    #[serde(default)]
    pub soc: Option<Names>,
    /// System property that carries the vendor OS version.
    #[serde(default = "default_os_version_prop")]
    pub os_version_prop: String,
    /// The locale the pages speak on a first start, where the system asks for
    /// none and the installed system names none.
    #[serde(default)]
    pub lang: Option<String>,
    /// The recovery partition's size in bytes, where the base recovery image
    /// is not the whole partition the phone holds it in.
    #[serde(default)]
    pub recovery_size: Option<u64>,
    /// Files the packer leaves out of the ramdisk, named as the ramdisk holds
    /// them: a third-party recovery's own program and theme are replaced by
    /// Skyward, and the partition may have no room for what only they used.
    #[serde(default)]
    pub drop: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Display {
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub density: u32,
    /// Where the brightness slider stands on a first start, from 0 to 1; a
    /// dark panel on a panel whose range starts high needs more than the
    /// default.
    #[serde(default)]
    pub brightness: Option<f32>,
    /// Marketed screen diagonal in inches.
    #[serde(default)]
    pub diagonal: Option<f32>,
    /// How round the screen's corners are, as a radius in pixels; a page
    /// sliding over another rounds its leading corners to it.
    #[serde(default)]
    pub corner_radius: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Modules {
    /// Partitions the phone keeps its kernel modules on, searched in order.
    #[serde(default = "default_module_sources")]
    pub sources: Vec<String>,
    /// Modules the recovery loads at startup that first-stage init leaves
    /// out, such as the touchscreen driver. Dependencies come from the
    /// source's modules.dep and load first.
    #[serde(default)]
    pub load: Vec<String>,
}

impl Default for Modules {
    fn default() -> Self {
        Self { sources: default_module_sources(), load: Vec::new() }
    }
}

/// The touchscreen, on phones whose touch controller is to be told
/// something before a recovery can read touches from it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Touch {
    /// What is written once the touchscreen's input device has appeared,
    /// each value to its file: the driver makes that device as the last
    /// step of starting the controller.
    #[serde(default)]
    pub writes: Vec<Write>,
}

/// Partitions as the phone itself mounts them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Partitions {
    /// Partitions the phone's own programs and drivers use where its own
    /// table mounts them, mounted there as that table says before any of
    /// them starts.
    #[serde(default)]
    pub mounted: Vec<String>,
}

/// A program from the phone's own partitions that the recovery keeps
/// running, such as the touch processing of a panel that hands raw frames
/// to the host.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Service {
    pub path: String,
    /// What the program is given after its own path, as init gives a
    /// service the words after its path.
    #[serde(default)]
    pub args: Vec<String>,
    /// Directories for LD_LIBRARY_PATH, searched in order.
    #[serde(default)]
    pub libraries: Vec<String>,
    /// The program runs once, to its end, as a service of init's marked
    /// oneshot does: it is not started again, the programs after it start
    /// once it has ended, and it has to end well.
    #[serde(default)]
    pub oneshot: bool,
    /// A property the program sets once it is ready for the rest. Programs
    /// are started in the order they are written, and one that says it has
    /// something to wait for holds up the ones after it.
    #[serde(default)]
    pub ready_property: Option<String>,
    /// A service the program registers with the phone's service directory
    /// once it is ready.
    #[serde(default)]
    pub ready_service: Option<String>,
    /// Services the program registers besides that one, which the
    /// directory takes only when they are described to it as well.
    #[serde(default)]
    pub registers: Vec<String>,
    /// A service the program registers with the other directory, the one
    /// the phone's hardware services of the older kind use. It is written
    /// as the interface and then which of them it is.
    #[serde(default)]
    pub ready_hardware_service: Option<String>,
    /// Where one model's hardware comes in more than one build, the
    /// installed system's own properties say which build it is: the program
    /// runs only where the property has this value.
    #[serde(default)]
    pub when: Option<Property>,
    /// The user the program runs as, as the phone's own init runs it, by
    /// the name Android gives the account; root where none is given. What
    /// the program makes in the data partition is then that user's, as it
    /// is when the phone runs it, and the phone's own copy of the program
    /// can still read and remove it.
    #[serde(default)]
    pub user: Option<String>,
    /// Its group and then the others it is in, as init's group line names
    /// them.
    #[serde(default)]
    pub groups: Vec<String>,
    /// What it may do of what only root may, as init's capabilities line
    /// names it: it keeps these alone, through its change of user.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// The kernel's capabilities, by the names init's capabilities lines give
/// them, in the order the kernel numbers them.
const CAPABILITIES: [&str; 41] = [
    "CHOWN",
    "DAC_OVERRIDE",
    "DAC_READ_SEARCH",
    "FOWNER",
    "FSETID",
    "KILL",
    "SETGID",
    "SETUID",
    "SETPCAP",
    "LINUX_IMMUTABLE",
    "NET_BIND_SERVICE",
    "NET_BROADCAST",
    "NET_ADMIN",
    "NET_RAW",
    "IPC_LOCK",
    "IPC_OWNER",
    "SYS_MODULE",
    "SYS_RAWIO",
    "SYS_CHROOT",
    "SYS_PTRACE",
    "SYS_PACCT",
    "SYS_ADMIN",
    "SYS_BOOT",
    "SYS_NICE",
    "SYS_RESOURCE",
    "SYS_TIME",
    "SYS_TTY_CONFIG",
    "MKNOD",
    "LEASE",
    "AUDIT_WRITE",
    "AUDIT_CONTROL",
    "SETFCAP",
    "MAC_OVERRIDE",
    "MAC_ADMIN",
    "SYSLOG",
    "WAKE_ALARM",
    "BLOCK_SUSPEND",
    "AUDIT_READ",
    "PERFMON",
    "BPF",
    "CHECKPOINT_RESTORE",
];

/// The kernel's number for a capability, by the name init's capabilities
/// line gives it.
pub fn capability(name: &str) -> Option<u32> {
    CAPABILITIES.iter().position(|c| *c == name).map(|n| n as u32)
}

/// A property of the installed system, and a value it may have.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Property {
    pub property: String,
    pub value: String,
}

impl Service {
    /// Whether the program, or a directory it loads libraries from, is
    /// under `dir`: without what is mounted there it cannot start again.
    pub fn uses(&self, dir: &Path) -> bool {
        Path::new(&self.path).starts_with(dir) || self.libraries.iter().any(|l| Path::new(l).starts_with(dir))
    }

    /// Whether the program is this phone's to run, by the installed
    /// system's own properties.
    pub fn runs_on(&self, props: &HashMap<String, String>) -> bool {
        self.when.as_ref().is_none_or(|p| props.get(&p.property) == Some(&p.value))
    }
}

fn default_true() -> bool {
    true
}

fn default_os_version_prop() -> String {
    "ro.build.display.id".into()
}

fn default_module_sources() -> Vec<String> {
    vec!["vendor_boot".into(), "vendor_dlkm".into(), "vendor".into()]
}

/// The phone a device configuration is for, read on its own: the
/// configuration a newer build of Skyward carries may hold what this build
/// does not read.
pub fn codename(text: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Named {
        device: NamedDevice,
    }
    #[derive(Deserialize)]
    struct NamedDevice {
        codename: String,
    }
    let named: Named = toml::from_str(text).context("parse device config")?;
    Ok(named.device.codename)
}

impl DeviceConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref();
        let text = std::fs::read_to_string(p)
            .with_context(|| format!("read config {}", p.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let cfg: DeviceConfig = toml::from_str(text).context("parse device config")?;
        if let Some(size) = cfg.device.recovery_size {
            ensure!(size > 0, "[device] recovery_size: 0 is not a partition's size");
        }
        for log in &cfg.wipe.logs {
            let path = Path::new(log);
            let plain = path.components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)));
            ensure!(
                path.is_absolute() && plain && path.starts_with(DATA) && path != Path::new(DATA),
                "[wipe] logs: {log} is not a folder inside {DATA}"
            );
        }
        for name in &cfg.backup.efs {
            ensure!(!name.is_empty() && !name.contains('/'), "[backup] efs: {name} is not a partition's name");
        }
        if let Some(firmware) = &cfg.firmware {
            let name = &firmware.partition;
            ensure!(!name.is_empty() && !name.contains('/'), "[firmware] partition: {name} is not a partition's name");
        }
        for name in &cfg.partitions.mounted {
            ensure!(!name.is_empty() && !name.contains('/'), "[partitions] mounted: {name} is not a partition's name");
        }
        for s in cfg.services.iter().filter(|s| s.oneshot) {
            let waits = s.ready_property.is_some() || s.ready_service.is_some() || s.ready_hardware_service.is_some();
            ensure!(
                !waits && s.registers.is_empty(),
                "[[service]] {}: a program run once is ready when it ends, and registers nothing",
                s.path
            );
        }
        for s in &cfg.services {
            ensure!(s.user.is_none() || !s.groups.is_empty(), "[[service]] {}: a program given a user is given its group too", s.path);
            for name in s.user.iter().chain(&s.groups) {
                ensure!(!name.is_empty() && !name.contains(char::is_whitespace), "[[service]] {}: {name:?} is not an account's name", s.path);
            }
            for name in &s.capabilities {
                ensure!(capability(name).is_some(), "[[service]] {}: {name} is not a capability init knows", s.path);
            }
        }
        for folder in &cfg.backup.data_skipped {
            let path = Path::new(folder);
            let plain = path.components().all(|c| matches!(c, std::path::Component::Normal(_)));
            ensure!(plain && !folder.is_empty(), "[backup] data_skipped: {folder} is not a path from the top of {DATA}");
        }
        if let Some(haptics) = &cfg.haptics {
            ensure!(
                haptics.timed.is_some() || haptics.resonance.is_some(),
                "[haptics] names neither the file a motor is played by nor the one its frequency is in"
            );
            if let Some(file) = &haptics.resonance {
                ensure!(Path::new(file).is_absolute(), "[haptics] resonance: {file} is not an absolute path");
            }
            if let Some(timed) = &haptics.timed {
                for file in [Some(&timed.duration), timed.activate.as_ref(), timed.intensity.as_ref()].into_iter().flatten() {
                    ensure!(Path::new(file).is_absolute(), "[haptics] timed: {file} is not an absolute path");
                }
                ensure!(timed.click > 0 && timed.ticks > 0, "[haptics] timed: a click or the ticks play for no time at all");
                ensure!(
                    timed.intensity.is_none() || timed.intensity_max.is_some_and(|m| m > 0),
                    "[haptics] timed: a strength file needs the value it takes as the strongest"
                );
            }
            let Strength { lightest, strongest } = haptics.strength;
            ensure!(
                0.0 < lightest && lightest < strongest && strongest <= 1.0,
                "[haptics] strength: {lightest} and {strongest} are not shares rising from above 0 to at most 1"
            );
        }
        if let Some(logd) = &cfg.logd {
            for library in &logd.libraries {
                let path = Path::new(library);
                let plain = path.components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)));
                ensure!(path.is_absolute() && plain && path.file_name().is_some(), "[logd] libraries: {library} is not an absolute path to a file");
            }
        }
        for owners in &cfg.owners {
            for name in [&owners.user, &owners.group] {
                ensure!(!name.is_empty() && !name.contains(char::is_whitespace), "[[owners]] {name:?} is not an account's name");
            }
            ensure!(owners.mode <= 0o7777, "[[owners]] mode {:o} is not a file's mode", owners.mode);
            ensure!(!owners.paths.is_empty(), "[[owners]] for {}:{} names no file", owners.user, owners.group);
            for file in &owners.paths {
                let path = Path::new(file);
                let plain = path.components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)));
                ensure!(path.is_absolute() && plain && path.file_name().is_some(), "[[owners]] paths: {file} is not an absolute path to a file");
            }
        }
        if let Some(stock) = &cfg.stock {
            let name = stock.package.rsplit('/').next().unwrap_or_default();
            ensure!(
                stock.package.starts_with("https://") && name.ends_with(".zip") && name.contains(&stock.version) && !stock.version.is_empty(),
                "[stock] package: {} is not an HTTPS address of the full package of {}",
                stock.package,
                stock.version
            );
            ensure!(
                stock.recovery.len() == 64 && stock.recovery.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "[stock] recovery: {} is not a SHA-256 in lowercase hex",
                stock.recovery
            );
        }
        if let Some(wlan) = &cfg.wlan {
            ensure!(wlan.probe.starts_with("http://"), "[wlan] probe: {} is not a plain HTTP address", wlan.probe);
            for dir in &wlan.libraries {
                ensure!(
                    dir == BUNDLE_PLACEHOLDER || Path::new(dir).is_absolute(),
                    "[wlan] libraries: {dir} is neither an absolute path nor {BUNDLE_PLACEHOLDER}"
                );
            }
            let named = wlan.libraries.iter().filter(|dir| *dir == BUNDLE_PLACEHOLDER).count();
            ensure!(named <= 1, "[wlan] libraries names {BUNDLE_PLACEHOLDER} {named} times");
            for lib in &wlan.preload {
                ensure!(Path::new(lib).is_absolute(), "[wlan] preload: {lib} is not an absolute path");
            }
            for file in &wlan.firmware {
                let name = Path::new(&file.name);
                let plain = name.components().all(|c| matches!(c, std::path::Component::Normal(_)));
                ensure!(plain && !file.name.is_empty(), "[wlan] firmware: {} is not a name from the top of the loader's folders", file.name);
                let path = Path::new(&file.path);
                match &file.partition {
                    Some(partition) => {
                        let plain = path.components().all(|c| matches!(c, std::path::Component::Normal(_)));
                        ensure!(
                            !partition.is_empty() && !partition.contains('/'),
                            "[wlan] firmware: {partition} is not a partition's name"
                        );
                        ensure!(plain && !file.path.is_empty(), "[wlan] firmware: {} is not a path from the top of {partition}", file.path);
                    }
                    None => ensure!(path.is_absolute(), "[wlan] firmware: {} is not an absolute path", file.path),
                }
            }
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_sit_beside_the_recovery_service_binary() {
        let folder = |path: &str| Path::new(path).parent().map(Path::to_path_buf);
        for command in ramdisk::COMMANDS {
            assert_eq!(folder(command), folder(ramdisk::RECOVERY), "{command}");
        }
    }

    #[test]
    fn parses_minimal() {
        let cfg = DeviceConfig::parse("[device]\ncodename = \"p3q\"\n").unwrap();
        assert_eq!(cfg.device.codename, "p3q");
        assert!(cfg.device.ab);
        assert_eq!(cfg.device.os_version_prop, "ro.build.display.id");
        assert_eq!(cfg.modules.sources, default_module_sources());
        assert!(cfg.modules.load.is_empty());
        assert!(cfg.firmware.is_none() && cfg.wlan.is_none(), "a phone may have none of them");
        assert_eq!(cfg.logd, None, "a recovery's own logd usually has all it needs");
    }

    #[test]
    fn parses_p3q() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/p3q.toml")).unwrap();
        assert_eq!(cfg.device.codename, "p3q");
        assert_eq!(cfg.device.os_version_prop, "ro.build.display.id");
        assert!(!cfg.device.ab, "Samsung does not use A/B updates");
        assert_eq!((cfg.display.width, cfg.display.height, cfg.display.density), (1440, 3200, 500));
        assert_eq!(cfg.display.corner_radius, 60);
        assert_eq!(cfg.device.recovery_size, None, "the base image is the whole partition");
        let wlan = cfg.wlan.expect("the S21 Ultra's WLAN is described");
        assert_eq!(wlan.modules, ["dhd"]);
        assert_eq!(wlan.interface, "wlan0");
        assert_eq!(wlan.supplicant, "/vendor/bin/hw/wpa_supplicant");
        assert_eq!(wlan.libraries[0], "/system/wifi-vndk/lib64");
        assert!(wlan.preload.is_empty(), "the vendor's own libc++ is the one its programs need");
        let haptics = cfg.haptics.expect("the S21 Ultra has a motor");
        assert_eq!(haptics.strength, Strength { lightest: 0.2, strongest: 1.0 });
        assert_eq!(cfg.backup.efs, ["efs", "sec_efs", "modemst1", "modemst2", "fsg", "fsc"]);
        assert!(cfg.wipe.logs.contains(&"/data/vendor/wifi".to_string()));
    }

    #[test]
    fn parses_y2q() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/y2q.toml")).unwrap();
        assert_eq!(cfg.device.codename, "y2q");
        assert_eq!(cfg.device.recovery_size, Some(82_694_144), "the Fox base is smaller than the recovery partition");
        assert_eq!(cfg.display.density, 375);
        assert_eq!(cfg.display.diagonal, Some(6.7));
        assert_eq!(cfg.display.brightness, Some(0.5));
        let wlan = cfg.wlan.expect("the S20+'s WLAN is described");
        assert_eq!(wlan.libraries[1], "$bundle", "the vendor interface's libraries sit where the phone needs them");
        assert_eq!(wlan.preload, ["/system/lib64/libc++.so"]);
        assert_eq!(cfg.touch.writes[0].path, "/sys/class/sec/tsp/cmd");
        assert_eq!(cfg.touch.writes[0].value, "set_scan_rate,1,2");
    }

    #[test]
    fn a_click_is_kept_or_made_and_its_strengths_rise() {
        let with = |body: &str| DeviceConfig::parse(&format!("[device]\ncodename = \"x\"\n[haptics]\n{body}\n"));
        let (strength, resonance) = ("strength = { lightest = 0.25, strongest = 1.0 }", "resonance = \"/sys/class/qcom-haptics/lra_frequency_hz\"");
        assert!(with(&format!("{resonance}\n{strength}")).is_ok(), "a click made");
        assert!(with(&format!("click = 0\n{resonance}\n{strength}")).is_ok(), "a click kept");
        assert!(with(&format!("click = 0\n{strength}")).is_err(), "no motor's resonance to make the ticks at");
        assert!(with(&format!("resonance = \"sys/f\"\n{strength}")).is_err(), "a file named from nowhere");
        assert!(with(resonance).is_err(), "a click without its strengths");
        for (lightest, strongest) in [(0.0, 0.5), (0.5, 0.5), (0.5, 0.25), (0.25, 1.5)] {
            let s = format!("{resonance}\nstrength = {{ lightest = {lightest:?}, strongest = {strongest:?} }}");
            assert!(with(&s).is_err(), "{lightest} to {strongest} is refused");
        }
    }

    #[test]
    fn a_library_for_logd_is_an_absolute_path_to_a_file() {
        let with = |library: &str| DeviceConfig::parse(&format!("[device]\ncodename = \"x\"\n[logd]\nlibraries = [\"{library}\"]\n"));
        assert!(with("/mnt/system/system/lib64/libcap.so").is_ok());
        for bad in ["lib64/libcap.so", "/", "/mnt/system/../lib64/libcap.so", ""] {
            assert!(with(bad).is_err(), "{bad} is refused");
        }
    }

    #[test]
    fn files_handed_over_name_their_accounts_a_mode_and_absolute_paths() {
        let with = |body: &str| DeviceConfig::parse(&format!("[device]\ncodename = \"x\"\n[[owners]]\n{body}\n"));
        let ok = with("user = \"system\"\ngroup = \"system\"\nmode = 0o664\npaths = [\"/dev/input/event1\"]").unwrap();
        assert_eq!(ok.owners, [Owners { user: "system".into(), group: "system".into(), mode: 0o664, paths: vec!["/dev/input/event1".into()] }]);
        assert!(with("user = \"system\"\ngroup = \"system\"\nmode = 0o664\npaths = []").is_err(), "no file");
        assert!(with("user = \"sys tem\"\ngroup = \"system\"\nmode = 0o664\npaths = [\"/dev/a\"]").is_err());
        assert!(with("user = \"system\"\ngroup = \"system\"\nmode = 0o17777\npaths = [\"/dev/a\"]").is_err(), "not a file's mode");
        for bad in ["dev/a", "/", "/sys/../dev/a"] {
            assert!(with(&format!("user = \"system\"\ngroup = \"system\"\nmode = 0o664\npaths = [\"{bad}\"]")).is_err(), "{bad} is refused");
        }
        assert!(DeviceConfig::parse("[device]\ncodename = \"x\"\n").unwrap().owners.is_empty(), "a phone may need none");
    }

    #[test]
    fn a_program_runs_only_where_the_property_has_its_value() {
        let base = "[device]\ncodename = \"x\"\n[[service]]\npath = \"/odm/bin/a\"\n";
        let only = DeviceConfig::parse(&format!("{base}when = {{ property = \"ro.sku\", value = \"p\" }}\n")).unwrap();
        let always = DeviceConfig::parse(base).unwrap();
        let props = |v: Option<&str>| -> HashMap<String, String> { v.map(|v| ("ro.sku".to_string(), v.to_string())).into_iter().collect() };
        for (value, runs) in [(Some("p"), true), (Some("q"), false), (None, false)] {
            assert_eq!(only.services[0].runs_on(&props(value)), runs, "with ro.sku {value:?}");
            assert!(always.services[0].runs_on(&props(value)));
        }
    }

    #[test]
    fn a_program_run_once_waits_for_nothing_and_registers_nothing() {
        let base = "[device]\ncodename = \"x\"\n[[service]]\npath = \"/vendor/bin/sh\"\nargs = [\"/vendor/bin/a.sh\"]\noneshot = true\n";
        let once = DeviceConfig::parse(base).unwrap();
        assert!(once.services[0].oneshot);
        assert_eq!(once.services[0].args, ["/vendor/bin/a.sh"]);
        assert!(DeviceConfig::parse(&format!("{base}ready_property = \"vendor.a\"\n")).is_err());
        assert!(DeviceConfig::parse(&format!("{base}ready_service = \"a.IA/default\"\n")).is_err());
        assert!(DeviceConfig::parse(&format!("{base}registers = [\"a.IB/default\"]\n")).is_err());
        let kept = DeviceConfig::parse("[device]\ncodename = \"x\"\n[[service]]\npath = \"/vendor/bin/a\"\n").unwrap();
        assert!(!kept.services[0].oneshot && kept.services[0].args.is_empty(), "a program is kept running unless it says otherwise");
    }

    #[test]
    fn a_program_runs_as_its_user_in_its_groups_keeping_named_capabilities() {
        let base = "[device]\ncodename = \"x\"\n[[service]]\npath = \"/odm/bin/a\"\n";
        let root = DeviceConfig::parse(base).unwrap();
        let s = &root.services[0];
        assert_eq!((s.user.as_ref(), s.groups.len(), s.capabilities.len()), (None, 0, 0), "root, as init runs one that names no user");
        let named = DeviceConfig::parse(&format!("{base}user = \"system\"\ngroups = [\"system\", \"drmrpc\"]\ncapabilities = [\"WAKE_ALARM\"]\n")).unwrap();
        let s = &named.services[0];
        assert_eq!((s.user.as_deref(), &s.groups[..], &s.capabilities[..]), (Some("system"), &["system".to_string(), "drmrpc".to_string()][..], &["WAKE_ALARM".to_string()][..]));
        assert!(DeviceConfig::parse(&format!("{base}user = \"system\"\n")).is_err(), "a user without its group");
        assert!(DeviceConfig::parse(&format!("{base}user = \"system\"\ngroups = [\"system\"]\ncapabilities = [\"WAKE\"]\n")).is_err(), "no such capability");
        assert!(DeviceConfig::parse(&format!("{base}user = \"sys tem\"\ngroups = [\"system\"]\n")).is_err());
        assert_eq!(
            [capability("CHOWN"), capability("SETPCAP"), capability("WAKE_ALARM"), capability("BLOCK_SUSPEND"), capability("CHECKPOINT_RESTORE"), capability("CAP_CHOWN")],
            [Some(0), Some(8), Some(35), Some(36), Some(40), None],
            "numbered as the kernel numbers them, named as init names them"
        );
    }

    #[test]
    fn a_stock_build_is_a_full_package_over_https_and_a_sha_256() {
        let ok = "[device]\ncodename = \"x\"\n[stock]\nversion = \"G9980ZHSGHZB1\"\npackage = \"https://d.example/G9980ZHSGHZB1/p3q-ota_full-G9980ZHSGHZB1-user-16.0-0123456789.zip\"\n";
        let hash = "faa425b10231a0cfe2d74ff34f344f032ee2d891ea5e9420bf4e67cd86d622a7";
        assert!(DeviceConfig::parse(&format!("{ok}recovery = \"{hash}\"\n")).is_ok());
        assert!(DeviceConfig::parse(&format!("{ok}recovery = \"{}\"\n", hash.to_uppercase())).is_err(), "in lowercase");
        assert!(DeviceConfig::parse(&format!("{ok}recovery = \"{}\"\n", &hash[1..])).is_err(), "too short");
        let plain = ok.replace("https://", "http://");
        assert!(DeviceConfig::parse(&format!("{plain}recovery = \"{hash}\"\n")).is_err(), "over plain HTTP");
        let other = ok.replace("version = \"G9980ZHSGHZB1", "version = \"G9980ZHSGHZB2");
        assert!(DeviceConfig::parse(&format!("{other}recovery = \"{hash}\"\n")).is_err(), "the package of another build");
        assert_eq!(DeviceConfig::parse("[device]\ncodename = \"x\"\n").unwrap().stock, None);
    }

    #[test]
    fn the_firmware_partition_is_named_plainly() {
        let base = "[device]\ncodename = \"x\"\n[firmware]\n";
        assert!(DeviceConfig::parse(&format!("{base}partition = \"modem\"\n")).is_ok());
        assert!(DeviceConfig::parse(&format!("{base}partition = \"by-name/modem\"\n")).is_err());
        assert!(DeviceConfig::parse(&format!("{base}partition = \"\"\n")).is_err());
    }

    #[test]
    fn a_partition_mounted_as_the_phone_mounts_it_is_named_plainly() {
        let with = |name: &str| DeviceConfig::parse(&format!("[device]\ncodename = \"x\"\n[partitions]\nmounted = [\"{name}\"]\n"));
        assert_eq!(with("persist").unwrap().partitions.mounted, ["persist"]);
        assert!(with("by-name/persist").is_err());
        assert!(with("").is_err());
        assert!(DeviceConfig::parse("[device]\ncodename = \"x\"\n").unwrap().partitions.mounted.is_empty(), "a phone may need none");
    }

    #[test]
    fn the_phone_is_read_from_a_configuration_this_build_cannot_read_whole() {
        assert_eq!(codename(include_str!("../../../devices/p3q.toml")).unwrap(), "p3q");
        let later = "[device]\ncodename = \"x\"\nab = \"no longer a switch\"\n[display]\nwidth = \"wide\"\n[aurora]\nnew = 1\n";
        assert!(DeviceConfig::parse(later).is_err());
        assert_eq!(codename(later).unwrap(), "x");
        assert!(codename("[display]\nwidth = 1440\n").is_err(), "a configuration that names no phone");
    }

    #[test]
    fn what_a_backup_takes_in_is_named_plainly() {
        let base = "[device]\ncodename = \"x\"\n";
        assert!(DeviceConfig::parse(&format!("{base}[backup]\nefs = [\"by-name/fsg\"]\n")).is_err());
        assert!(DeviceConfig::parse(&format!("{base}[backup]\ndata_skipped = [\"/data/vendor\"]\n")).is_err());
        assert!(DeviceConfig::parse(&format!("{base}[backup]\ndata_skipped = [\"vendor/../system\"]\n")).is_err());
        assert!(DeviceConfig::parse(&format!("{base}[backup]\ndata_skipped = [\"vendor_de/*/fpdata\"]\n")).is_ok());
    }

    #[test]
    fn the_phones_own_logs_are_folders_inside_the_data_partition() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/p3q.toml")).unwrap();
        assert!(cfg.wipe.logs.contains(&"/data/vendor/wifi".to_string()));
        assert!(DeviceConfig::parse("[device]\ncodename = \"x\"\n").unwrap().wipe.logs.is_empty(), "a phone may have none");
        for bad in ["/data", "/data/", "/metadata/logs", "data/log", "/data/../system"] {
            let text = format!("[device]\ncodename = \"x\"\n[wipe]\nlogs = [\"{bad}\"]\n");
            assert!(DeviceConfig::parse(&text).is_err(), "{bad} is refused");
        }
    }

    #[test]
    fn p3q_brings_its_wlan_up_from_the_ramdisk() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/p3q.toml")).unwrap();
        let wlan = cfg.wlan.expect("the S21 Ultra's WLAN is described");
        assert_eq!(wlan.modules, ["dhd"]);
        assert_eq!(wlan.interface, "wlan0");
        assert_eq!(wlan.supplicant, "/vendor/bin/hw/wpa_supplicant");
        assert_eq!(wlan.libraries[0], "/system/wifi-vndk/lib64", "the VNDK libraries the vendor leaves out come first");
        assert!(wlan.preload.is_empty(), "the vendor's own libc++ is the one its programs need");
        assert_eq!(wlan.service, None, "the supplicant registers with the older directory itself");
        assert_eq!(wlan.probe, "http://connectivitycheck.gstatic.com/generate_204");
    }

    #[test]
    fn y2q_brings_its_wlan_up_from_the_kernels_own_driver() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/y2q.toml")).unwrap();
        let wlan = cfg.wlan.expect("the S20+'s WLAN is described");
        assert!(wlan.modules.is_empty(), "the driver is in the kernel");
        assert_eq!(wlan.writes[0].path, "/sys/module/dhd/parameters/firmware_path");
        assert_eq!(wlan.writes[1].path, "/sys/module/dhd/parameters/nvram_path");
        assert_eq!(wlan.libraries[1], "$bundle", "the vendor interface's libraries sit where the phone needs them");
        assert_eq!(wlan.preload, ["/system/lib64/libc++.so"]);
        assert_eq!(wlan.probe, "http://connectivitycheck.gstatic.com/generate_204");
    }

    #[test]
    fn a_wlan_is_probed_over_plain_http() {
        let base = "[device]\ncodename = \"x\"\n[wlan]\nmodules = []\ninterface = \"wlan0\"\nsupplicant = \"/s\"\nservice = \"a/b\"\n";
        assert!(DeviceConfig::parse(&format!("{base}probe = \"http://connectivitycheck.gstatic.com/generate_204\"\n")).is_ok());
        assert!(DeviceConfig::parse(&format!("{base}probe = \"https://connectivitycheck.gstatic.com/generate_204\"\n")).is_err(), "a login cannot answer in its stead");
        assert!(DeviceConfig::parse(base).is_err(), "a WLAN says how it is probed");
    }

    #[test]
    fn firmware_is_named_from_the_loaders_folders_and_found_plainly() {
        let base = "[device]\ncodename = \"x\"\n[wlan]\nmodules = []\ninterface = \"wlan0\"\nsupplicant = \"/s\"\nservice = \"a/b\"\nprobe = \"http://p/\"\n";
        let with = |file: &str| DeviceConfig::parse(&format!("{base}firmware = [{file}]\n"));
        assert!(with("{ name = \"wlan/a.ini\", path = \"/vendor/etc/a.ini\" }").is_ok());
        assert!(with("{ name = \"wlan/mac.bin\", partition = \"persist\", path = \"kiwi/mac.bin\" }").is_ok());
        assert!(with("{ name = \"/etc/firmware/a.ini\", path = \"/vendor/etc/a.ini\" }").is_err());
        assert!(with("{ name = \"../a.ini\", path = \"/vendor/etc/a.ini\" }").is_err());
        assert!(with("{ name = \"a.ini\", path = \"vendor/etc/a.ini\" }").is_err());
        assert!(with("{ name = \"mac.bin\", partition = \"persist\", path = \"/kiwi/mac.bin\" }").is_err());
        assert!(with("{ name = \"mac.bin\", partition = \"by-name/persist\", path = \"mac.bin\" }").is_err());
    }

    #[test]
    fn services_use_the_partitions_they_run_from() {
        let cfg = DeviceConfig::parse(
            "[device]\ncodename = \"x\"\n\
             [[service]]\npath = \"/odm/bin/a\"\n\
             [[service]]\npath = \"/vendor/bin/sh\"\nlibraries = [\"/system/lib64\", \"/vendor/lib64\"]\n",
        )
        .unwrap();
        let [odm, vendor] = &cfg.services[..] else { panic!("two programs") };
        assert!(odm.uses(Path::new("/odm")));
        assert!(!odm.uses(Path::new("/odm_dlkm")), "a longer name is another partition");
        assert!(!odm.uses(Path::new("/vendor")));
        assert!(vendor.uses(Path::new("/vendor")));
        assert!(!vendor.uses(Path::new("/mnt/system")), "its /system/lib64 is the recovery's own");
    }
}

//! The programs a recovery starts to use the keys the phone keeps in its
//! secure world, found in what the phone's own partitions say: which
//! program serves each service the recovery asks, and how the phone's init
//! starts it.

use crate::description::{Described, Kind};
use crate::phone::{Happens, Phone};
use crate::script::{Program, Socket};
use anyhow::{bail, ensure, Context, Result};
use skyward_config::Service;
use std::path::Path;

/// The key service, and the packages whose services it serves besides: it
/// is one program behind them all.
const KEYS: &str = "android.hardware.security.keymint.IKeyMintDevice/default";
const KEY_PACKAGES: [&str; 3] = ["android.hardware.security.keymint", "android.hardware.security.secureclock", "android.hardware.security.sharedsecret"];

/// The credential service and the boot control, each by its package and
/// its interface; a phone has either as a service of the newer kind or of
/// the older.
const CREDENTIALS: (&str, &str) = ("android.hardware.gatekeeper", "IGatekeeper");
const BOOT: (&str, &str) = ("android.hardware.boot", "IBootControl");

/// The weaver, on phones that keep a part of each user's secret in a
/// secure element.
const WEAVER: &str = "android.hardware.weaver.IWeaver/default";

/// The service of one secure element, by its package and its interface,
/// and what the ones built into the phone are named after, as opposed to
/// the SIM cards'.
const ELEMENT: (&str, &str) = ("android.hardware.secure_element", "ISecureElement");
const EMBEDDED: &str = "eSE";

/// Android's secure element service, which in the system an app serves and
/// in a recovery the recovery does.
const ELEMENT_SERVICE: &str = skyward_omapi::SERVICE;

/// Which of a service the recovery asks: the one every phone has.
const INSTANCE: &str = "default";

/// What a weaver reaches its secure element through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Through {
    /// Android's secure element service.
    ElementService,
    /// A service of the secure element's maker's own, by its interface.
    Own(&'static str),
    /// Nothing Android describes: its slots the secure world keeps of
    /// itself, and the weaver's program reaches it on its own.
    SecureWorld,
}

/// The makes of weaver, each by what the name of its program ends in, the
/// longer endings first: Thales' and NXP's go through Android's secure
/// element service, Goodix's, whose program is named after no maker, through
/// Goodix's own, and Samsung's through nothing Android names, the slots the
/// secure world of its Hermes daemon keeps.
const WEAVERS: [(&str, Through); 4] = [
    (".thales", Through::ElementService),
    (".nxp-qti", Through::ElementService),
    ("weaver-service", Through::Own("vendor.goodix.hardware.secure_element_aidl.IGdxEse")),
    ("hermesd", Through::SecureWorld),
];

/// Qualcomm's programs between Android and the secure world, by the names
/// of their files: the one that registers the listeners the secure world
/// calls back through, with the property it sets once they are registered,
/// and the ones the secure world's apps are opened through.
const LISTENERS: &str = "qseecomd";
const LISTENERS_READY: &str = "vendor.sys.listeners.registered";
const OPENERS: [&str; 2] = ["ssgtzd", "hlosminkdaemon"];

/// The directory for hardware services of the older kind, by the name init
/// knows it under, and the property it sets once it takes calls.
pub const HARDWARE_DIRECTORY: &str = "hwservicemanager";
const HARDWARE_DIRECTORY_READY: &str = "hwservicemanager.ready";

/// The folders a program's libraries are looked for in, by the partition
/// the program is on: the recovery's own come before the vendor
/// partition's, which carries its own copies of the lowest of them, and a
/// program loading those would not be understood by the recovery's service
/// directory; with the vendor partition's comes its folder of hardware
/// libraries, where Android has a program on either partition look too.
/// The directory for hardware services is the installed system's own
/// program and runs on its libraries alone.
const VENDOR_LIBRARIES: [&str; 3] = ["/system/lib64", "/vendor/lib64", "/vendor/lib64/hw"];
const ODM_LIBRARIES: [&str; 4] = ["/system/lib64", "/vendor/lib64", "/vendor/lib64/hw", "/odm/lib64"];
const SYSTEM_LIBRARIES: [&str; 3] = ["/mnt/system/system/lib64/bootstrap", "/mnt/system/system/lib64", "/mnt/system_ext/lib64"];

/// The installed system's folders, where a program on the vendor or the
/// odm partition finds the few libraries it asks for that none of its own
/// folders has: the ones Android lets such a program share with the
/// system, which a recovery may not carry.
const SHARED_LIBRARIES: [&str; 2] = ["/mnt/system/system/lib64", "/mnt/system/system/lib64/bootstrap"];

/// The folders the program a script names `path` finds its libraries in,
/// in the order they are looked in: those of its partition, then `bundle`,
/// where a recovery has opened the libraries the phone's partitions were
/// built against, then, for a program on the vendor or the odm partition,
/// the installed system's. The bundle comes before those: the installed
/// system carries libraries under the names of the bundle's, built for its
/// own programs.
pub(crate) fn libraries_of(path: &str, bundle: Option<&Path>) -> Vec<String> {
    let (own, shared): (&[&str], &[&str]) = match path.split('/').nth(1) {
        Some("vendor") => (&VENDOR_LIBRARIES, &SHARED_LIBRARIES),
        Some("odm") => (&ODM_LIBRARIES, &SHARED_LIBRARIES),
        _ => (&SYSTEM_LIBRARIES, &[]),
    };
    let bundle = bundle.map(|dir| dir.to_string_lossy().into_owned());
    own.iter().map(|l| l.to_string()).chain(bundle).chain(shared.iter().map(|l| l.to_string())).collect()
}

/// The folders a hardware service finds its libraries in, in the order the
/// phone's own hardware namespace looks in them: the libraries of its
/// partition and of the vendor partition, the hardware folder beside them,
/// then the bundle of the libraries the phone was built against, and last
/// what the recovery and the installed system carry that none of those
/// has. The recovery's own folder is not looked in first: it carries
/// older copies of the libraries the hardware service is built against,
/// and loading those is what the phone's namespace is meant to prevent.
fn hardware_libraries_of(path: &str, bundle: Option<&Path>) -> Vec<String> {
    let own: &[&str] = match path.split('/').nth(1) {
        Some("odm") => &["/odm/lib64", "/vendor/lib64", "/vendor/lib64/hw"],
        _ => &["/vendor/lib64", "/vendor/lib64/hw"],
    };
    let bundle = bundle.map(|dir| dir.to_string_lossy().into_owned());
    own.iter()
        .map(|l| l.to_string())
        .chain(bundle)
        .chain(std::iter::once("/system/lib64".to_string()))
        .chain(SHARED_LIBRARIES.iter().map(|l| l.to_string()))
        .collect()
}

/// The recovery's C++ runtime, which a hardware service loads ahead of the
/// one the bundle carries: the phone's logs, which the service and the
/// bundle's libraries both use, are built against the recovery's, and a
/// newer runtime runs what was built for an older one.
const RECOVERY_CXX: &str = "/system/lib64/libc++.so";

/// A program to start, with what init gives it that a recovery has to
/// give it too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub service: Service,
    pub sockets: Vec<Socket>,
    pub environment: Vec<(String, String)>,
    /// It holds on to a service the recovery serves itself, which goes
    /// when the recovery's own program ends: one an earlier run of the
    /// recovery left running is ended and started anew.
    pub anew: bool,
}

impl From<Service> for Started {
    /// A program init gives nothing besides what `service` says.
    fn from(service: Service) -> Started {
        Started { service, sockets: Vec::new(), environment: Vec::new(), anew: false }
    }
}

/// One thing to do, in the order they are done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Start a program of the phone's and wait until it is ready.
    Start(Box<Started>),
    /// Serve Android's secure element service, with a reader for each
    /// secure element whose own service has registered by then.
    ServeElements,
}

/// What a program is ready with.
enum Ready {
    Nothing,
    Property(&'static str),
    Service(String),
    HardwareService(String),
}

/// What to do, in order: start the listeners and the openers of the secure
/// world, the key service, the directory for hardware services where the
/// phone has services of the older kind, and the credential service; on a
/// phone with a weaver, start what the weaver reaches its secure element
/// through, serving Android's secure element service where that is what it
/// goes through, and the weaver; and start the boot control.
pub fn steps(phone: &Phone) -> Result<Vec<Step>> {
    let mut out = Vec::new();
    let start = |out: &mut Vec<Step>, started: Started| out.push(Step::Start(Box::new(started)));
    if let Some(listeners) = named(phone, LISTENERS)? {
        start(&mut out, started(phone, listeners, Ready::Property(LISTENERS_READY), Vec::new()));
    }
    for opener in OPENERS {
        if let Some(program) = named(phone, opener)? {
            start(&mut out, started(phone, program, Ready::Nothing, Vec::new()));
        }
    }

    // The key service is of the newer kind where the phone describes one,
    // and of the older kind otherwise: the phone its vendor was built for
    // is what decides, as Android's own keystore reads the same
    // descriptions and reaches the older kind over the driver the hardware
    // services talk on.
    let (keys, keys_kind) = match described(phone, Kind::Aidl, KEYS).is_some() {
        true => (KEYS.to_string(), Kind::Aidl),
        false => offered(phone, ("android.hardware.keymaster", "IKeymasterDevice")).context("the phone describes no key service")?,
    };
    let (credentials, credentials_kind) = offered(phone, CREDENTIALS).context("the phone describes no credential service")?;
    // A phone without a second slot has nothing to control: Android's boot
    // control is described only where there is a slot to switch to.
    let boot = offered(phone, BOOT);
    let hardware_kinds = [Some(keys_kind), Some(credentials_kind), boot.as_ref().map(|(_, kind)| *kind)];
    if hardware_kinds.contains(&Some(Kind::Hidl)) {
        // The phone's own directory for hardware services is started where
        // its scripts name one; a recovery that carries its own has that
        // one running already, and the services register with it as they
        // would with the phone's.
        if let Some(directory) = phone.programs.iter().find(|p| p.name == HARDWARE_DIRECTORY && phone.carries(p)) {
            start(&mut out, started(phone, directory, Ready::Property(HARDWARE_DIRECTORY_READY), Vec::new()));
        }
    }
    let ready = |name: String, kind: Kind| match kind {
        Kind::Aidl => Ready::Service(name),
        Kind::Hidl => Ready::HardwareService(name),
    };
    let program = behind(phone, "the key service", &keys, |file| {
        (file.contains("keymint") || file.contains("keymaster")) && !file.contains("strongbox")
    })?;
    let with_keys = |d: &Described| d.instance == INSTANCE && KEY_PACKAGES.contains(&d.package.as_str());
    let registers = besides(phone, program, &keys, with_keys);
    start(&mut out, started(phone, program, ready(keys.clone(), keys_kind), registers));
    let alone = |_: &Described| false;
    let program = behind(phone, "the credential service", &credentials, |file| file.contains("gatekeeper"))?;
    let registers = besides(phone, program, &credentials, alone);
    start(&mut out, started(phone, program, ready(credentials, credentials_kind), registers));

    // A weaver is described of the newer kind, or of the older one, as
    // Samsung's is, where the slots live in the secure world and the
    // program that serves it registers with the directory for hardware
    // services.
    let weaver_of = |kind: Kind| {
        phone.services.iter().find(|s| {
            s.described.kind == kind
                && (s.described.package.as_str(), s.described.interface.as_str()) == ("android.hardware.weaver", "IWeaver")
                && s.described.instance == INSTANCE
        })
    };
    if weaver_of(Kind::Aidl).is_some() || weaver_of(Kind::Hidl).is_some() {
        out.extend(weaver(phone)?);
    }

    if let Some((boot, boot_kind)) = boot {
        let program = behind(phone, "the boot control", &boot, |file| file.starts_with(BOOT.0))?;
        let registers = besides(phone, program, &boot, alone);
        start(&mut out, started(phone, program, ready(boot, boot_kind), registers));
    }
    Ok(out)
}

/// The description that tells of the service `name`, where it is the one
/// of `program` alone: a description named as the program's script is
/// tells of what that program registers and of nothing else. One named
/// otherwise may tell of what several programs register, as the one a
/// whole partition shares does, or one that tells of an NFC chip's service
/// and of the secure element's in the chip.
fn own_description<'a>(phone: &'a Phone, program: &Program, name: &str) -> Option<&'a str> {
    let told = described(phone, Kind::Aidl, name)?;
    (!program.script.is_empty() && stem(&program.script) == stem(&told.file)).then_some(told.file.as_str())
}

/// What `program`, ready with the service `name`, registers besides it,
/// which the service directory takes only when it is told of them too:
/// every other service of the newer kind its own description tells of, and
/// whichever of all the phone describes `also` takes.
fn besides(phone: &Phone, program: &Program, name: &str, also: impl Fn(&Described) -> bool) -> Vec<String> {
    let own = own_description(phone, program, name);
    let mut out = Vec::new();
    for service in phone.services.iter().filter(|s| s.described.kind == Kind::Aidl) {
        let other = service.described.name();
        if (own == Some(service.file.as_str()) || also(&service.described)) && other != name && !out.contains(&other) {
            out.push(other);
        }
    }
    out
}

/// The programs among `steps`, in order.
pub fn programs(steps: &[Step]) -> Vec<&Started> {
    steps
        .iter()
        .filter_map(|step| match step {
            Step::Start(started) => Some(started.as_ref()),
            Step::ServeElements => None,
        })
        .collect()
}

/// What starting the weaver takes: what it reaches its secure element
/// through, and then the weaver itself.
fn weaver(phone: &Phone) -> Result<Vec<Step>> {
    let mut out = Vec::new();
    let of = |kind: Kind| {
        phone
            .services
            .iter()
            .find(|s| s.described.kind == kind && (s.described.package.as_str(), s.described.interface.as_str()) == ("android.hardware.weaver", "IWeaver") && s.described.instance == INSTANCE)
    };
    let (name, kind) = match of(Kind::Aidl) {
        Some(_) => (WEAVER.to_string(), Kind::Aidl),
        None => (of(Kind::Hidl).expect("the phone describes a weaver").described.name(), Kind::Hidl),
    };
    let program = behind(phone, "the weaver", &name, |file| WEAVERS.iter().any(|(ending, _)| file.ends_with(ending)))?;
    let file = file_name(program);
    let (_, through) = WEAVERS.iter().find(|(ending, _)| file.ends_with(ending)).with_context(|| format!("{} is a weaver of a make the recovery does not know", program.path))?;
    match through {
        Through::Own(interface) => {
            let own = phone
                .services
                .iter()
                .find(|s| s.described.kind == Kind::Aidl && format!("{}.{}", s.described.package, s.described.interface) == *interface)
                .with_context(|| format!("{} goes through {interface}, which the phone does not describe", program.path))?;
            out.push(Step::Start(Box::new(element(phone, &own.described.name())?)));
        }
        Through::ElementService => {
            ensure!(described(phone, Kind::Aidl, ELEMENT_SERVICE).is_some(), "{} goes through {ELEMENT_SERVICE}, which the phone does not describe", program.path);
            let embedded: Vec<String> = phone
                .services
                .iter()
                .map(|s| &s.described)
                .filter(|d| d.kind == Kind::Aidl && (d.package.as_str(), d.interface.as_str()) == ELEMENT && d.instance.starts_with(EMBEDDED))
                .map(Described::name)
                .collect();
            ensure!(!embedded.is_empty(), "{} goes through a secure element built into the phone, and the phone describes the service of none", program.path);
            for name in embedded {
                let started = element(phone, &name)?;
                if !out.iter().any(|step| matches!(step, Step::Start(s) if s.service.path == started.service.path)) {
                    out.push(Step::Start(Box::new(started)));
                }
            }
            out.push(Step::ServeElements);
        }
        Through::SecureWorld => {}
    }
    // A weaver whose slots the secure world keeps comes up only once the
    // data partition is mounted, for the state it keeps beside the phone's
    // own: it is started here and left to its own retries, and whoever
    // reads a slot waits for it then.
    let ready = match (kind, through) {
        (Kind::Aidl, _) => Ready::Service(name.clone()),
        (Kind::Hidl, Through::SecureWorld) => Ready::Nothing,
        (Kind::Hidl, _) => Ready::HardwareService(name.clone()),
    };
    let mut weaver = started(phone, program, ready, besides(phone, program, &name, |_| false));
    // A weaver whose slots the secure world keeps runs on the installed
    // system's C++ runtime, which the libraries beside it were built
    // against, where a recovery's own is older and misses what those ask
    // for. Preloading it takes it ahead of the recovery's own.
    if *through == Through::SecureWorld {
        weaver.environment.retain(|(name, _)| name != "LD_PRELOAD");
        weaver.environment.push(("LD_PRELOAD".to_string(), format!("{}/libc++.so", SHARED_LIBRARIES[0])));
    }
    weaver.anew = *through == Through::ElementService;
    out.push(Step::Start(Box::new(weaver)));
    Ok(out)
}

/// The program behind the service of a secure element registered as
/// `name`: it is ready with the secure element's own service, which is
/// `name` itself or the one the program's own description tells of beside
/// it.
fn element(phone: &Phone, name: &str) -> Result<Started> {
    let program = behind(phone, "a secure element's service", name, |file| file.contains("secure_element"))?;
    let told = described(phone, Kind::Aidl, name).expect("the service was found among those described");
    let of_an_element = |d: &Described| (d.package.as_str(), d.interface.as_str()) == ELEMENT;
    let own = match of_an_element(&told.described) {
        true => name.to_string(),
        false => {
            let beside = own_description(phone, program, name).and_then(|file| phone.services.iter().find(|s| s.file == file && s.described.kind == Kind::Aidl && of_an_element(&s.described)));
            beside.with_context(|| format!("{} registers {name}, and its description tells of the service of no secure element beside it", program.path))?.described.name()
        }
    };
    let registers = besides(phone, program, &own, |_| false);
    Ok(started(phone, program, Ready::Service(own), registers))
}

/// The description of the service `name` of `kind`, where the phone
/// describes it.
fn described<'a>(phone: &'a Phone, kind: Kind, name: &str) -> Option<&'a crate::phone::Service> {
    phone.services.iter().find(|s| s.described.kind == kind && s.described.name() == name)
}

/// The name the phone has the default service of `interface` under, and
/// its kind: the newer kind where it has it, else the older at the highest
/// version it describes.
fn offered(phone: &Phone, (package, interface): (&str, &str)) -> Option<(String, Kind)> {
    let of = |kind: Kind| phone.services.iter().map(|s| &s.described).filter(move |d| d.kind == kind && d.package == package && d.interface == interface && d.instance == INSTANCE);
    if let Some(newer) = of(Kind::Aidl).next() {
        return Some((newer.name(), Kind::Aidl));
    }
    let version = |d: &&Described| -> Vec<u32> { d.version.iter().flat_map(|v| v.split('.')).filter_map(|n| n.parse().ok()).collect() };
    of(Kind::Hidl).max_by_key(version).map(|older| (older.name(), Kind::Hidl))
}

/// The name of a program's file.
fn file_name(program: &Program) -> &str {
    program.path.rsplit('/').next().unwrap_or(&program.path)
}

/// The name of a file without what follows its last dot, which a script
/// and a description that belong to one program share.
fn stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem)
}

/// The one program the phone runs whose file is named `file`; none where
/// the phone has none.
fn named<'a>(phone: &'a Phone, file: &str) -> Result<Option<&'a Program>> {
    one(phone, file, phone.programs.iter().filter(|p| file_name(p) == file).collect())
}

/// Whether init starts `program` only when a service its section says it
/// registers is asked for. Nothing can count on such a program for any
/// other service, which would be there only while something else had
/// asked for one of those.
fn on_demand(program: &Program) -> bool {
    program.disabled && !program.interfaces.is_empty()
}

/// The one program the phone runs that serves `name`: one whose script
/// says it registers it; else one whose script is named as the description
/// that tells of the service is; else one whose file `matches`. A program
/// started only on demand is one of the first kind or none.
fn behind<'a>(phone: &'a Phone, what: &str, name: &str, matches: impl Fn(&str) -> bool) -> Result<&'a Program> {
    let saying: Vec<&Program> = phone.programs.iter().filter(|p| p.interfaces.iter().any(|i| i == name)).collect();
    let may: Vec<&Program> = phone.programs.iter().filter(|p| !on_demand(p)).collect();
    let described_with: Vec<&Program> = match phone.services.iter().find(|s| s.described.name() == name) {
        Some(service) => may.iter().copied().filter(|p| !p.script.is_empty() && stem(&p.script) == stem(&service.file)).collect(),
        None => Vec::new(),
    };
    // A program whose script says it registers some other instance of
    // this interface is not the one asked for, as a vendor's own build of
    // a service beside the phone's is not: the one that says nothing
    // registers what its file is named after.
    let matching: Vec<&Program> = may
        .iter()
        .copied()
        .filter(|p| matches(file_name(p)))
        .filter(|p| p.interfaces.is_empty() || p.interfaces.iter().any(|i| i == name))
        .collect();
    for candidates in [saying, described_with, matching] {
        if let Some(program) = one(phone, what, candidates)? {
            return Ok(program);
        }
    }
    bail!("no script of the phone's runs {what}, which would register {name}")
}

/// The one of `candidates` the phone's init runs: those it runs for
/// certain before those it may run. More than one is refused, as nothing
/// here tells which the phone means.
fn one<'a>(phone: &Phone, what: &str, candidates: Vec<&'a Program>) -> Result<Option<&'a Program>> {
    for wanted in [Happens::Yes, Happens::Maybe] {
        let running: Vec<&Program> = candidates.iter().copied().filter(|p| phone.runs(p) == wanted).collect();
        match running[..] {
            [] => continue,
            [program] => return Ok(Some(program)),
            _ => bail!("the phone runs more than one program for {what}: {}", running.iter().map(|p| p.path.as_str()).collect::<Vec<_>>().join(", ")),
        }
    }
    Ok(None)
}

/// `program` as it is started: from where its partition is mounted, as the
/// user, in the groups and with the capabilities its script gives it, with
/// the libraries of its partition. It is kept running whatever its script
/// says of starting it once: what a recovery starts it for is the service
/// it registers.
fn started(phone: &Phone, program: &Program, ready: Ready, registers: Vec<String>) -> Started {
    let (ready_property, ready_service, ready_hardware_service) = match ready {
        Ready::Nothing => (None, None, None),
        Ready::Property(name) => (Some(name.to_string()), None, None),
        Ready::Service(name) => (None, Some(name), None),
        Ready::HardwareService(name) => (None, None, Some(name)),
    };
    let hardware = ready_hardware_service.is_some();
    let libraries = match hardware {
        true => hardware_libraries_of(&program.path, phone.bundle.as_deref()),
        false => libraries_of(&program.path, phone.bundle.as_deref()),
    };
    let mut environment = program.environment.clone();
    if hardware {
        environment.push(("LD_PRELOAD".to_string(), RECOVERY_CXX.to_string()));
    }
    let service = Service {
        path: phone.started_from(&program.path),
        args: program.args.clone(),
        libraries,
        oneshot: false,
        ready_property,
        ready_service,
        registers,
        ready_hardware_service,
        when: None,
        user: program.user.clone(),
        groups: program.groups.clone(),
        capabilities: program.capabilities.clone().unwrap_or_default(),
    };
    Started { service, sockets: program.sockets.clone(), environment, anew: false }
}

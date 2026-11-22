// The numbering serves the phone's own driver and the host tests.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

#[cfg(target_os = "android")]
use crate::parcel::Parcel;
#[cfg(target_os = "android")]
use crate::status;
#[cfg(target_os = "android")]
use anyhow::{Context, Result};

/// The directory every program registers with and looks things up in.
const DIRECTORY: &str = "android.os.IServiceManager";

/// What the directory numbers the methods it is asked by here. It numbers
/// its methods in the order its interface declares them, and Android 16
/// declared two lookups of its own ahead of the rest; the directory is the
/// recovery's own program, so the version the recovery was built as says
/// which numbering it keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Methods {
    /// Ask for a service without waiting for one that has not registered
    /// yet.
    check_service: u32,
    /// Register a service.
    add_service: u32,
    /// List the names registered at the dump priorities given.
    list_services: u32,
    /// Ask whether the phone says it has a service, whether or not it is
    /// running.
    is_declared: u32,
}

/// The first version of Android whose directory declares the two lookups.
#[cfg(test)]
const LOOKUPS_DECLARED: u32 = 36;

/// How the directory of a recovery built as Android version `sdk` numbers
/// its methods.
#[cfg(test)]
fn methods_of(sdk: u32) -> Methods {
    match sdk >= LOOKUPS_DECLARED {
        true => Methods { check_service: 3, add_service: 5, list_services: 6, is_declared: 9 },
        false => Methods { check_service: 2, add_service: 3, list_services: 4, is_declared: 7 },
    }
}

/// The calls the directory takes: the numbers of its methods, and the mark
/// its header carries — none for the directories of builds before the mark
/// was introduced, whose header holds two marks only. Which of the forms a
/// phone's directory keeps cannot be read from the recovery's own
/// properties, which may name a build older than the directory's binary: a
/// recovery ramdisk built long ago on top of the installed system's
/// directories of today. So the forms are tried, and the one that answers
/// is kept.
#[cfg(target_os = "android")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Form {
    check: u32,
    add: u32,
    list: u32,
    declared: u32,
    /// The mark the header carries, or [`crate::parcel::HEADER_NONE`].
    header: i32,
}

#[cfg(target_os = "android")]
impl Form {
    fn methods(self) -> Methods {
        Methods { check_service: self.check, add_service: self.add, list_services: self.list, is_declared: self.declared }
    }

    fn write_name(self, call: &mut Parcel, name: &str) {
        call.string(name);
    }
}

/// The form the directory answered, kept once a lookup has found a service
/// or a registration has taken.
#[cfg(target_os = "android")]
static FORM: std::sync::OnceLock<Form> = std::sync::OnceLock::new();

/// The forms the directories of the builds a phone may carry answer in,
/// newest first. Android 16 declared the two lookups and writes the mark;
/// builds before it numbered the calls as they are here without it.
#[cfg(target_os = "android")]
fn forms() -> Vec<Form> {
    let new = Methods { check_service: 3, add_service: 5, list_services: 6, is_declared: 9 };
    let old = Methods { check_service: 2, add_service: 3, list_services: 4, is_declared: 7 };
    let mark = |m: Methods, header: i32| Form {
        check: m.check_service,
        add: m.add_service,
        list: m.list_services,
        declared: m.is_declared,
        header,
    };
    let remembered = FORM.get().copied();
    let mut out = Vec::new();
    if let Some(f) = remembered {
        out.push(f);
    }
    for (m, header) in [
        (new, crate::parcel::HEADER_RECOVERY),
        (old, crate::parcel::HEADER_NONE),
        (new, crate::parcel::HEADER_SYSTEM),
        (old, crate::parcel::HEADER_SYSTEM),
        (old, crate::parcel::HEADER_RECOVERY),
        (new, crate::parcel::HEADER_VENDOR),
        (old, crate::parcel::HEADER_VENDOR),
        (new, crate::parcel::HEADER_NONE),
    ] {
        let f = mark(m, header);
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

#[cfg(target_os = "android")]
fn remember(form: Form) {
    let _ = FORM.set(form);
    crate::parcel::use_header(form.header);
}

/// Every dump priority a service can register at, and the one a service
/// registers at that asks for none.
const DUMP_PRIORITY_ALL: i32 = 0b1111;
const DUMP_PRIORITY_DEFAULT: i32 = 0b1000;

/// Ask any object which process serves it. libbinder answers this itself,
/// for every object, before any interface sees the call.
const DEBUG_PID: u32 = u32::from_be_bytes(*b"_PID");

/// Ask any object which interface it serves, which libbinder answers the
/// same way.
const INTERFACE: u32 = u32::from_be_bytes(*b"_NTF");

/// Ask a service which version of its interface it serves. Every service
/// of the newer kind answers this, numbered from the top of the range its
/// own methods are numbered from the bottom of.
const INTERFACE_VERSION: u32 = 0x00ff_ffff;

/// Which version of `interface` a service serves. It answers this only
/// once it has taken the call as one for that interface, so a service that
/// will not answer it is one that turns the call away before reading
/// anything of what it carries.
#[cfg(target_os = "android")]
pub fn version(service: &crate::driver::Handle, interface: &str) -> Result<i32> {
    let what = format!("ask which version of {interface} is served");
    let reply = service.transact(INTERFACE_VERSION, &Parcel::call(interface)).with_context(|| what.clone())?;
    let mut read = reply.read();
    status::ok(&mut read).with_context(|| what.clone())?;
    read.i32().with_context(|| what)
}

/// The service registered under `name`, or `None` while nothing has
/// registered it. The forms the directory's calls take are tried in turn,
/// and the one that answers is kept for the calls to come.
#[cfg(target_os = "android")]
pub fn find<'a>(binder: &'a crate::driver::Binder, name: &str) -> Result<Option<crate::driver::Handle<'a>>> {
    let directory = binder.directory()?;
    let mut said: Option<anyhow::Error> = None;
    for form in forms() {
        crate::parcel::use_header(form.header);
        let mut call = Parcel::call(DIRECTORY);
        form.write_name(&mut call, name);
        let reply = match directory.transact(form.check, &call) {
            Ok(reply) => reply,
            Err(e) => {
                said = Some(e.context(format!("look up {name}")));
                continue;
            }
        };
        let mut read = reply.read();
        let found = match status::ok(&mut read) {
            Ok(()) => match read.object().with_context(|| format!("read what the directory answered for {name}"))? {
                Some(handle) => crate::driver::Handle::hold(binder, handle).map(Some),
                None => Ok(None),
            },
            Err(e) => Err(e).with_context(|| format!("look up {name}")),
        };
        match found {
            // The form the directory answered in is the one it reads, even
            // where the answer is that the name has nothing under it.
            Ok(Some(handle)) => {
                remember(form);
                return Ok(Some(handle));
            }
            Ok(None) => {
                remember(form);
                return Ok(None);
            }
            Err(e) => said = Some(e),
        }
    }
    match said {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

/// Register the object this program serves under `number`, through
/// `binder`, as the service `name`: for every program to find, none of
/// them in a sandbox, at no dump priority of its own. The directory takes
/// a hardware service only where the phone's description names it. The
/// form of the calls is the one a lookup has already found the directory
/// answering, and the registration is looked up again in the same form to
/// be sure a form the directory does not read has not taken a wrong name.
#[cfg(target_os = "android")]
pub fn add(binder: &crate::driver::Binder, name: &str, number: u64) -> Result<()> {
    for form in forms() {
        let directory = binder.directory()?;
        let mut call = Parcel::call(DIRECTORY);
        form.write_name(&mut call, name);
        call.object(number);
        call.bool(false);
        call.i32(DUMP_PRIORITY_DEFAULT);
        let reply = match directory.transact(form.methods().add_service, &call) {
            Ok(reply) => reply,
            Err(_) => {
                continue;
            }
        };
        if status::ok(&mut reply.read()).is_err() {
            continue;
        }
        match find(binder, name)? {
            Some(_) => {
                remember(form);
                return Ok(());
            }
            None => continue,
        }
    }
    Err(anyhow::anyhow!("register {name}: the directory took none of the calls"))
}

/// Every name registered with the directory, the directory's own among
/// them. The forms the directory's calls take are tried in turn, and the
/// one that answers is kept for the calls to come.
#[cfg(target_os = "android")]
pub fn names(binder: &crate::driver::Binder) -> Result<Vec<String>> {
    let directory = binder.directory()?;
    let mut said: Option<anyhow::Error> = None;
    for form in forms() {
        let mut call = Parcel::call(DIRECTORY);
        call.i32(DUMP_PRIORITY_ALL);
        let reply = match directory.transact(form.methods().list_services, &call) {
            Ok(reply) => reply,
            Err(e) => {
                said = Some(e.context("list what has registered"));
                continue;
            }
        };
        let mut read = reply.read();
        if status::ok(&mut read).is_err() {
            continue;
        }
        let listed = (|| -> Result<Vec<String>> {
            let count = read.i32().context("read how many have registered")?;
            anyhow::ensure!(count >= 0, "the directory says {count} have registered");
            (0..count).map(|_| read.string().context("read a registered name")).collect()
        })()
        .context("list what has registered");
        match listed {
            Ok(names) => {
                remember(form);
                return Ok(names);
            }
            Err(e) => said = Some(e),
        }
    }
    match said {
        Some(e) => Err(e),
        None => Ok(Vec::new()),
    }
}

/// The process that serves an object.
#[cfg(target_os = "android")]
pub fn pid(service: &crate::driver::Handle) -> Result<u32> {
    let reply = service.transact(DEBUG_PID, &Parcel::default()).context("ask which process serves an object")?;
    let pid = reply.read().i32().context("read which process serves an object")?;
    u32::try_from(pid).with_context(|| format!("an object says process {pid} serves it"))
}

/// The interface an object serves, by its full name.
#[cfg(target_os = "android")]
pub fn interface(service: &crate::driver::Handle) -> Result<String> {
    let reply = service.transact(INTERFACE, &Parcel::default()).context("ask which interface an object serves")?;
    reply.read().string().context("read which interface an object serves")
}

/// Whether the phone's own description says it has this service. The
/// directory turns away a hardware service the description does not name,
/// so a service that will not register is often one the directory cannot
/// see described.
#[cfg(target_os = "android")]
pub fn declared(binder: &crate::driver::Binder, name: &str) -> Result<bool> {
    let directory = binder.directory()?;
    let mut said = None;
    for form in forms() {
        let mut call = Parcel::call(DIRECTORY);
        form.write_name(&mut call, name);
        let reply = match directory.transact(form.methods().is_declared, &call) {
            Ok(reply) => reply,
            Err(e) => {
                said = Some(e.context(format!("ask whether {name} is described")));
                continue;
            }
        };
        let mut read = reply.read();
        let told = status::ok(&mut read)
            .and_then(|()| read.bool().with_context(|| format!("read whether {name} is described")));
        match told {
            Ok(answer) => {
                remember(form);
                return Ok(answer);
            }
            Err(e) => said = Some(e),
        }
    }
    match said {
        Some(e) => Err(e),
        None => Ok(false),
    }
}

/// A name no phone's directory has, asked for to tell which mark the
/// directory reads.
const PROBE: &str = "skyward.header.probe";

/// Settle which mark the calls made from here carry, and answer it. A
/// directory that reads the mark answers for the probe name that it has
/// none; one built for another copy of libbinder turns the call away
/// before reading it. The recovery's own copy is the usual one, but a
/// recovery ramdisk that carries the installed system's copy, as this
/// phone's does, takes only the system mark.
#[cfg(target_os = "android")]
pub fn choose_header(binder: &crate::driver::Binder) -> Result<i32> {
    // The directory's own process may take a while to start, and while it
    // has not, every call ends in the driver. The form it answers in is
    // found by trying, and the mark it reads comes with the form.
    let give = std::time::Instant::now() + std::time::Duration::from_secs(12);
    loop {
        match find(binder, PROBE) {
            Ok(_) => return Ok(crate::parcel::header_mark()),
            Err(e) => {
                if std::time::Instant::now() >= give {
                    // The directory never answered: say why, from the last of
                    // the tries.
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The positions of getService, checkService, addService, listServices,
    /// registerForNotifications, unregisterForNotifications and isDeclared
    /// in IServiceManager.aidl, and of the two lookups Android 16 put after
    /// the first and the second of them.
    #[test]
    fn the_directory_of_android_16_numbers_its_methods_two_and_four_further_on() {
        assert_eq!(methods_of(33), Methods { check_service: 2, add_service: 3, list_services: 4, is_declared: 7 }, "the 13 Ultra's recovery");
        assert_eq!(methods_of(35), methods_of(33));
        assert_eq!(methods_of(36), Methods { check_service: 3, add_service: 5, list_services: 6, is_declared: 9 }, "the 17 Ultra's recovery");
        assert_eq!(methods_of(37), methods_of(36));
    }
}

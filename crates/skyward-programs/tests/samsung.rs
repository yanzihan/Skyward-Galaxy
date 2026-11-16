//! What a recovery does with a Samsung phone's own keys, read from the init
//! scripts and the hardware descriptions its vendor partition carries. The
//! files under tests/described/p3q are those of an SM-G9980 on the stock
//! Android 11 vendor the ports are built on.

use skyward_programs::phone::{Partition, Phone, PARTITIONS};
use skyward_programs::secure::{self, Step};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn described(codename: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/described").join(codename)
}

/// Where a recovery has each partition mounted.
fn mounted(partition: &str) -> String {
    match partition {
        "vendor" | "odm" => format!("/{partition}"),
        other => format!("/mnt/{other}"),
    }
}

fn phone(codename: &str, properties: &[(&str, &str)]) -> Phone {
    let at = described(codename);
    let partitions = PARTITIONS.iter().map(|name| Partition { name: name.to_string(), mounted: mounted(name).into(), at: at.join(name) }).collect();
    let properties: HashMap<String, String> = properties.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
    Phone::read(partitions, properties, None).unwrap()
}

/// The steps, each as the name of the program's file, or as the secure
/// element service where the recovery serves it.
fn names(steps: &[Step]) -> Vec<&str> {
    steps
        .iter()
        .map(|step| match step {
            Step::Start(started) => started.service.path.rsplit('/').next().unwrap(),
            Step::ServeElements => "(the secure element service)",
        })
        .collect()
}

#[test]
fn p3q_starts_the_key_services_its_own_init_scripts_name() {
    let phone = phone("p3q", &[("ro.boot.hardware", "qcom"), ("ro.boot.product.vendor.sku", "lahaina")]);
    let steps = secure::steps(&phone).unwrap();
    assert_eq!(
        names(&steps),
        ["qseecomd", "ssgtzd", "android.hardware.keymaster@4.0-service", "android.hardware.gatekeeper@1.0-service"],
        "the listeners, the secure element service, the key service and the credential service"
    );
    let Step::Start(keymaster) = &steps[2] else { panic!("the key service is started") };
    assert_eq!(
        keymaster.service.ready_hardware_service.as_deref(),
        Some("android.hardware.keymaster@4.0::IKeymasterDevice/default"),
        "the key service registers with the directory for the older hardware services"
    );
    assert_eq!((keymaster.service.user.as_deref(), keymaster.service.groups.as_slice()), (Some("system"), &["system".to_string(), "drmrpc".to_string()][..]));
    let Step::Start(gatekeeper) = &steps[3] else { panic!("the credential service is started") };
    assert_eq!(gatekeeper.service.groups.last().map(String::as_str), Some("5678"), "its own group is a number the phone's list names");
    assert_eq!(gatekeeper.service.ready_hardware_service.as_deref(), Some("android.hardware.gatekeeper@1.0::IGatekeeper/default"));
}

#[test]
fn the_secure_element_service_is_started_where_its_listeners_are_ready() {
    let steps = secure::steps(&phone("p3q", &[("ro.boot.hardware", "qcom"), ("ro.boot.product.vendor.sku", "lahaina")])).unwrap();
    let Step::Start(listeners) = &steps[0] else { panic!("the listeners are started first") };
    assert_eq!(listeners.service.ready_property.as_deref(), Some("vendor.sys.listeners.registered"));
    let Step::Start(elements) = &steps[1] else { panic!("the secure element service follows the listeners") };
    assert_eq!(elements.service.path, "/vendor/bin/ssgtzd");
    assert_eq!(elements.sockets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["ssgtzd", "qwes_ipc"]);
}

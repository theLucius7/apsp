use apsp_agent_core::bundled::{BundledComponents, BundledItem, BundledPayload};
use std::{borrow::Cow, collections::BTreeMap};

include!(concat!(env!("OUT_DIR"), "/embedded_bundle.rs"));

pub fn manifest_frame() -> &'static [u8] {
    EMBEDDED_COMPONENTS_FRAME
}

fn singbox_binary_name() -> &'static str {
    if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    }
}

#[cfg(target_os = "linux")]
fn tcp_probe_binary_name() -> &'static str {
    "apsp-tcp-probe"
}

fn tar_gz_item(
    component: &str,
    binary_name: &str,
    embedded: Option<(&'static str, &'static str, &'static str, &'static [u8])>,
) -> Option<BundledItem> {
    let (version, sha256, inventory_sha256, bytes) = embedded?;
    Some(BundledItem {
        component_name: component.to_owned(),
        binary_name: binary_name.to_owned(),
        version: version.to_owned(),
        payload: BundledPayload::TarGz {
            inventory_sha256: Some(inventory_sha256.to_owned()),
            archive_sha256: sha256.to_owned(),
            bytes: Cow::Borrowed(bytes),
        },
    })
}

pub fn payload(name: &str) -> Option<BundledItem> {
    match name {
        "sing-box" | "singbox" => tar_gz_item("sing-box", singbox_binary_name(), EMBEDDED_SINGBOX),
        #[cfg(target_os = "linux")]
        "tcpquality" => Some(BundledItem {
            component_name: "tcpquality".to_owned(),
            binary_name: tcp_probe_binary_name().to_owned(),
            version: EMBEDDED_TCPQUALITY_VERSION.to_owned(),
            payload: BundledPayload::AgentSubcommand {
                subcommand: "tcp-probe".to_owned(),
            },
        }),
        "nodequality" => tar_gz_item("nodequality", "nodequality", EMBEDDED_NODEQUALITY),
        "ipquality" => tar_gz_item("ipquality", "ipquality", EMBEDDED_IPQUALITY),
        "network-workbench" => tar_gz_item(
            "network-workbench",
            "apsp-network-workbench",
            EMBEDDED_NETWORK_WORKBENCH,
        ),
        _ => None,
    }
}

pub fn components() -> BundledComponents {
    let frame = std::hint::black_box(manifest_frame());
    let _ = std::hint::black_box((frame.first(), frame.last()));

    let mut runtimes = BTreeMap::new();
    if let Some(singbox) = payload("sing-box") {
        runtimes.insert("singbox".to_owned(), singbox);
    }

    let mut tools = BTreeMap::new();
    #[cfg(target_os = "linux")]
    if let Some(tcpquality) = payload("tcpquality") {
        tools.insert("tcpquality".to_owned(), tcpquality);
    }
    for name in ["nodequality", "ipquality", "network-workbench"] {
        if let Some(item) = payload(name) {
            tools.insert(name.to_owned(), item);
        }
    }

    BundledComponents { runtimes, tools }
}

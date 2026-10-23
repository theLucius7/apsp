#![forbid(unsafe_code)]

use apsp_compiler::{
    Access, ManagedAcceptance, ManagedEndpointSnapshot, Node, OrderedPath, PathHop, ProbeControl,
    ProtocolConfig, Relay, compile_server_with_paths, compile_server_with_relays,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

fn absolute_input(name: &str) -> PathBuf {
    let value = PathBuf::from(
        std::env::var_os(name).expect("native gate requires its explicit private input"),
    );
    assert!(
        value.is_absolute(),
        "native gate inputs must use absolute paths"
    );
    value
        .canonicalize()
        .expect("native gate input is unavailable")
}
fn run(binary: &Path, args: &[&std::ffi::OsStr]) -> (bool, Vec<u8>) {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("native command could not start");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("native command wait failed") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native command exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .expect("native stdout")
        .take(8193)
        .read_to_end(&mut output)
        .expect("native version output could not be read");
    assert!(
        output.len() <= 8192,
        "native command output exceeded its budget"
    );
    (status.success(), output)
}
fn native_version(binary: &Path) {
    let (success, output) = run(binary, &[std::ffi::OsStr::new("version")]);
    assert!(success, "native version command failed");
    let output = std::str::from_utf8(&output).expect("native version output is not UTF-8");
    assert!(
        output
            .lines()
            .any(|line| line.trim() == "sing-box version 1.14.2"),
        "native gate requires exact runtime version 1.14.2"
    );
    let tags = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("Tags: "))
        .expect("native gate requires observed build tags");
    let actual: std::collections::BTreeSet<_> = tags.split(',').map(str::trim).collect();
    for required in ["with_clash_api", "with_v2ray_api", "with_utls", "with_quic"] {
        assert!(
            actual.contains(required),
            "native runtime lacks a required build tag"
        );
    }
}
fn node(id: i64, user: bool) -> Node {
    // These reserved endpoints are check-only examples, never online claims.
    Node {
        id,
        name: format!("node-{id}"),
        port: 443,
        public_host: format!("node-{id}.example.com"),
        sni: "www.example.com".into(),
        private_key: URL_SAFE_NO_PAD.encode([1; 32]),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
        enabled: true,
        settings: Default::default(),
        protocol_config: ProtocolConfig::VlessReality,
        users: if user {
            vec![Access {
                user_id: 7,
                uuid: Uuid::from_u128(7),
                credential: String::new(),
            }]
        } else {
            vec![]
        },
    }
}
fn endpoint(id: i64) -> ManagedEndpointSnapshot {
    ManagedEndpointSnapshot {
        version_id: Uuid::from_u128(1000 + id as u128),
        server_id: id,
        node: node(id, false),
    }
}
fn managed(id: i64, identity: u128) -> PathHop {
    PathHop::Managed {
        endpoint: Box::new(endpoint(id)),
        relay_uuid: Uuid::from_u128(identity),
    }
}
fn external() -> PathHop {
    PathHop::External{source_id:1,identity_epoch:1,node_id:Uuid::from_u128(31),version_id:Uuid::from_u128(131),source_revision_id:Uuid::from_u128(9000),
    outbound:serde_json::from_value(json!({"type":"http","server":"external.example.com","server_port":8080,"username":"TEST_ONLY_ACCOUNT","password":"TEST_ONLY_PASSWORD","path":"/","headers":{}})).unwrap()}
}
fn accept(generation: u64, position: u8, id: i64, identity: u128) -> ManagedAcceptance {
    ManagedAcceptance {
        endpoint: endpoint(id),
        chain_id: 11,
        generation,
        position,
        relay_uuid: Uuid::from_u128(identity),
    }
}

#[test]
#[ignore = "requires exact sing-box 1.14.2 with full observed feature tags and a private evidence directory"]
fn exact_native_checks_three_and_four_hops_acceptances_and_legacy_bytes() {
    let binary = absolute_input("APSP_ORDERED_NATIVE_BINARY");
    assert!(binary.is_file(), "native binary input must be a file");
    native_version(&binary);
    let parent = absolute_input("APSP_ORDERED_NATIVE_EVIDENCE_DIR");
    assert!(
        parent.is_dir(),
        "native evidence parent must be a directory"
    );
    let directory = parent.join(format!("ordered-native-{}", Uuid::new_v4()));
    fs::create_dir(&directory).expect("native evidence directory creation failed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let entry = node(1, true);
    let current = OrderedPath {
        chain_id: 11,
        generation: 1,
        entry_node_id: 1,
        entry_server_id: 1,
        hops: vec![external(), managed(3, 3001)],
        active: true,
    };
    let mut candidate = OrderedPath {
        chain_id: 11,
        generation: 2,
        entry_node_id: 1,
        entry_server_id: 1,
        hops: vec![managed(2, 2002), external(), managed(3, 3002)],
        active: false,
    };
    let probe = ProbeControl {
        listen_port: 18086,
        secret: "a".repeat(64),
    };
    let three = compile_server_with_paths(
        std::slice::from_ref(&entry),
        &[],
        std::slice::from_ref(&current),
        &[],
        Some(&probe),
    )
    .unwrap();
    let coexist = compile_server_with_paths(
        std::slice::from_ref(&entry),
        &[],
        &[current.clone(), candidate.clone()],
        &[],
        Some(&probe),
    )
    .unwrap();
    candidate.active = true;
    let four = compile_server_with_paths(
        std::slice::from_ref(&entry),
        &[],
        std::slice::from_ref(&candidate),
        &[],
        Some(&probe),
    )
    .unwrap();
    let transit =
        compile_server_with_paths(&[node(2, false)], &[], &[], &[accept(2, 1, 2, 2002)], None)
            .unwrap();
    let shared_exit = compile_server_with_paths(
        &[node(3, true)],
        &[],
        &[],
        &[accept(1, 2, 3, 3001), accept(2, 3, 3, 3002)],
        None,
    )
    .unwrap();
    let relay = Relay {
        settings: Default::default(),
        fingerprint: Default::default(),
        chain_id: 11,
        entry_node_id: 1,
        exit_node_id: 3,
        uuid: Uuid::from_u128(101),
        public_host: "node-3.example.com".into(),
        port: 443,
        sni: "www.example.com".into(),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
    };
    let legacy_entry =
        compile_server_with_relays(std::slice::from_ref(&entry), std::slice::from_ref(&relay))
            .unwrap();
    assert_eq!(
        legacy_entry,
        compile_server_with_paths(
            std::slice::from_ref(&entry),
            std::slice::from_ref(&relay),
            &[],
            &[],
            None
        )
        .unwrap()
    );
    let exit = node(3, true);
    let legacy_exit =
        compile_server_with_relays(std::slice::from_ref(&exit), std::slice::from_ref(&relay))
            .unwrap();
    assert_eq!(
        legacy_exit,
        compile_server_with_paths(std::slice::from_ref(&exit), &[relay], &[], &[], None).unwrap()
    );
    let external_server=serde_json::to_string_pretty(&json!({"inbounds":[{"type":"http","listen":"127.0.0.1","listen_port":8080,
        "users":[{"username":"TEST_ONLY_ACCOUNT","password":"TEST_ONLY_PASSWORD"}]}],"outbounds":[{"type":"direct","tag":"direct"}],"route":{"final":"direct"}})).unwrap();
    for (name, config) in [
        ("three-entry", three),
        ("four-entry", four),
        ("candidate-coexist", coexist),
        ("managed-transit", transit),
        ("shared-exit", shared_exit),
        ("external-http", external_server),
        ("legacy-entry", legacy_entry),
        ("legacy-exit", legacy_exit),
    ] {
        let file = directory.join(format!("{name}.json"));
        fs::write(&file, config).expect("native fixture write failed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let (success, _) = run(
            &binary,
            &[
                std::ffi::OsStr::new("check"),
                std::ffi::OsStr::new("-c"),
                file.as_os_str(),
            ],
        );
        assert!(success, "native check failed for controlled fixture {name}");
    }
    fs::write(directory.join("acceptance-scope.json"),serde_json::to_vec_pretty(&json!({"schema":1,"runtime_version":"1.14.2","native_check":true,
        "required_build_tags":["with_clash_api","with_v2ray_api","with_utls","with_quic"],
        "legacy_bytes_unchanged":true,"traffic_verified":false,"udp_verified":false,"failure_no_bypass_verified":false,"entry_accounting_verified":false})).unwrap()).unwrap();
}

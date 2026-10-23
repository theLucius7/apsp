#![forbid(unsafe_code)]

use apsp_compiler::{
    Access, ManagedAcceptance, ManagedEndpointSnapshot, Node, OrderedPath, PathHop, ProbeControl,
    ProtocolConfig, compile_client, compile_server_with_paths,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    schema: u32,
    test_only: bool,
    runtime_version: String,
    native_binary_sha256: String,
    ports: BTreeMap<String, u16>,
    node_keys: BTreeMap<String, KeyPair>,
    credentials: Credentials,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyPair {
    private_key: String,
    public_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    public_uuid: Uuid,
    x_username: String,
    x_password: String,
    controller_secret: String,
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
fn directory(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn port(input: &Input, name: &str) -> Result<u16> {
    input
        .ports
        .get(name)
        .copied()
        .ok_or_else(|| "missing fixture port".into())
}
fn node(input: &Input, id: i64, role: &str, user: bool) -> Result<Node> {
    let key = input
        .node_keys
        .get(role)
        .ok_or("missing fixture key pair")?;
    let mut node = Node {
        id,
        name: format!("TEST_ONLY_{role}"),
        port: port(input, &role.to_ascii_lowercase())?,
        public_host: "127.0.0.1".into(),
        sni: "reality.test".into(),
        private_key: key.private_key.clone(),
        public_key: key.public_key.clone(),
        short_id: "1234abcd".into(),
        enabled: true,
        settings: Default::default(),
        protocol_config: ProtocolConfig::VlessReality,
        users: if user {
            vec![Access {
                user_id: 7,
                uuid: input.credentials.public_uuid,
                credential: String::new(),
            }]
        } else {
            vec![]
        },
    };
    node.settings.listen = "127.0.0.1".into();
    node.settings.reality.handshake_server = Some("127.0.0.1".into());
    node.settings.reality.handshake_port = port(input, "handshake")?;
    Ok(node)
}
fn endpoint(input: &Input, id: i64, role: &str) -> Result<ManagedEndpointSnapshot> {
    Ok(ManagedEndpointSnapshot {
        version_id: Uuid::from_u128(1000 + id as u128),
        server_id: id,
        node: node(input, id, role, false)?,
    })
}
fn managed(input: &Input, id: i64, role: &str, uuid: u128) -> Result<PathHop> {
    Ok(PathHop::Managed {
        endpoint: Box::new(endpoint(input, id, role)?),
        relay_uuid: Uuid::from_u128(uuid),
    })
}
fn external(input: &Input) -> Result<PathHop> {
    Ok(PathHop::External {
        source_id: 1,
        identity_epoch: 1,
        node_id: Uuid::from_u128(31),
        version_id: Uuid::from_u128(131),
        source_revision_id: Uuid::from_u128(9000),
        outbound: serde_json::from_value(
            json!({"type":"http","server":"127.0.0.1","server_port":port(input,"x")?,"username":input.credentials.x_username,
        "password":input.credentials.x_password,"path":"","headers":{}}),
        )?,
    })
}
fn accept(
    input: &Input,
    chain: i64,
    position: u8,
    id: i64,
    role: &str,
    uuid: u128,
) -> Result<ManagedAcceptance> {
    Ok(ManagedAcceptance {
        endpoint: endpoint(input, id, role)?,
        chain_id: chain,
        generation: 1,
        position,
        relay_uuid: Uuid::from_u128(uuid),
    })
}

fn output_case(input: &Input, parent: &Path, name: &str, four: bool) -> Result<Value> {
    let path = parent.join(name);
    directory(&path)?;
    let entry = node(input, 1, "A", true)?;
    let chain = if four { 12 } else { 11 };
    let mut hops = Vec::new();
    if four {
        hops.push(managed(input, 2, "M", 1202)?);
    }
    hops.push(external(input)?);
    hops.push(managed(input, 3, "B", if four { 1203 } else { 1103 })?);
    let ordered = OrderedPath {
        chain_id: chain,
        generation: 1,
        entry_node_id: 1,
        entry_server_id: 1,
        hops,
        active: true,
    };
    let control = ProbeControl {
        listen_port: port(input, "controller")?,
        secret: input.credentials.controller_secret.clone(),
    };
    let middle = if four {
        vec![accept(input, chain, 1, 2, "M", 1202)?]
    } else {
        vec![]
    };
    let products = [
        (
            "A",
            compile_server_with_paths(
                std::slice::from_ref(&entry),
                &[],
                std::slice::from_ref(&ordered),
                &[],
                Some(&control),
            )?,
        ),
        (
            "M",
            compile_server_with_paths(&[node(input, 2, "M", false)?], &[], &[], &middle, None)?,
        ),
        (
            "B",
            compile_server_with_paths(
                &[node(input, 3, "B", false)?],
                &[],
                &[],
                &[accept(
                    input,
                    chain,
                    if four { 3 } else { 2 },
                    3,
                    "B",
                    if four { 1203 } else { 1103 },
                )?],
                None,
            )?,
        ),
        ("client", compile_client(std::slice::from_ref(&entry), 7)?),
    ];
    let mut files = BTreeMap::new();
    for (role, product) in products {
        let product_name = format!("{name}/product-{role}.json");
        private_write(&parent.join(&product_name), product.as_bytes())?;
        let mut fixture: Value = serde_json::from_str(&product)?;
        let (field, from, to) = if role == "client" {
            let from = fixture["inbounds"][0]["listen_port"].clone();
            let to = json!(port(input, "client")?);
            fixture["inbounds"][0]["listen_port"] = to.clone();
            ("/inbounds/0/listen_port", from, to)
        } else {
            let from = fixture["experimental"]["v2ray_api"]["listen"].clone();
            let to = json!(format!(
                "127.0.0.1:{}",
                port(input, &format!("stats_{}", role.to_ascii_lowercase()))?
            ));
            fixture["experimental"]["v2ray_api"]["listen"] = to.clone();
            ("/experimental/v2ray_api/listen", from, to)
        };
        let fixture = format!("{}\n", serde_json::to_string_pretty(&fixture)?);
        let fixture_name = format!("{name}/fixture-{role}.json");
        private_write(&parent.join(&fixture_name), fixture.as_bytes())?;
        files.insert(role,json!({"product_file":product_name,"product_sha256":sha(product.as_bytes()),"fixture_file":fixture_name,"fixture_sha256":sha(fixture.as_bytes()),"transforms":[{"pointer":field,"from":from,"to":to}]}));
    }
    let mut versions = Vec::new();
    if four {
        versions.push(json!({"node_id":2,"version_id":Uuid::from_u128(1002)}));
    }
    versions.push(json!({"node_id":3,"version_id":Uuid::from_u128(1003)}));
    Ok(
        json!({"name":name,"chain_id":chain,"generation":1,"final_tag":ordered.final_tag(),"entry_stat":"u7_n1","files":files,
        "managed_endpoint_versions":versions,
        "external_version":{"source_id":1,"identity_epoch":1,"node_id":Uuid::from_u128(31),"version_id":Uuid::from_u128(131),"source_revision_id":Uuid::from_u128(9000)}}),
    )
}
fn execute() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("expected absolute input and new output directory".into());
    }
    let input_file = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    if !input_file.is_absolute() || !output.is_absolute() {
        return Err("absolute paths are required".into());
    }
    let size = fs::metadata(&input_file)?.len();
    if size > 2 * 1024 * 1024 {
        return Err("fixture input exceeds budget".into());
    }
    let bytes = fs::read(input_file)?;
    let input: Input = serde_json::from_slice(&bytes)?;
    if input.schema != 1
        || !input.test_only
        || input.runtime_version != "1.14.2"
        || input.native_binary_sha256.len() != 64
        || !input
            .native_binary_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid test-only runtime contract".into());
    }
    let names = BTreeSet::from([
        "a",
        "m",
        "b",
        "x",
        "client",
        "stats_a",
        "stats_m",
        "stats_b",
        "https",
        "tcp_echo",
        "udp_echo",
        "handshake",
        "controller",
    ]);
    if input
        .ports
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != names
        || input.ports.values().any(|port| *port < 1024)
        || input.ports.values().copied().collect::<BTreeSet<_>>().len() != names.len()
        || port(&input, "controller")? != 18086
    {
        return Err("invalid or overlapping test ports".into());
    }
    if input
        .node_keys
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != BTreeSet::from(["A", "M", "B"])
        || input.credentials.public_uuid.is_nil()
        || !input.credentials.x_username.starts_with("TEST_ONLY_")
        || !input.credentials.x_password.starts_with("TEST_ONLY_")
        || input.credentials.controller_secret.len() != 64
        || !input
            .credentials
            .controller_secret
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid test-only credentials".into());
    }
    for key in input.node_keys.values() {
        for encoded in [&key.private_key, &key.public_key] {
            if !URL_SAFE_NO_PAD
                .decode(encoded)
                .is_ok_and(|value| value.len() == 32)
            {
                return Err("invalid test key".into());
            }
        }
    }
    directory(&output)?;
    let cases = [
        output_case(&input, &output, "three", false)?,
        output_case(&input, &output, "four", true)?,
    ];
    let sources = [
        ("lib.rs", include_bytes!("../src/lib.rs").as_slice()),
        ("client.rs", include_bytes!("../src/client.rs").as_slice()),
        ("relays.rs", include_bytes!("../src/relays.rs").as_slice()),
        (
            "external.rs",
            include_bytes!("../src/external.rs").as_slice(),
        ),
        (
            "protocols.rs",
            include_bytes!("../src/protocols.rs").as_slice(),
        ),
        (
            "settings/mod.rs",
            include_bytes!("../src/settings/mod.rs").as_slice(),
        ),
        (
            "settings/apply.rs",
            include_bytes!("../src/settings/apply.rs").as_slice(),
        ),
        (
            "settings/transport.rs",
            include_bytes!("../src/settings/transport.rs").as_slice(),
        ),
        (
            "settings/validate.rs",
            include_bytes!("../src/settings/validate.rs").as_slice(),
        ),
        (
            "certificates.rs",
            include_bytes!("../src/certificates.rs").as_slice(),
        ),
        (
            "paths/mod.rs",
            include_bytes!("../src/paths/mod.rs").as_slice(),
        ),
        (
            "paths/model.rs",
            include_bytes!("../src/paths/model.rs").as_slice(),
        ),
        (
            "paths/network.rs",
            include_bytes!("../src/paths/network.rs").as_slice(),
        ),
        (
            "paths/context.rs",
            include_bytes!("../src/paths/context.rs").as_slice(),
        ),
        (
            "paths/validate.rs",
            include_bytes!("../src/paths/validate.rs").as_slice(),
        ),
        (
            "paths/render.rs",
            include_bytes!("../src/paths/render.rs").as_slice(),
        ),
        (
            "paths/transport_validation.rs",
            include_bytes!("../src/paths/transport_validation.rs").as_slice(),
        ),
        (
            "paths/outbound_validation.rs",
            include_bytes!("../src/paths/outbound_validation.rs").as_slice(),
        ),
    ];
    let sources: BTreeMap<_, _> = sources
        .into_iter()
        .map(|(name, bytes)| (name, sha(bytes)))
        .collect();
    let manifest = json!({"schema":1,"test_only":true,"runtime_version":"1.14.2","native_binary_sha256":input.native_binary_sha256,"compiler_input_sha256":sha(&bytes),
        "generator_source_sha256":sha(include_bytes!("ordered_native_fixture.rs")),"compiler_sources":sources,"cases":cases,
        "scope":"product routing graph with explicitly relocated fixture statistics and mixed listener ports; not production bundle or Agent deployment acceptance"});
    private_write(
        &output.join("compilation-manifest.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}
fn main() {
    if execute().is_err() {
        eprintln!("test-only product fixture generation failed");
        std::process::exit(1);
    }
}

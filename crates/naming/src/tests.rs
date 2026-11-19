use super::{
    model::{self, Config, Observation, Rule},
    providers::Providers,
};
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr};
use uuid::Uuid;

mod lifecycle;
mod multicloud;
mod provider;
mod scheduling;

const ZONE: &str = "00000000000000000000000000000001";
const RECORD: &str = "00000000000000000000000000000002";
const TOKEN: &str = "TEST_ONLY_CLOUDFLARE_TOKEN";

fn config() -> Config {
    Config {
        address_source: model::AddressSource::Agent,
        manual_ip: None,
        credential_id: None,
        interface_name: None,
        account_id: None,
        provider: model::Provider::Cloudflare,
        line: String::new(),
        name: "测试规则".into(),
        server_id: 1,
        zone_id: ZONE.into(),
        record_name: "node.example.com".into(),
        record_type: "A".into(),
        ttl: 300,
        proxied: false,
        interval_secs: 300,
        enabled: true,
        adopt_existing: false,
    }
}

fn rule() -> Rule {
    Rule {
        id: Uuid::new_v4(),
        config: config(),
        api_token: TOKEN.into(),
        access_key_id: String::new(),
        access_key_secret: String::new(),
        revision: 1,
        record_id: None,
        last_ip: None,
        last_success_at: None,
        attempted_at: None,
        next_run_at: 0,
        failures: 0,
        status: "pending".into(),
        error_code: None,
        lease_until: 0,
    }
}

// Synthetic numeric addresses are only payloads to the loopback provider;
// they are never contacted or resolved by these tests.
fn public_ip(last: u8) -> IpAddr {
    Ipv4Addr::new(1, 2, 3, last).into()
}

#[test]
fn domains_tokens_and_ttl_are_validated_before_storage() {
    assert_eq!(
        model::domain(" *.EXAMPLE.com. ").as_deref(),
        Some("*.example.com")
    );
    let idn = model::domain("节点.example.com").unwrap();
    assert!(idn.starts_with("xn--") && idn.ends_with(".example.com"));
    for name in [
        "https://example.com",
        "example.com/path",
        "example.com:443",
        "a..example.com",
        "example.com@evil.test",
        "127.0.0.1",
        "*foo.example.com",
        "_srv.example.com",
        "example.com%2fevil",
        "example.com?x",
    ] {
        assert!(model::domain(name).is_none(), "{name}");
    }
    let mut spec = config();
    for ttl in [0, 2, 30, 59, 86401] {
        spec.ttl = ttl;
        assert!(spec.normalize().is_err());
    }
    spec.proxied = true;
    spec.normalize().unwrap();
    assert_eq!(spec.ttl, 1);
    assert!(model::token("invalid\nheader-value").is_err());
    assert!(model::token(TOKEN).is_ok());
    let serialized = serde_json::to_string(&rule()).unwrap();
    assert!(!serialized.contains(TOKEN));
    assert!(!serialized.contains("api_token"));
}

#[test]
fn selection_keeps_a_still_reported_address_and_requires_recent_authenticated_reports() {
    let now = 10000;
    let mut info = Observation {
        name: "测试".into(),
        static_info: json!({"ip_addresses":[public_ip(8),public_ip(3),"127.0.0.1","192.0.2.1","2001:db8::1"]}),
        static_info_received_at: Some(now),
        last_seen: Some(now),
        deleted_at: None,
        retiring: false,
    };
    let spec = config();
    assert_eq!(info.select(&spec, None, now).unwrap(), public_ip(3));
    assert_eq!(
        info.select(&spec, Some(&public_ip(8).to_string()), now)
            .unwrap(),
        public_ip(8)
    );
    let mut v6 = config();
    v6.record_type = "AAAA".into();
    assert_eq!(info.select(&v6, None, now), Err("no_public_ip"));
    info.static_info_received_at = None;
    assert_eq!(info.select(&spec, None, now), Err("ip_stale"));
    info.static_info_received_at = Some(now - 601);
    assert_eq!(info.select(&spec, None, now), Err("ip_stale"));
    info.static_info_received_at = Some(now);
    info.last_seen = Some(now - 61);
    assert_eq!(info.select(&spec, None, now), Err("server_offline"));
    info.retiring = true;
    assert_eq!(info.select(&spec, None, now), Err("server_retired"));
}

#[test]
fn ddns_rejects_new_documentation_benchmark_and_deprecated_ipv4_ranges() {
    let now = 10000;
    let mut info = Observation {
        name: "TEST_ONLY public filter".into(),
        static_info: Value::Null,
        static_info_received_at: Some(now),
        last_seen: Some(now),
        deleted_at: None,
        retiring: false,
    };
    for address in [
        "3fff::1",
        "3fff:0fff:ffff::1",
        "2001:2::1",
        "2001:2:0:ffff::1",
        "192.88.99.1",
    ] {
        info.static_info = json!({"ip_addresses":[address]});
        let mut spec = config();
        spec.record_type = if address.contains(':') { "AAAA" } else { "A" }.into();
        assert_eq!(
            info.select(&spec, None, now),
            Err("no_public_ip"),
            "{address}"
        );
    }
    info.static_info = json!({"ip_addresses":["2001:200::1","3fff:1000::1"]});
    let mut spec = config();
    spec.record_type = "AAAA".into();
    assert!(info.select(&spec, None, now).is_ok());
}

struct TempPath(std::path::PathBuf);

impl TempPath {
    fn new(prefix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("apsp-{prefix}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(dir)
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn builtin_lego_verifier_enforces_readonly_path_architecture_digest_and_version_identity() {
    use crate::{
        acme::{BUILTIN_LEGO_PATH, BUILTIN_LEGO_VERSION},
        acme_executor::{
            BinaryFormat, BuiltinTargetSpec, Invocation, PINNED_TARGETS, build_lego_command,
            check_binary_header, verify_builtin_binary, verify_version_identity,
        },
    };
    use sha2::{Digest, Sha256};
    use std::{os::unix::fs::PermissionsExt, path::Path};

    assert_eq!(BUILTIN_LEGO_VERSION, "5.6.0");
    assert_eq!(BUILTIN_LEGO_PATH, "/usr/local/libexec/apsp/lego");
    for (os, arch) in [("linux", "x86_64"), ("linux", "aarch64")] {
        let spec = PINNED_TARGETS
            .iter()
            .find(|item| item.os == os && item.arch == arch)
            .expect("pinned target spec");
        assert_eq!(spec.sha256.len(), 64);
        assert!(spec.size > 1_000_000);
    }

    let temp = TempPath::new("acme-cmd");
    let storage = temp.0.join("storage");
    let domains = vec![
        "network.example.com".to_owned(),
        "*.network.example.com".to_owned(),
    ];
    let command = build_lego_command(
        Path::new(BUILTIN_LEGO_PATH),
        &Invocation {
            workspace: &temp.0,
            storage: &storage,
            email: "ops@example.com",
            staging: true,
            domains: &domains,
            cloudflare_api_token: TOKEN,
        },
    );
    let std_cmd = command.as_std();
    assert_eq!(std_cmd.get_program(), Path::new(BUILTIN_LEGO_PATH));
    let args: Vec<String> = std_cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "run");
    assert!(args.contains(&format!("--path={}", storage.display())));
    assert!(args.contains(&"--email=ops@example.com".to_owned()));
    assert!(args.contains(&"--dns=cloudflare".to_owned()));
    assert!(args.contains(&"--key-type=ec256".to_owned()));
    assert!(args.contains(&"--accept-tos".to_owned()));
    assert!(
        args.contains(
            &"--server=https://acme-staging-v02.api.letsencrypt.org/directory".to_owned()
        )
    );
    assert!(args.contains(&"--domains=network.example.com".to_owned()));
    assert!(args.contains(&"--domains=*.network.example.com".to_owned()));
    let envs: std::collections::BTreeMap<String, Option<String>> = std_cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|val| val.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(envs.len(), 4);
    assert_eq!(envs.get("CF_DNS_API_TOKEN"), Some(&Some(TOKEN.to_owned())));
    assert_eq!(
        envs.get("CLOUDFLARE_DNS_API_TOKEN"),
        Some(&Some(TOKEN.to_owned()))
    );

    let mut x86_elf = vec![0u8; 32];
    x86_elf[..4].copy_from_slice(b"\x7fELF");
    x86_elf[4] = 2;
    x86_elf[5] = 1;
    x86_elf[16..18].copy_from_slice(&2u16.to_le_bytes());
    x86_elf[18..20].copy_from_slice(&62u16.to_le_bytes());
    assert!(check_binary_header(&x86_elf, BinaryFormat::Elf64Le { e_machine: 62 }).is_ok());
    assert_eq!(
        check_binary_header(&x86_elf, BinaryFormat::Elf64Le { e_machine: 183 }),
        Err("issuer_binary_architecture_mismatch")
    );

    let spec = BuiltinTargetSpec {
        os: "linux",
        arch: "x86_64",
        upstream_platform: "linux/amd64",
        size: x86_elf.len() as u64,
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        format: BinaryFormat::Elf64Le { e_machine: 62 },
    };
    assert_eq!(
        verify_builtin_binary(Path::new("relative/lego"), &spec, "5.6.0", false)
            .await
            .unwrap_err()
            .code,
        "issuer_builtin_path_invalid"
    );
    assert_eq!(
        verify_builtin_binary(&temp.0.join("missing"), &spec, "5.6.0", false)
            .await
            .unwrap_err()
            .code,
        "issuer_builtin_binary_missing"
    );

    let writable = temp.0.join("writable-lego");
    std::fs::write(&writable, &x86_elf).unwrap();
    std::fs::set_permissions(&writable, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        verify_builtin_binary(&writable, &spec, "5.6.0", false)
            .await
            .unwrap_err()
            .code,
        "issuer_builtin_permissions_invalid"
    );

    let symlink_path = temp.0.join("symlink-lego");
    std::os::unix::fs::symlink(&writable, &symlink_path).unwrap();
    assert_eq!(
        verify_builtin_binary(&symlink_path, &spec, "5.6.0", false)
            .await
            .unwrap_err()
            .code,
        "issuer_builtin_binary_invalid"
    );

    let readonly_wrong_arch = temp.0.join("wrong-arch-lego");
    let mut arm_elf = x86_elf.clone();
    arm_elf[18..20].copy_from_slice(&183u16.to_le_bytes());
    let arm_digest = format!("{:x}", Sha256::digest(&arm_elf));
    std::fs::write(&readonly_wrong_arch, &arm_elf).unwrap();
    std::fs::set_permissions(&readonly_wrong_arch, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert_eq!(
        verify_builtin_binary(&readonly_wrong_arch, &spec, "5.6.0", false)
            .await
            .unwrap_err()
            .code,
        "issuer_binary_architecture_mismatch"
    );

    let readonly_bad_digest = temp.0.join("bad-digest-lego");
    std::fs::write(&readonly_bad_digest, &x86_elf).unwrap();
    std::fs::set_permissions(&readonly_bad_digest, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert_eq!(
        verify_builtin_binary(&readonly_bad_digest, &spec, "5.6.0", false)
            .await
            .unwrap_err()
            .code,
        "issuer_builtin_binary_invalid"
    );
    let _ = arm_digest;

    let script_ok = temp.0.join("version-ok");
    std::fs::write(
        &script_ok,
        "#!/bin/sh\necho 'lego version 5.6.0 linux/amd64'\n",
    )
    .unwrap();
    std::fs::set_permissions(&script_ok, std::fs::Permissions::from_mode(0o500)).unwrap();
    verify_version_identity(&script_ok, "5.6.0", "linux/amd64")
        .await
        .unwrap();
    assert_eq!(
        verify_version_identity(&script_ok, "4.35.2", "linux/amd64")
            .await
            .unwrap_err()
            .code,
        "issuer_tool_identity_invalid"
    );
}

#[cfg(unix)]
fn generate_self_signed_pair(dir: &std::path::Path, cn: &str) -> (String, String) {
    let cert_path = dir.join("generated.crt");
    let key_path = dir.join("generated.key");
    let status = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:prime256v1",
            "-nodes",
            "-days",
            "90",
            "-subj",
            &format!("/CN={cn}"),
            "-addext",
            "subjectAltName=DNS:network.example.com,DNS:*.network.example.com",
            "-keyout",
        ])
        .arg(&key_path)
        .arg("-out")
        .arg(&cert_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("openssl available for certificate fixture");
    assert!(
        status.success(),
        "openssl failed generating test certificate"
    );
    let cert = std::fs::read_to_string(&cert_path).unwrap();
    let key = std::fs::read_to_string(&key_path).unwrap();
    let _ = std::fs::remove_file(&cert_path);
    let _ = std::fs::remove_file(&key_path);
    (cert, key)
}

#[cfg(unix)]
#[sqlx::test(migrations = "../panel/migrations")]
async fn acme_runner_issues_rotates_restores_account_cleans_workspace_and_rejects_legacy_versions(
    pool: sqlx::PgPool,
) {
    use crate::{
        acme::{self, BUILTIN_LEGO_VERSION, Plan},
        acme_executor::{BoxFuture, ConfiguredRunner, ExecutionFailure, Invocation, IssuerRunner},
        control_center,
    };
    use reqwest::{Method, StatusCode};
    use sqlx::Row;
    use std::{
        os::unix::fs::MetadataExt,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    struct TestIssuerRunner {
        calls: AtomicUsize,
        succeed: AtomicBool,
        restored_accounts_seen: Mutex<Vec<bool>>,
    }

    impl IssuerRunner for TestIssuerRunner {
        fn version(&self) -> &str {
            BUILTIN_LEGO_VERSION
        }

        fn verify_ready(&self) -> BoxFuture<'_, Result<(), ExecutionFailure>> {
            Box::pin(async { Ok(()) })
        }

        fn run<'a>(
            &'a self,
            invocation: Invocation<'a>,
        ) -> BoxFuture<'a, Result<bool, ExecutionFailure>> {
            let call_num = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            let should_succeed = self.succeed.load(Ordering::SeqCst);
            let workspace = invocation.workspace.to_path_buf();
            let storage = invocation.storage.to_path_buf();
            let email = invocation.email.to_owned();
            let staging = invocation.staging;
            let domains = invocation.domains.to_vec();
            let token = invocation.cloudflare_api_token.to_owned();
            Box::pin(async move {
                assert!(
                    !workspace.join("lego").exists(),
                    "executor must not copy lego binary into per-job workspace"
                );
                assert_eq!(std::fs::metadata(&workspace).unwrap().mode() & 0o777, 0o700);
                assert_eq!(std::fs::metadata(&storage).unwrap().mode() & 0o777, 0o700);
                assert_eq!(email, "ops@example.com");
                assert!(staging);
                assert_eq!(domains, vec!["network.example.com".to_owned()]);
                assert_eq!(token, TOKEN);

                let acct_dir = storage
                    .join("accounts/acme-staging-v02.api.letsencrypt.org")
                    .join(&email);
                let acct_json = acct_dir.join("account.json");
                let restored = acct_json.exists();
                if restored {
                    assert_eq!(std::fs::metadata(&acct_json).unwrap().mode() & 0o777, 0o600);
                } else {
                    std::fs::create_dir_all(&acct_dir).unwrap();
                    std::fs::write(&acct_json, r#"{"email":"ops@example.com"}"#).unwrap();
                    std::fs::write(
                        acct_dir.join("ops@example.com.key"),
                        "TEST_ONLY_ACCOUNT_KEY",
                    )
                    .unwrap();
                }
                self.restored_accounts_seen.lock().unwrap().push(restored);
                if !should_succeed {
                    return Ok(false);
                }
                let certs_dir = storage.join("certificates");
                std::fs::create_dir_all(&certs_dir).unwrap();
                let (cert_pem, key_pem) = generate_self_signed_pair(
                    &workspace,
                    &format!("round-{call_num}.network.example.com"),
                );
                // Also write `.issuer.crt` as lego v5 does so `result_files` proves it skips `.issuer.crt`.
                let (issuer_pem, _) =
                    generate_self_signed_pair(&workspace, "issuer.example.invalid");
                std::fs::write(certs_dir.join("network.example.com.issuer.crt"), issuer_pem)
                    .unwrap();
                std::fs::write(certs_dir.join("network.example.com.crt"), cert_pem).unwrap();
                std::fs::write(certs_dir.join("network.example.com.key"), key_pem).unwrap();
                Ok(true)
            })
        }
    }

    let data_dir = TempPath::new("acme-data");
    let runner = Arc::new(TestIssuerRunner {
        calls: AtomicUsize::new(0),
        succeed: AtomicBool::new(true),
        restored_accounts_seen: Mutex::new(Vec::new()),
    });
    let mut state = apsp_governance::state(
        pool.clone(),
        crate::config::Config {
            database_url: String::new(),
            listen: "127.0.0.1:8080".parse().unwrap(),
            public_url: "http://127.0.0.1:8080".into(),
            data_dir: data_dir.0.clone(),
            admin_password: Some("test-admin-password".into()),
        },
    )
    .await
    .unwrap();
    state.domain_states = state.domain_states.with(ConfiguredRunner(
        Arc::clone(&runner) as Arc<dyn IssuerRunner>
    ));

    let now = apsp_protocol::now_timestamp();
    let session_token = crate::auth::random_token();
    let session_hash = crate::auth::hash_token(&session_token);
    sqlx::query("INSERT INTO sessions(token_hash,admin_id,expires_at) VALUES($1,1,$2)")
        .bind(&session_hash)
        .bind(now + 3600)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3)",
    )
    .bind(&session_hash)
    .bind(now)
    .bind(now + 300)
    .execute(&pool)
    .await
    .unwrap();
    let cookie = format!("apsp_session={session_token}");

    let server: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY acme') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let dns_cred = control_center::credentials::store_generated(
        &pool,
        "TEST_ONLY Cloudflare DNS",
        "dns",
        json!({"provider": "cloudflare", "api_token": TOKEN}),
    )
    .await
    .unwrap();
    let ddns_rule = Uuid::new_v4();
    let mut ddns_cfg = config();
    ddns_cfg.server_id = server;
    sqlx::query("INSERT INTO ddns_rules(id,server_id,config,api_token) VALUES($1,$2,$3,'')")
        .bind(ddns_rule)
        .bind(server)
        .bind(json!(ddns_cfg))
        .execute(&pool)
        .await
        .unwrap();

    let domain_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO network_documents(id,kind,config,revision,created_at,updated_at) VALUES($1,'domain',$2,1,$3,$3)",
    )
    .bind(domain_id)
    .bind(json!({
        "kind": "domain",
        "name": "network.example.com",
        "server_ids": [server],
        "ddns_rule_ids": [ddns_rule],
        "applications": [],
        "maintainer": "TEST_ONLY",
        "notes": ""
    }))
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    let cert_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO network_documents(id,kind,config,revision,created_at,updated_at) VALUES($1,'certificate',$2,1,$3,$3)",
    )
    .bind(cert_id)
    .bind(json!({
        "kind": "certificate",
        "name": "TEST_ONLY cert",
        "domain_ids": [domain_id],
        "maintainer": "TEST_ONLY",
        "issuer": "Let's Encrypt",
        "targets": [{
            "server_id": server,
            "service": "nginx",
            "domain": "network.example.com",
            "port": 443,
            "certificate_path": "/srv/example-certificates/tls.crt",
            "private_key_path": "/srv/example-certificates/tls.key"
        }],
        "renewal": {
            "mode": "dns01",
            "responsibility": "panel",
            "ddns_rule_id": ddns_rule
        }
    }))
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = crate::routes().with_state(state.clone());
    let http_task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = apsp_panel_host::http::Outbound::default().build().unwrap();
    let send = |method: reqwest::Method, uri: &str, body: Option<Value>| {
        let client = client.clone();
        let cookie = cookie.clone();
        let url = format!("http://{addr}{uri}");
        async move {
            let mut builder = client
                .request(method, url)
                .header(reqwest::header::COOKIE, cookie);
            if let Some(payload) = body {
                builder = builder.json(&payload);
            }
            let resp = builder.send().await.unwrap();
            let status = resp.status();
            let json: Value = resp.json().await.unwrap_or(Value::Null);
            (status, json)
        }
    };

    // Save plan with pinned built-in version 5.6.0.
    let valid_plan = Plan {
        tool_version: BUILTIN_LEGO_VERSION.into(),
        credential_id: dns_cred,
        email: "ops@example.com".into(),
        staging: true,
        terms_accepted: true,
        license_accepted: true,
        automatic_renewal: false,
        renew_before_days: 30,
    };
    let (status, saved) = send(
        Method::PUT,
        &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
        Some(json!({"config": valid_plan, "revision": null})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["revision"], 1);

    // Trigger issuance #1 and run worker iteration.
    let (status, queued1) = send(
        Method::POST,
        &format!("/api/network-configuration/certificates/{cert_id}/issue"),
        Some(json!({"confirmed": true, "remote_reconciled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{queued1}");
    let job1 = Uuid::parse_str(queued1["operation_id"].as_str().unwrap()).unwrap();
    acme::iteration(&state).await.unwrap();

    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert!(
        !data_dir
            .0
            .join("network-acme-work")
            .join(job1.to_string())
            .exists()
    );
    let row1 = sqlx::query(
        "SELECT status,result,version_id,key_secret_ref FROM network_acme_jobs WHERE id=$1",
    )
    .bind(job1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row1.get::<String, _>("status"), "succeeded");
    let res1: Value = row1.get("result");
    assert_eq!(res1["issued"], true);
    assert_eq!(res1["saved"], true);
    assert_eq!(res1["deployed"], false);
    assert_eq!(res1["handshake_verified"], false);
    assert_eq!(res1["tool"], "lego");
    assert_eq!(res1["tool_version"], BUILTIN_LEGO_VERSION);
    assert_eq!(res1["tool_supply"], "panel_builtin_verified_binary");
    assert_eq!(res1["temporary_material_cleanup"], "confirmed");
    let v1: Uuid = row1.get("version_id");
    let k1: Uuid = row1.get("key_secret_ref");

    // Issuance #2 (rotation / renewal): verifies encrypted account restore & version history preservation.
    let (status, queued2) = send(
        Method::POST,
        &format!("/api/network-configuration/certificates/{cert_id}/issue"),
        Some(json!({"confirmed": true, "remote_reconciled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{queued2}");
    let job2 = Uuid::parse_str(queued2["operation_id"].as_str().unwrap()).unwrap();
    acme::iteration(&state).await.unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *runner.restored_accounts_seen.lock().unwrap(),
        vec![false, true],
        "second issuance must restore encrypted lego account files from credential_entries"
    );
    let versions: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT id,revision FROM network_certificate_versions WHERE certificate_id=$1 ORDER BY revision",
    )
    .bind(cert_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0], (v1, 1));
    let v2 = versions[1].0;
    let active: Option<Uuid> =
        sqlx::query_scalar("SELECT active_version FROM network_documents WHERE id=$1")
            .bind(cert_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(active, Some(v2));

    // Deployment preview with omitted key_credential_id automatically associates the issued key reference.
    let (status, preview) = send(
        Method::POST,
        &format!("/api/network-configuration/certificates/{cert_id}/deployment-preview"),
        Some(json!({
            "version_id": v2,
            "key_credential_id": null,
            "target_indexes": [0],
            "adopt_existing": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");

    // Manual selection back to v1 requires explicit confirmation and does not auto-deploy.
    let current_rev: i64 = sqlx::query_scalar("SELECT revision FROM network_documents WHERE id=$1")
        .bind(cert_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let (status, selected) = send(
        Method::POST,
        &format!("/api/network-configuration/certificates/{cert_id}/select-version"),
        Some(json!({"version_id": v1, "revision": current_rev, "confirmed": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{selected}");
    assert_eq!(selected["active_version"], json!(v1));
    assert_eq!(selected["deployment"], "requires_explicit_target_preview");
    let _ = k1;

    // Simulate legacy stored plan with mismatched tool_version ("4.20.0") & automatic_renewal:
    // GET acme-plan reports mismatch remediation, POST issue refuses with clear message,
    // and background auto-renewal fails pre-execution without invoking lego or spoofing old version.
    sqlx::query("UPDATE network_documents SET active_version=NULL WHERE id=$1")
        .bind(cert_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut legacy_plan = valid_plan.clone();
    legacy_plan.tool_version = "4.20.0".into();
    legacy_plan.automatic_renewal = true;
    sqlx::query("UPDATE network_acme_plans SET config=$2,next_run_at=0 WHERE certificate_id=$1")
        .bind(cert_id)
        .bind(json!(legacy_plan))
        .execute(&pool)
        .await
        .unwrap();

    let (status, plan_view) = send(
        Method::GET,
        &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan_view["builtin_tool_version"], BUILTIN_LEGO_VERSION);
    assert_eq!(
        plan_view["tool_version_mismatch"]["code"],
        "issuer_tool_version_mismatch"
    );
    assert_eq!(
        plan_view["tool_version_mismatch"]["configured_tool_version"],
        "4.20.0"
    );

    let (status, issue_err) = send(
        Method::POST,
        &format!("/api/network-configuration/certificates/{cert_id}/issue"),
        Some(json!({"confirmed": true, "remote_reconciled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        issue_err["error"]
            .as_str()
            .is_some_and(|msg| msg.contains("4.20.0") && msg.contains(BUILTIN_LEGO_VERSION))
    );

    acme::iteration(&state).await.unwrap();
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "mismatched legacy plan must never execute built-in lego"
    );
    let legacy_job = sqlx::query(
        "SELECT status,result FROM network_acme_jobs WHERE certificate_id=$1 AND id NOT IN ($2,$3)",
    )
    .bind(cert_id)
    .bind(job1)
    .bind(job2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(legacy_job.get::<String, _>("status"), "failed");
    let legacy_res: Value = legacy_job.get("result");
    assert_eq!(legacy_res["error_code"], "issuer_tool_version_mismatch");
    assert_eq!(legacy_res["requested_tool_version"], "4.20.0");
    assert_eq!(legacy_res["builtin_tool_version"], BUILTIN_LEGO_VERSION);
    assert_eq!(legacy_res["issued"], false);
    assert_eq!(legacy_res["cleanup_required"], false);
    assert!(
        legacy_res["error"]
            .as_str()
            .is_some_and(|msg| msg.contains(BUILTIN_LEGO_VERSION))
    );

    // Fix plan back to 5.6.0, simulate runner validation failure -> status='unknown' + manual reconciliation.
    let (status, _) = send(
        Method::PUT,
        &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
        Some(json!({"config": valid_plan, "revision": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    runner.succeed.store(false, Ordering::SeqCst);
    let (status, queued3) = send(
        Method::POST,
        &format!("/api/network-configuration/certificates/{cert_id}/issue"),
        Some(json!({"confirmed": true, "remote_reconciled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{queued3}");
    let job4 = Uuid::parse_str(queued3["operation_id"].as_str().unwrap()).unwrap();
    acme::iteration(&state).await.unwrap();
    let unknown_job = sqlx::query("SELECT status,result FROM network_acme_jobs WHERE id=$1")
        .bind(job4)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(unknown_job.get::<String, _>("status"), "unknown");
    assert_eq!(
        unknown_job.get::<Value, _>("result")["cleanup_required"],
        true
    );

    // Without remote_reconciled, re-issuing is blocked; with remote_reconciled=true, it queues cleanly.
    let (status, _) = send(
        Method::POST,
        &format!("/api/network-configuration/certificates/{cert_id}/issue"),
        Some(json!({"confirmed": true, "remote_reconciled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        Method::POST,
        &format!("/api/network-configuration/certificates/{cert_id}/issue"),
        Some(json!({"confirmed": true, "remote_reconciled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    http_task.abort();
}

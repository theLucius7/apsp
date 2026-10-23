use super::*;
use crate::Config;
#[cfg(unix)]
use crate::{artifacts::PanelClient, identity};
#[cfg(unix)]
use apsp_protocol::{Bundle, EnrollRequest};
#[cfg(unix)]
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
#[cfg(unix)]
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::{collections::BTreeMap, time::Duration};
#[cfg(unix)]
use tokio::time::timeout;

#[test]
fn older_configuration_keeps_the_default_public_trust() -> Result<()> {
    let config: Config = toml::from_str("panel_url = 'https://panel.example.invalid'\n")?;
    assert!(config.panel_ca_file.is_none());
    config.validate()?;
    assert!(websocket_connector(None)?.is_none());
    let mut changed = config;
    changed.panel_ca_file = Some(Path::new("/TEST_ONLY/trust/panel-ca.pem").into());
    let restored: Config = toml::from_str(&toml::to_string(&changed)?)?;
    assert_eq!(restored.panel_ca_file, changed.panel_ca_file);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn private_panel_ca_registers_and_downloads_without_changing_other_clients() -> Result<()> {
    use super::test_support::{CertificateKind, HttpServer, Material};
    let material = Material::new().await?;
    let mut enrollment = HttpServer::start(
        &material,
        CertificateKind::Valid,
        200,
        br#"{"server_id":7}"#.to_vec(),
    )
    .await?;
    let config = material.config(&enrollment.origin);
    assert_eq!(identity::enroll(&config, "TEST_ONLY_enrollment").await?, 7);
    let received = enrollment.next().await?;
    assert_eq!(received.target, "/api/agent/v1/enroll");
    let request: EnrollRequest = serde_json::from_slice(&received.body)?;
    assert_eq!(request.token, "TEST_ONLY_enrollment");
    let identity = identity::load(&config)?;
    assert_eq!(identity.server_id, 7);
    assert_eq!(
        request.device_public_key,
        URL_SAFE_NO_PAD.encode(identity.signing_key.verifying_key().as_bytes())
    );
    enrollment.stop().await?;

    let bundle = Bundle {
        files: BTreeMap::from([("config.json".into(), "{}".into())]),
    };
    let body = serde_json::to_vec(&bundle)?;
    let digest = format!("{:x}", Sha256::digest(&body));
    let mut server = HttpServer::start(&material, CertificateKind::Valid, 200, body).await?;
    let config = material.config(&server.origin);
    let address = format!("{}/api/agent/v1/bundle", server.origin);
    let client = PanelClient::from_config(&config, "TEST_ONLY_session")?;
    assert_eq!(
        timeout(Duration::from_secs(6), client.bundle(&address, &digest)).await??,
        bundle
    );
    let received = server.next().await?;
    assert_eq!(received.target, "/api/agent/v1/bundle");
    assert!(received.headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && value.trim() == "Bearer TEST_ONLY_session"
        })
    }));

    // The old constructor and an independent public client cannot inherit the
    // newly trusted panel CA. Both perform actual certificate verification.
    let old = PanelClient::new(&server.origin, "TEST_ONLY_session")?;
    let rejected = timeout(Duration::from_secs(6), old.bundle(&address, &digest))
        .await?
        .unwrap_err();
    assert!(rejected.chain().any(|source| {
        source
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_connect)
    }));
    let independent = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()?;
    assert!(
        independent
            .get(&address)
            .send()
            .await
            .unwrap_err()
            .is_connect()
    );
    let rejected = timeout(
        Duration::from_secs(3),
        client.bundle("https://127.0.0.1:1/api/agent/v1/bundle", &digest),
    )
    .await?
    .unwrap_err();
    assert!(rejected.to_string().contains("configured panel origin"));
    assert!(
        !server.has_pending_request(),
        "origin validation must precede sending the session"
    );
    server.stop().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn private_panel_ca_preserves_https_name_expiry_and_unknown_ca_checks() -> Result<()> {
    use super::test_support::{CertificateKind, HttpServer, Material};
    let material = Material::new().await?;
    for (index, kind, configured_ca) in [
        (0, CertificateKind::Valid, false),
        (1, CertificateKind::WrongName, true),
        (2, CertificateKind::Expired, true),
    ] {
        let mut server =
            HttpServer::start(&material, kind, 200, br#"{"server_id":7}"#.to_vec()).await?;
        let mut config = material.config(&server.origin);
        config.identity_dir = material.root.join(format!("identity-rejected-{index}"));
        if !configured_ca {
            config.panel_ca_file = None;
        }
        let rejected = identity::enroll(&config, "TEST_ONLY_enrollment")
            .await
            .unwrap_err();
        assert!(rejected.chain().any(|source| {
            source
                .downcast_ref::<reqwest::Error>()
                .is_some_and(reqwest::Error::is_connect)
        }));
        assert!(!config.identity_dir.join("server_id").exists());
        let client = PanelClient::from_config(&config, "TEST_ONLY_session")?;
        let rejected = timeout(
            Duration::from_secs(6),
            client.bundle(
                &format!("{}/api/agent/v1/bundle", server.origin),
                &"0".repeat(64),
            ),
        )
        .await?
        .unwrap_err();
        assert!(rejected.chain().any(|source| {
            source
                .downcast_ref::<reqwest::Error>()
                .is_some_and(reqwest::Error::is_connect)
        }));
        assert!(
            !server.has_pending_request(),
            "TLS rejection must precede application traffic"
        );
        server.stop().await?;
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_panel_ca_files_fail_before_enrollment_identity_is_created() -> Result<()> {
    use super::test_support::Material;
    use std::os::unix::fs::symlink;
    let material = Material::new().await?;
    let certificate = std::fs::read(&material.ca)?;
    let cases = [
        Vec::new(),
        b"TEST_ONLY_missing_certificates".to_vec(),
        b"-----BEGIN CERTIFICATE-----\n%%%TEST_ONLY_invalid_base64\n-----END CERTIFICATE-----\n"
            .to_vec(),
        b"-----BEGIN CERTIFICATE-----\nVEVTVF9PTkxZX2JhZF9ERVI=\n-----END CERTIFICATE-----\n"
            .to_vec(),
        b"-----BEGIN CERTIFICATE-----\nVEVTVF9PTkxZ\n".to_vec(),
        b"-----BEGIN PRIVATE KEY-----\nTEST_ONLY_private_material\n-----END PRIVATE KEY-----\n"
            .to_vec(),
        [
            certificate.as_slice(),
            b"-----BEGIN PUBLIC KEY-----\nTEST_ONLY_not_a_certificate\n-----END PUBLIC KEY-----\n",
        ]
        .concat(),
        [
            certificate.as_slice(),
            b"-----BEGIN TEST_ONLY UNKNOWN-----\nVEVTVF9PTkxZ\n-----END TEST_ONLY UNKNOWN-----\n",
        ]
        .concat(),
        [
            certificate.as_slice(),
            b"-----BEGIN CERTIFICATE-----\nVEVTVF9PTkxZ\n-----END CERTIFICATE-----\n",
        ]
        .concat(),
        [
            certificate.as_slice(),
            b"TEST_ONLY_text_outside_certificate\n",
        ]
        .concat(),
        certificate.repeat(MAX_CERTIFICATES + 1),
        vec![b' '; MAX_BYTES as usize + 1],
    ];
    let mut paths = Vec::new();
    for (index, bytes) in cases.iter().enumerate() {
        let path = material.root.join(format!("rejected-{index}.pem"));
        std::fs::write(&path, bytes)?;
        paths.push(path);
    }
    let directory = material.root.join("not-a-file.pem");
    std::fs::create_dir(&directory)?;
    paths.push(directory);
    paths.push(Path::new("relative-TEST_ONLY.pem").into());
    paths.push(material.root.join("absent.pem"));
    let leaf_link = material.root.join("linked.pem");
    symlink(&material.ca, &leaf_link)?;
    paths.push(leaf_link);
    let parent_link = material.root.join("linked-parent");
    symlink(&material.root, &parent_link)?;
    paths.push(parent_link.join("ca.pem"));
    let nested = material.root.join("nested");
    std::fs::create_dir(&nested)?;
    paths.push(nested.join("../ca.pem"));
    for (index, path) in paths.into_iter().enumerate() {
        let mut config = material.config("https://127.0.0.1:1");
        config.identity_dir = material.root.join(format!("untouched-identity-{index}"));
        config.panel_ca_file = Some(path);
        let error = config.validate().unwrap_err();
        assert!(!format!("{error:#}").contains("TEST_ONLY_private_material"));
        assert!(
            identity::enroll(&config, "TEST_ONLY_enrollment")
                .await
                .is_err()
        );
        assert!(
            !config.identity_dir.exists(),
            "invalid CA cannot write panel origin or device key"
        );
        assert!(PanelClient::from_config(&config, "TEST_ONLY_session").is_err());
        assert!(websocket_connector(config.panel_ca_file.as_deref()).is_err());
    }
    let permitted = material.root.join("32-certificates.pem");
    std::fs::write(
        &permitted,
        [
            b"# TEST_ONLY verified CA bundle\n\n".as_slice(),
            certificate.repeat(MAX_CERTIFICATES).as_slice(),
        ]
        .concat(),
    )?;
    assert_eq!(certificates(Some(&permitted))?.len(), MAX_CERTIFICATES);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn replacement_with_a_fifo_after_preflight_cannot_block_ca_opening() -> Result<()> {
    use super::test_support::Material;
    use std::os::unix::fs::{OpenOptionsExt, symlink};
    let material = Material::new().await?;
    let path = material.root.join("replaced-after-check.pem");
    std::fs::copy(&material.ca, &path)?;
    let expected = ordinary_path(&path)?;
    std::fs::remove_file(&path)?;
    let mut child = tokio::process::Command::new("mkfifo")
        .arg(&path)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let status = match timeout(Duration::from_secs(3), child.wait()).await {
        Ok(status) => status?,
        Err(error) => {
            let _ = timeout(Duration::from_secs(2), child.kill()).await;
            let _ = timeout(Duration::from_secs(2), child.wait()).await;
            return Err(error.into());
        }
    };
    ensure!(
        status.success(),
        "TEST_ONLY FIFO fixture could not be created"
    );
    let opened_path = path.clone();
    let mut opened = tokio::task::spawn_blocking(move || open_checked(&opened_path, &expected));
    let completed = timeout(Duration::from_secs(1), &mut opened).await;
    if completed.is_err() {
        // Release a regressed blocking FIFO opener before failing; this does
        // not retry the production operation or convert the deadline to a pass.
        let _release = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)?;
        let _ = timeout(Duration::from_secs(2), &mut opened).await;
        anyhow::bail!("panel CA opening blocked after a FIFO replacement");
    }
    assert!(completed??.is_err());
    std::fs::remove_file(&path)?;
    std::fs::copy(&material.ca, &path)?;
    let expected = ordinary_path(&path)?;
    std::fs::remove_file(&path)?;
    symlink(&material.ca, &path)?;
    assert!(
        open_checked(&path, &expected).is_err(),
        "the checked inode cannot be replaced by a link"
    );
    Ok(())
}

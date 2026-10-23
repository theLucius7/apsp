use super::*;

const AUXILIARY_NAMES: [&str; 5] = [
    "build-info.json",
    "LICENSE",
    "source.tar.gz",
    "Cargo.lock",
    "THIRD_PARTY_NOTICES.txt",
];

struct ProvenanceAdapter {
    declared: bool,
    tamper: bool,
}

impl DiagnosticAdapter for ProvenanceAdapter {
    fn auxiliary_files(&self) -> Vec<String> {
        if self.declared {
            AUXILIARY_NAMES.into_iter().map(str::to_owned).collect()
        } else {
            Vec::new()
        }
    }
    fn describe(&self) -> DiagnosticDescriptor {
        TestAdapter.describe()
    }
    fn prepare<'a>(
        &'a self,
        spec: &'a DiagnosticSpec,
        privileged: &'a dyn Privileged,
    ) -> BoxFuture<'a, ServiceJob> {
        Box::pin(async move {
            let service = TestAdapter.prepare(spec, privileged).await?;
            if self.tamper {
                let auxiliary = spec.binary_path.parent().unwrap().join("source.tar.gz");
                let mut content = std::fs::read(&auxiliary)?;
                content[0] ^= 1;
                std::fs::write(auxiliary, content)?;
            }
            Ok(service)
        })
    }
    fn collect<'a>(&'a self, spec: &'a DiagnosticSpec) -> BoxFuture<'a, Option<DiagnosticOutput>> {
        TestAdapter.collect(spec)
    }
}

fn cached_provenance(directory: &Directory, id: Uuid) -> Result<DiagnosticJob> {
    let mut job = deadline::cached_job(directory, id)?;
    let artifact = directory
        .config()
        .install_root
        .join("diagnostic-fixture/v1");
    let binary = std::fs::read(artifact.join("runner"))?;
    let mut entry = release_support::entry(
        "diagnostic-fixture",
        "v1",
        "runner",
        "tar.gz",
        b"cached archive",
        &binary,
    );
    for name in AUXILIARY_NAMES {
        let content = format!("TEST_ONLY diagnostic provenance {name}");
        std::fs::write(artifact.join(name), content.as_bytes())?;
        entry.auxiliary_files.insert(
            name.into(),
            apsp_protocol::release::ReleaseFile {
                sha256: release_support::hash(content.as_bytes()),
                size: content.len() as u64,
            },
        );
    }
    let proof = release_support::signed_release(vec![(entry, b"cached archive".to_vec())]);
    release_support::install_proof(&artifact, &proof);
    job.artifact.proof = Some(proof);
    Ok(job)
}

#[tokio::test]
async fn diagnostic_provenance_files_are_required_and_rechecked_before_service_start() -> Result<()>
{
    for (declared, tamper, missing, expected_starts) in [
        (true, false, false, 1),
        (false, false, false, 0),
        (true, false, true, 0),
        (true, true, false, 0),
    ] {
        let directory = Directory::new();
        let config = directory.config();
        let services = Arc::new(Services::new(JobStatus::Running));
        let job = cached_provenance(&directory, Uuid::new_v4())?;
        if missing {
            std::fs::remove_file(config.install_root.join("diagnostic-fixture/v1/LICENSE"))?;
        }
        let worker = DiagnosticWorker::new(
            config.clone(),
            Arc::new(Mutex::new(State::open(&config.state_db)?)),
            vec![Arc::new(ProvenanceAdapter { declared, tamper })],
            Arc::new(FakeResourceOps::new(Arc::new(SystemOps))),
            services.clone(),
        )?
        .with_trusted_keys(release_support::trusted_keys());
        worker.save(&Checkpoint::Preparing(job))?;
        let client = PanelClient::new("http://127.0.0.1:1", "test-session")?
            .with_trusted_keys(release_support::trusted_keys());
        worker.tick(Some(&client)).await?;
        assert_eq!(
            services.starts.load(Ordering::Relaxed),
            expected_starts,
            "declared={declared}, tamper={tamper}, missing={missing}"
        );
        if expected_starts == 0 {
            assert!(worker.active()?.is_none());
            let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
            assert_eq!(pending[0].status, DiagnosticStatus::Failed);
        } else {
            assert!(matches!(worker.active()?, Some(Checkpoint::Started { .. })));
        }
    }
    Ok(())
}

const VERSIONED_AUXILIARY_NAMES: [&str; 2] = ["rootfs.tar.gz", "rootfs-manifest.json"];

struct VersionedProvenanceAdapter {
    requested_versions: Mutex<Vec<String>>,
}

impl DiagnosticAdapter for VersionedProvenanceAdapter {
    fn auxiliary_files(&self) -> Vec<String> {
        AUXILIARY_NAMES.into_iter().map(str::to_owned).collect()
    }
    fn auxiliary_files_for_version(&self, version: &str) -> Vec<String> {
        self.requested_versions.lock().unwrap().push(version.into());
        if version == "v1" {
            VERSIONED_AUXILIARY_NAMES
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else {
            Vec::new()
        }
    }
    fn describe(&self) -> DiagnosticDescriptor {
        TestAdapter.describe()
    }
    fn prepare<'a>(
        &'a self,
        spec: &'a DiagnosticSpec,
        privileged: &'a dyn Privileged,
    ) -> BoxFuture<'a, ServiceJob> {
        TestAdapter.prepare(spec, privileged)
    }
    fn collect<'a>(&'a self, spec: &'a DiagnosticSpec) -> BoxFuture<'a, Option<DiagnosticOutput>> {
        TestAdapter.collect(spec)
    }
}

fn cached_versioned_provenance(
    directory: &Directory,
    version: &str,
    include_auxiliary: bool,
) -> Result<DiagnosticJob> {
    let mut job = job(Uuid::new_v4());
    job.version = version.into();
    job.artifact.url = format!(
        "http://127.0.0.1:1/api/agent/v1/artifacts/diagnostic-fixture/{version}/{}",
        apsp_protocol::release::native_arch()?
    );
    let artifact = directory
        .config()
        .install_root
        .join("diagnostic-fixture")
        .join(version);
    std::fs::create_dir_all(&artifact)?;
    let binary = b"TEST_ONLY version-aware diagnostic binary";
    let archive = b"TEST_ONLY cached archive";
    std::fs::write(artifact.join("runner"), binary)?;
    let mut entry = release_support::entry(
        "diagnostic-fixture",
        version,
        "runner",
        "tar.gz",
        archive,
        binary,
    );
    if include_auxiliary {
        for name in VERSIONED_AUXILIARY_NAMES {
            let content = format!("TEST_ONLY signed auxiliary fixture {name}");
            std::fs::write(artifact.join(name), content.as_bytes())?;
            entry.auxiliary_files.insert(
                name.into(),
                apsp_protocol::release::ReleaseFile {
                    sha256: release_support::hash(content.as_bytes()),
                    size: content.len() as u64,
                },
            );
        }
    }
    let proof = release_support::signed_release(vec![(entry, archive.to_vec())]);
    release_support::install_proof(&artifact, &proof);
    job.artifact.sha256 = release_support::hash(archive);
    job.artifact.proof = Some(proof);
    Ok(job)
}

#[tokio::test]
async fn requested_version_selects_the_exact_signed_inventory_before_start() -> Result<()> {
    for (version, include_auxiliary, expected_starts) in [
        ("v1", true, 1),
        ("v1", false, 0),
        ("v2", true, 0),
        ("v2", false, 1),
    ] {
        let directory = Directory::new();
        let config = directory.config();
        let services = Arc::new(Services::new(JobStatus::Running));
        let job = cached_versioned_provenance(&directory, version, include_auxiliary)?;
        let adapter = Arc::new(VersionedProvenanceAdapter {
            requested_versions: Mutex::new(Vec::new()),
        });
        let worker = DiagnosticWorker::new(
            config.clone(),
            Arc::new(Mutex::new(State::open(&config.state_db)?)),
            vec![adapter.clone()],
            Arc::new(FakeResourceOps::new(Arc::new(SystemOps))),
            services.clone(),
        )?
        .with_trusted_keys(release_support::trusted_keys());
        worker.save(&Checkpoint::Preparing(job))?;
        let client = PanelClient::new("http://127.0.0.1:1", "TEST_ONLY session")?
            .with_trusted_keys(release_support::trusted_keys());
        worker.tick(Some(&client)).await?;
        assert_eq!(
            *adapter.requested_versions.lock().unwrap(),
            vec![version.to_owned()]
        );
        assert_eq!(
            services.starts.load(Ordering::Relaxed),
            expected_starts,
            "version={version}, include_auxiliary={include_auxiliary}"
        );
        if expected_starts == 0 {
            assert!(worker.active()?.is_none());
            let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].status, DiagnosticStatus::Failed);
            assert!(
                pending[0]
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("differs from adapter")
            );
        } else {
            assert!(
                matches!(worker.active()?, Some(Checkpoint::Started { spec, .. }) if spec.version == version)
            );
        }
    }
    Ok(())
}

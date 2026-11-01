-- New deployments use the self-maintained runtime build (ADR 0104). Facts already
-- recorded for upstream 1.14.2 stay valid history.
ALTER TABLE singbox_chain_runtime_requirements
    DROP CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check,
    ADD CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check
        CHECK (runtime_version IN ('1.14.2', '1.14.2-apsp.1'));
ALTER TABLE singbox_runtime_manifest_facts
    DROP CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check,
    ADD CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check
        CHECK (runtime_version IN ('1.14.2', '1.14.2-apsp.1'));
ALTER TABLE singbox_runtime_rollouts
    DROP CONSTRAINT singbox_runtime_rollouts_runtime_version_check,
    ADD CONSTRAINT singbox_runtime_rollouts_runtime_version_check
        CHECK (runtime_version IN ('1.14.2', '1.14.2-apsp.1'));

-- activated_at keeps the first cutover. Pacing needs the latest one.
ALTER TABLE singbox_nexus_runtime_hosts ADD COLUMN last_activated_at BIGINT;

-- New deployments use the legacy service's kernel, rebuilt and signed by Apsp
-- (ADR 0119). Facts recorded for the earlier runtimes stay valid history.
ALTER TABLE singbox_chain_runtime_requirements
    DROP CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check,
    ADD CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check
        CHECK (runtime_version IN ('1.14.2', '1.14.2-apsp.1', '1.14.0-apsp.1'));
ALTER TABLE singbox_runtime_manifest_facts
    DROP CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check,
    ADD CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check
        CHECK (runtime_version IN ('1.14.2', '1.14.2-apsp.1', '1.14.0-apsp.1'));
ALTER TABLE singbox_runtime_rollouts
    DROP CONSTRAINT singbox_runtime_rollouts_runtime_version_check,
    ADD CONSTRAINT singbox_runtime_rollouts_runtime_version_check
        CHECK (runtime_version IN ('1.14.2', '1.14.2-apsp.1', '1.14.0-apsp.1'));

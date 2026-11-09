-- One-time baseline adoption for ADR 0131: the legacy two-hop and mixed chain models and the
-- numbered (A) source model leave the baseline. From the S-UI baseline (checksum 405612a7…,
-- fingerprint 7701ab9e…) to the new one (838b323f…, 00168f4f…).
-- Fingerprints are PostgreSQL 16 values with sqlx's _sqlx_migrations present, checked on a copy of
-- the production structure. Production held no chain of any kind and its numbered sources were
-- migrated to the ordered (B) model on 2026-11-04; the archive rows were exported before this ran.
-- singbox_chains and singbox_ordered_chain_versions are empty and are recreated, because dropping
-- columns would leave column numbers that a fresh baseline never has.
-- psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-one-model-20261109.sql
SET LOCAL lock_timeout = '10s';
SET LOCAL search_path TO public;
SET LOCAL check_function_bodies = false;
SELECT pg_advisory_xact_lock(73401220);
DO $$
DECLARE actual text;
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1 OR NOT EXISTS (
        SELECT 1 FROM public._sqlx_migrations WHERE version=1 AND success AND checksum=decode('405612a777a144b2f00f90bd2446dca71e3b754c5b1e5625249469ec274fe1af75b2171a15b720882beca2630b69f1a7','hex')
    ) THEN RAISE EXCEPTION 'expected the S-UI baseline 405612a7; refusing to modify database'; END IF;
    SELECT (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
FROM (
    SELECT 'relation:' || c.relname || ':' || c.relkind::text || ':' || a.attname || ':' || a.attnum || ':' ||
           format_type(a.atttypid,a.atttypmod) || ':' || a.attnotnull || ':' ||
           COALESCE(pg_get_expr(d.adbin,d.adrelid),'') AS definition
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
    LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
    WHERE n.nspname='public' AND c.relname <> '_sqlx_migrations'
    UNION ALL
    SELECT 'constraint:' || c.relname || ':' || k.conname || ':' || pg_get_constraintdef(k.oid)
    FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND c.relname <> '_sqlx_migrations'
    UNION ALL
    SELECT 'index:' || indexname || ':' || indexdef FROM pg_indexes
    WHERE schemaname='public' AND tablename <> '_sqlx_migrations'
    UNION ALL
    SELECT 'view:' || viewname || ':' || definition FROM pg_views WHERE schemaname='public'
    UNION ALL
    SELECT 'function:' || p.proname || ':' || pg_get_functiondef(p.oid)
    FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND p.prokind='f'
    UNION ALL
    SELECT 'trigger:' || c.relname || ':' || pg_get_triggerdef(t.oid)
    FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND NOT t.tgisinternal
) AS definitions) INTO actual;
    IF actual <> '7701ab9e14096c9fb137d56ccdac5e32' OR actual IS DISTINCT FROM (SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton) THEN
        RAISE EXCEPTION 'existing schema differs from its recorded baseline';
    END IF;
    IF EXISTS (SELECT 1 FROM public.singbox_chains) OR EXISTS (SELECT 1 FROM public.singbox_ordered_chain_versions) THEN
        RAISE EXCEPTION 'chains exist; this adoption only recreates empty chain tables';
    END IF;
END $$;

-- The ordered (B) sources keep their public-id sequences under their own names.
ALTER SEQUENCE public.singbox_external_nodes_id_seq OWNED BY public.singbox_ordered_external_nodes.public_id;
ALTER SEQUENCE public.singbox_external_nodes_id_seq RENAME TO singbox_ordered_external_nodes_public_id_seq;
ALTER SEQUENCE public.singbox_external_node_versions_id_seq OWNED BY public.singbox_ordered_external_node_versions.public_id;
ALTER SEQUENCE public.singbox_external_node_versions_id_seq RENAME TO singbox_ordered_external_node_versions_public_id_seq;

-- Shared checks and views read the ordered sources only.

CREATE OR REPLACE FUNCTION public.singbox_check_external_metadata() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.kind <> 'external'
        OR EXISTS(SELECT 1 FROM singbox_ordered_external_nodes WHERE public_id=NEW.id) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'external node metadata refers to an unknown node' USING ERRCODE='23503';
END $$;

CREATE OR REPLACE FUNCTION public.singbox_check_policy_external() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS(
        SELECT 1 FROM singbox_ordered_external_nodes n
        JOIN singbox_ordered_external_node_versions v ON v.node_id=n.id
        WHERE n.public_id=NEW.external_node_id AND v.public_id=NEW.node_version_id
    ) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'policy group external node refers to an unknown node version' USING ERRCODE='23503';
END $$;

CREATE OR REPLACE VIEW public.singbox_external_node_states AS
 SELECT n.public_id AS id,
    n.source_id,
    n.identity_epoch,
    v.public_id AS current_version_id,
    COALESCE((m.public_preview ->> 'name'::text), (v.public_preview ->> 'name'::text), ''::text) AS name,
    n.adopted,
    COALESCE((n.last_seen_revision = ( SELECT r.id
           FROM public.singbox_subscription_source_revisions r
          WHERE ((r.source_id = n.source_id) AND (r.identity_epoch = n.identity_epoch))
          ORDER BY r.generation DESC
         LIMIT 1)), false) AS present,
    (n.identity_state = 'unique'::text) AS identity_unique,
    s.name AS source_name,
    s.identity_epoch AS source_epoch,
    s.archived AS source_archived,
    s.deleted_at AS source_deleted_at,
    (s.last_error ->> 'kind'::text) AS source_last_error,
    s.settings_revision AS source_settings_revision
   FROM (((public.singbox_ordered_external_nodes n
     JOIN public.singbox_ordered_subscription_sources s ON ((s.id = n.source_id)))
     LEFT JOIN public.singbox_ordered_external_node_versions v ON ((v.id = n.latest_version)))
     LEFT JOIN public.singbox_subscription_revision_nodes m ON (((m.node_id = n.id) AND (m.source_revision_id = n.last_seen_revision))));

CREATE OR REPLACE VIEW public.singbox_external_version_configs AS
 SELECT v.public_id AS id,
    n.public_id AS external_node_id,
    n.source_id,
    n.identity_epoch,
    r.parser_version,
    true AS ordered,
    v.legacy_config AS config_json,
    v.legacy_config_sha256 AS config_sha256,
    v.normalized_config,
    v.content_digest
   FROM ((public.singbox_ordered_external_node_versions v
     JOIN public.singbox_ordered_external_nodes n ON ((n.id = v.node_id)))
     JOIN public.singbox_subscription_source_revisions r ON ((r.id = v.source_revision_id)));

CREATE OR REPLACE FUNCTION public.singbox_eligible_accesses(at_s bigint) RETURNS TABLE(user_id bigint, node_id bigint, uuid uuid, stat_name text, credential text)
    LANGUAGE sql STABLE
    AS $$
    SELECT a.user_id,a.node_id,a.uuid,a.stat_name,a.credential FROM accesses a
    JOIN singbox_desired_accesses d USING(user_id,node_id)
    JOIN singbox_entitlements(at_s) e ON e.user_id=a.user_id AND e.allowed
    JOIN nodes n ON n.id=a.node_id AND n.enabled
    WHERE NOT EXISTS(SELECT 1 FROM singbox_chains c WHERE c.entry_node_id=n.id AND c.deleted_at IS NULL
        AND EXISTS(SELECT 1 FROM singbox_ordered_chain_hops h JOIN nodes x ON x.id=h.managed_node_id
            WHERE h.chain_id=c.id AND h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) AND NOT x.enabled));
$$;


-- The views over chains go first; the chain tables are recreated below.

DROP VIEW public.singbox_desired_accesses;

DROP VIEW public.singbox_live_chains;

ALTER TABLE public.singbox_chain_runtime_requirements DROP CONSTRAINT singbox_chain_runtime_requirements_chain_id_generation_fkey;

ALTER TABLE public.singbox_chain_state DROP CONSTRAINT singbox_chain_state_chain_id_fkey;

ALTER TABLE public.singbox_node_metadata DROP CONSTRAINT singbox_node_metadata_chain_id_fkey;

ALTER TABLE public.singbox_ordered_chain_hops DROP CONSTRAINT singbox_ordered_chain_hops_chain_id_generation_fkey;

ALTER TABLE public.singbox_path_deployment_dependencies DROP CONSTRAINT singbox_path_deployment_dependencies_chain_id_generation_fkey;

ALTER TABLE public.singbox_path_probes DROP CONSTRAINT singbox_path_probes_chain_id_generation_fkey;

ALTER TABLE public.singbox_path_stage_deployments DROP CONSTRAINT singbox_path_stage_deployments_chain_id_generation_fkey;

ALTER TABLE public.singbox_policy_chains DROP CONSTRAINT singbox_policy_chains_chain_id_fkey;

DROP TABLE public.singbox_chain_creation_requests, public.singbox_chain_hops, public.singbox_chain_versions, public.singbox_legacy_takeovers, public.singbox_mixed_conversions, public.singbox_path_controls, public.singbox_path_deployments, public.singbox_retired_path_scopes, public.singbox_external_node_versions, public.singbox_external_nodes, public.singbox_subscription_sources, public.singbox_source_id_map, public.singbox_source_jobs, public.singbox_source_migration, public.singbox_source_previews, public.singbox_source_revision_map, public.singbox_source_revisions, public.singbox_chains, public.singbox_ordered_chain_versions;

DROP FUNCTION public.singbox_immutable_source_version();

DROP FUNCTION public.singbox_path_ready(bigint);

DROP FUNCTION public.singbox_path_resources_available(bigint);

DROP FUNCTION public.singbox_preserve_path_hop();

DROP FUNCTION public.singbox_preserve_path_version();

DROP FUNCTION public.singbox_reject_numbered_source_write();

DROP FUNCTION public.singbox_sources_migrated();


-- The chain tables as the new baseline defines them.

CREATE SEQUENCE public.singbox_chains_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE TABLE public.singbox_chains (
    id bigint NOT NULL,
    name text NOT NULL,
    entry_node_id bigint NOT NULL,
    path_kind text DEFAULT 'ordered'::text NOT NULL,
    minimum_generation bigint DEFAULT 0 NOT NULL,
    deleted_at bigint,
    settings_revision bigint DEFAULT 1 NOT NULL,
    desired_generation bigint DEFAULT 1 NOT NULL,
    applied_generation bigint,
    candidate_generation bigint,
    recovery_generation bigint,
    phase text DEFAULT 'preparing_dependencies'::text NOT NULL,
    last_error text,
    route_enabled boolean DEFAULT true NOT NULL,
    last_granted boolean DEFAULT false NOT NULL,
    restore_step text,
    CONSTRAINT singbox_chains_desired_generation_check CHECK ((desired_generation > 0)),
    CONSTRAINT singbox_chains_path_kind_check CHECK ((path_kind = 'ordered'::text)),
    CONSTRAINT singbox_chains_phase_check CHECK ((phase = ANY (ARRAY['preparing_dependencies'::text, 'preparing_entry'::text, 'probing_candidate'::text, 'switching_entry'::text, 'probing_switched'::text, 'fixing_barrier'::text, 'retiring_old'::text, 'applied'::text, 'restoring'::text, 'failed'::text, 'retiring'::text, 'retired'::text]))),
    CONSTRAINT singbox_chains_restore_step_check CHECK ((restore_step = ANY (ARRAY['entry'::text, 'probe'::text, 'barrier'::text, 'cleanup'::text]))),
    CONSTRAINT singbox_chains_settings_revision_check CHECK ((settings_revision > 0)),
    CONSTRAINT singbox_ordered_minimum_generation_check CHECK ((minimum_generation >= 0))
);

CREATE TABLE public.singbox_ordered_chain_versions (
    chain_id bigint NOT NULL,
    generation bigint NOT NULL,
    entry_endpoint_version uuid NOT NULL,
    semantic_sha256 text NOT NULL,
    capabilities jsonb NOT NULL,
    snapshot jsonb NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT singbox_ordered_chain_versions_capabilities_check CHECK ((jsonb_typeof(capabilities) = 'object'::text)),
    CONSTRAINT singbox_ordered_chain_versions_generation_check CHECK ((generation > 0)),
    CONSTRAINT singbox_ordered_chain_versions_semantic_sha256_check CHECK ((semantic_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT singbox_ordered_chain_versions_snapshot_check CHECK ((jsonb_typeof(snapshot) = 'object'::text))
);

ALTER SEQUENCE public.singbox_chains_id_seq OWNED BY public.singbox_chains.id;

ALTER TABLE ONLY public.singbox_chains ALTER COLUMN id SET DEFAULT nextval('public.singbox_chains_id_seq'::regclass);

ALTER TABLE ONLY public.singbox_chains
    ADD CONSTRAINT singbox_chains_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.singbox_ordered_chain_versions
    ADD CONSTRAINT singbox_ordered_chain_versions_pkey PRIMARY KEY (chain_id, generation);

CREATE UNIQUE INDEX singbox_chain_one_live_entry ON public.singbox_chains USING btree (entry_node_id) WHERE ((deleted_at IS NULL) OR ((path_kind = 'ordered'::text) AND (phase <> 'retired'::text)));

CREATE UNIQUE INDEX singbox_live_chain_entry ON public.singbox_chains USING btree (entry_node_id) WHERE (deleted_at IS NULL);

CREATE TRIGGER singbox_path_snapshot_immutable BEFORE DELETE OR UPDATE ON public.singbox_ordered_chain_versions FOR EACH ROW EXECUTE FUNCTION public.singbox_reject_path_snapshot_change();

ALTER TABLE ONLY public.singbox_chains
    ADD CONSTRAINT singbox_chains_entry_node_id_fkey FOREIGN KEY (entry_node_id) REFERENCES public.nodes(id);

ALTER TABLE ONLY public.singbox_ordered_chain_versions
    ADD CONSTRAINT singbox_ordered_chain_versions_chain_id_fkey FOREIGN KEY (chain_id) REFERENCES public.singbox_chains(id);

ALTER TABLE ONLY public.singbox_ordered_chain_versions
    ADD CONSTRAINT singbox_ordered_chain_versions_entry_endpoint_version_fkey FOREIGN KEY (entry_endpoint_version) REFERENCES public.singbox_managed_endpoint_versions(id);

ALTER TABLE ONLY public.singbox_chain_runtime_requirements
    ADD CONSTRAINT singbox_chain_runtime_requirements_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_chain_state
    ADD CONSTRAINT singbox_chain_state_chain_id_fkey FOREIGN KEY (chain_id) REFERENCES public.singbox_chains(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.singbox_node_metadata
    ADD CONSTRAINT singbox_node_metadata_chain_id_fkey FOREIGN KEY (chain_id) REFERENCES public.singbox_chains(id);

ALTER TABLE ONLY public.singbox_ordered_chain_hops
    ADD CONSTRAINT singbox_ordered_chain_hops_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_path_deployment_dependencies
    ADD CONSTRAINT singbox_path_deployment_dependencies_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_path_probes
    ADD CONSTRAINT singbox_path_probes_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_path_stage_deployments
    ADD CONSTRAINT singbox_path_stage_deployments_chain_id_generation_fkey FOREIGN KEY (chain_id, generation) REFERENCES public.singbox_ordered_chain_versions(chain_id, generation);

ALTER TABLE ONLY public.singbox_policy_chains
    ADD CONSTRAINT singbox_policy_chains_chain_id_fkey FOREIGN KEY (chain_id) REFERENCES public.singbox_chains(id);

CREATE VIEW public.singbox_desired_accesses AS
 SELECT DISTINCT granted.user_id,
    granted.node_id
   FROM ((((( SELECT u_1.user_id,
            n_1.node_id
           FROM (public.singbox_user_policies u_1
             JOIN public.singbox_policy_nodes n_1 ON ((n_1.group_id = u_1.group_id)))
          WHERE ((NOT (EXISTS ( SELECT 1
                   FROM public.singbox_chains c_1
                  WHERE ((c_1.path_kind = 'ordered'::text) AND (c_1.entry_node_id = n_1.node_id) AND ((c_1.deleted_at IS NULL) OR (c_1.phase <> 'retired'::text)))))) AND (NOT (EXISTS ( SELECT 1
                   FROM public.singbox_nexus_runtime_nodes r
                  WHERE (r.node_id = n_1.node_id)))))
        UNION
         SELECT u_1.user_id,
            c_1.entry_node_id
           FROM ((public.singbox_user_policies u_1
             JOIN public.singbox_policy_chains p ON ((p.group_id = u_1.group_id)))
             JOIN public.singbox_chains c_1 ON ((c_1.id = p.chain_id)))
          WHERE (c_1.deleted_at IS NULL)) granted
     JOIN public.users u ON (((u.id = granted.user_id) AND (u.deleted_at IS NULL))))
     JOIN public.nodes n ON (((n.id = granted.node_id) AND (n.deleted_at IS NULL))))
     JOIN public.servers s ON (((s.id = n.server_id) AND (s.deleted_at IS NULL))))
     LEFT JOIN public.singbox_chains c ON (((c.entry_node_id = n.id) AND ((c.deleted_at IS NULL) OR ((c.path_kind = 'ordered'::text) AND (c.phase <> 'retired'::text))))))
  WHERE ((c.id IS NULL) OR ((c.deleted_at IS NULL) AND (n.protocol = 'vless-reality'::text) AND (EXISTS ( SELECT 1
           FROM public.singbox_ordered_chain_versions v
          WHERE ((v.chain_id = c.id) AND (v.generation = c.desired_generation)))) AND (NOT (EXISTS ( SELECT 1
           FROM ((public.singbox_ordered_chain_hops h
             LEFT JOIN public.nodes e ON ((e.id = h.managed_node_id)))
             LEFT JOIN public.servers es ON ((es.id = h.managed_server_id)))
          WHERE ((h.chain_id = c.id) AND (h.generation = c.desired_generation) AND (h.kind = 'managed'::text) AND ((e.id IS NULL) OR (es.id IS NULL) OR (e.deleted_at IS NOT NULL) OR (es.deleted_at IS NOT NULL) OR (NOT e.enabled))))))));


DO $$
DECLARE actual text;
BEGIN
    SELECT (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
FROM (
    SELECT 'relation:' || c.relname || ':' || c.relkind::text || ':' || a.attname || ':' || a.attnum || ':' ||
           format_type(a.atttypid,a.atttypmod) || ':' || a.attnotnull || ':' ||
           COALESCE(pg_get_expr(d.adbin,d.adrelid),'') AS definition
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
    LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
    WHERE n.nspname='public' AND c.relname <> '_sqlx_migrations'
    UNION ALL
    SELECT 'constraint:' || c.relname || ':' || k.conname || ':' || pg_get_constraintdef(k.oid)
    FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND c.relname <> '_sqlx_migrations'
    UNION ALL
    SELECT 'index:' || indexname || ':' || indexdef FROM pg_indexes
    WHERE schemaname='public' AND tablename <> '_sqlx_migrations'
    UNION ALL
    SELECT 'view:' || viewname || ':' || definition FROM pg_views WHERE schemaname='public'
    UNION ALL
    SELECT 'function:' || p.proname || ':' || pg_get_functiondef(p.oid)
    FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND p.prokind='f'
    UNION ALL
    SELECT 'trigger:' || c.relname || ':' || pg_get_triggerdef(t.oid)
    FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND NOT t.tgisinternal
) AS definitions) INTO actual;
    IF actual <> '00168f4f0200aba049c809451856d5e5' THEN RAISE EXCEPTION 'result differs from fresh target baseline: %',actual; END IF;
    UPDATE public.apsp_schema_baseline SET fingerprint=actual WHERE singleton;
    UPDATE public._sqlx_migrations SET checksum=decode('838b323fe0b55de2229fb571e5b6ae983e946973aa655e53a45a7f2f2e494b1c2c7efbef80328d0e1c3ea9bcbd6c1a58','hex') WHERE version=1;
END $$;

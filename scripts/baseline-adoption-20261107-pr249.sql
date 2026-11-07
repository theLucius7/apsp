-- 生产库从 #245 的基线接入 #249 的基线（2026-11-07）。先备份，再在一个事务中执行（psql -1）。
-- 只接受迁移记录与结构指纹恰好是 #245 基线的数据库；任何检查不通过即整体回滚。
SET LOCAL lock_timeout = '10s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);

DO $$
BEGIN
    IF (SELECT count(*) FROM public._sqlx_migrations) <> 1
       OR NOT EXISTS (SELECT 1 FROM public._sqlx_migrations WHERE version = 1 AND success
                      AND checksum = decode('9f83b4d8e034cde34b2b147fed4556e51b5b2d287dd8ccfc1882a25a9249f23f3d6dac28329cbe3b2e1683f2dd210ee6', 'hex'))
       OR (SELECT fingerprint FROM public.apsp_schema_baseline) <> '073b3f751082bd3bb9308407129a228f' THEN
        RAISE EXCEPTION 'not the #245 baseline';
    END IF;
END
$$;

-- Changes in #249, copied from crates/panel/migrations/0001_current.sql.
ALTER TABLE public.singbox_chain_runtime_requirements DROP CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check;
ALTER TABLE public.singbox_chain_runtime_requirements ADD CONSTRAINT singbox_chain_runtime_requirements_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text])));
ALTER TABLE public.singbox_runtime_manifest_facts DROP CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check;
ALTER TABLE public.singbox_runtime_manifest_facts ADD CONSTRAINT singbox_runtime_manifest_facts_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text])));
ALTER TABLE public.singbox_runtime_rollouts DROP CONSTRAINT singbox_runtime_rollouts_runtime_version_check;
ALTER TABLE public.singbox_runtime_rollouts ADD CONSTRAINT singbox_runtime_rollouts_runtime_version_check CHECK ((runtime_version = ANY (ARRAY['1.14.2'::text, '1.14.2-apsp.1'::text, '1.14.0-apsp.1'::text, '1.14.4-apsp.1'::text])));
CREATE TABLE public.singbox_user_limits (
    user_id bigint PRIMARY KEY REFERENCES public.users(id),
    upload_mbps integer NOT NULL DEFAULT 0 CHECK (upload_mbps BETWEEN 0 AND 1000000),
    download_mbps integer NOT NULL DEFAULT 0 CHECK (download_mbps BETWEEN 0 AND 1000000),
    max_connections integer NOT NULL DEFAULT 0 CHECK (max_connections BETWEEN 0 AND 1000000),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision BETWEEN 0 AND 9007199254740991)
);

UPDATE public._sqlx_migrations SET checksum = decode('5e0ed43b555091e715ba2641438748aec06b18fb7c8682542756649cd500e14ebfe2914950907677ee573c509292ad77', 'hex') WHERE version = 1;
UPDATE public.apsp_schema_baseline SET fingerprint = (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
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
) AS definitions);

DO $$
BEGIN
    IF (SELECT fingerprint FROM public.apsp_schema_baseline) <> '90578f83a578ed9d2a7df8c2b1b4dcb7' THEN
        RAISE EXCEPTION 'fingerprint % differs from a fresh #249 baseline', (SELECT fingerprint FROM public.apsp_schema_baseline);
    END IF;
END
$$;

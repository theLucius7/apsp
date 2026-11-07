-- 生产数据库一次性接入当前基线（2026-11-07，见 docs/database-baseline.md「生产安装」）。
-- 用法：先备份，再在一个事务中执行：psql -v ON_ERROR_STOP=1 -1 -f scripts/baseline-adoption-20261107.sql
-- 只接受恰好是已退役迁移 1–90 的数据库；任何检查不通过即整体回滚，不改动任何内容。

SET LOCAL lock_timeout = '10s';
SET LOCAL search_path TO public;
SELECT pg_advisory_xact_lock(73401220);

DO $$
DECLARE n bigint; digest text;
BEGIN
    SELECT count(*), md5(string_agg(version||':'||encode(checksum,'hex'), ',' ORDER BY version))
      INTO n, digest FROM public._sqlx_migrations WHERE success;
    IF n <> 90 OR digest <> 'e284164e6ce63186bb271019e42bb926' OR EXISTS (SELECT 1 FROM public._sqlx_migrations WHERE NOT success) THEN
        RAISE EXCEPTION 'not the retired 1..90 history (rows %, digest %)', n, digest;
    END IF;
    IF to_regclass('public.apsp_schema_baseline') IS NOT NULL OR to_regclass('public.alicloud_traffic_history') IS NOT NULL THEN
        RAISE EXCEPTION 'database already has baseline objects';
    END IF;
END
$$;

-- Schema added after migration 90, copied from crates/panel/migrations/0001_current.sql.
ALTER TABLE public.alicloud_accounts ADD COLUMN traffic_history_identity text;
ALTER TABLE public.alicloud_resources
    ADD COLUMN cdt_guard_next_at bigint DEFAULT 0 NOT NULL,
    ADD COLUMN ipv6_addresses jsonb DEFAULT '[]'::jsonb NOT NULL,
    ADD COLUMN ipv6_checked_at bigint,
    ADD COLUMN ipv6_error text;
CREATE TABLE public.alicloud_traffic_history (
    account_id uuid NOT NULL,
    bucket_seconds integer NOT NULL,
    bucket_start bigint NOT NULL,
    sampled_at bigint NOT NULL,
    mainland_bytes numeric(20,0) NOT NULL,
    overseas_bytes numeric(20,0) NOT NULL,
    CONSTRAINT alicloud_traffic_history_bucket_seconds_check CHECK ((bucket_seconds = ANY (ARRAY[3600, 86400]))),
    CONSTRAINT alicloud_traffic_history_mainland_bytes_check CHECK ((mainland_bytes >= (0)::numeric)),
    CONSTRAINT alicloud_traffic_history_overseas_bytes_check CHECK ((overseas_bytes >= (0)::numeric))
);
ALTER TABLE ONLY public.alicloud_traffic_history
    ADD CONSTRAINT alicloud_traffic_history_pkey PRIMARY KEY (account_id, bucket_seconds, bucket_start);
CREATE INDEX alicloud_traffic_history_retention ON public.alicloud_traffic_history USING btree (bucket_start);
ALTER TABLE ONLY public.alicloud_traffic_history
    ADD CONSTRAINT alicloud_traffic_history_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.alicloud_accounts(id);

-- The baseline record, exactly as the #245 binary writes it on a fresh database.
DELETE FROM public._sqlx_migrations;
INSERT INTO public._sqlx_migrations(version, description, installed_on, success, checksum, execution_time)
VALUES (1, 'current', now(), true, decode('9f83b4d8e034cde34b2b147fed4556e51b5b2d287dd8ccfc1882a25a9249f23f3d6dac28329cbe3b2e1683f2dd210ee6', 'hex'), 0);

CREATE TABLE public.apsp_schema_baseline (singleton boolean PRIMARY KEY CHECK (singleton), fingerprint text NOT NULL);
INSERT INTO public.apsp_schema_baseline SELECT true, (SELECT md5(string_agg(definition, E'\n' ORDER BY definition COLLATE "C"))
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

-- The structure must equal a freshly initialized baseline database.
DO $$
BEGIN
    IF (SELECT fingerprint FROM public.apsp_schema_baseline) <> '073b3f751082bd3bb9308407129a228f' THEN
        RAISE EXCEPTION 'fingerprint % differs from a fresh baseline', (SELECT fingerprint FROM public.apsp_schema_baseline);
    END IF;
END
$$;

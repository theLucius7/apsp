CREATE TABLE singbox_node_metadata (
    kind TEXT NOT NULL CHECK (kind IN ('direct','chain','external')),
    id BIGINT NOT NULL CHECK (id > 0),
    node_id BIGINT GENERATED ALWAYS AS (CASE WHEN kind='direct' THEN id END) STORED REFERENCES nodes(id),
    chain_id BIGINT GENERATED ALWAYS AS (CASE WHEN kind='chain' THEN id END) STORED REFERENCES singbox_chains(id),
    external_node_id BIGINT GENERATED ALWAYS AS (CASE WHEN kind='external' THEN id END) STORED REFERENCES singbox_external_nodes(id),
    name_override TEXT CHECK (name_override IS NULL OR (char_length(name_override) BETWEEN 1 AND 128)),
    tags JSONB NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(tags)='array' AND jsonb_array_length(tags)<=16),
    note TEXT NOT NULL DEFAULT '' CHECK (octet_length(note)<=1024),
    sort_order BIGINT NOT NULL DEFAULT 0,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision>0),
    deleted_at BIGINT,
    PRIMARY KEY(kind,id),
    CHECK (kind='external' OR name_override IS NULL)
);

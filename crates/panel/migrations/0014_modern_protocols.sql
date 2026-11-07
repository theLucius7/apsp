ALTER TABLE nodes DROP CONSTRAINT nodes_protocol_check;
ALTER TABLE nodes ADD CONSTRAINT nodes_protocol_check CHECK (
    protocol IN ('vless-reality', 'hysteria2', 'shadowsocks2022', 'tuic', 'anytls', 'naive', 'snell-v6')
);
ALTER TABLE nodes ADD COLUMN protocol_config JSONB NOT NULL DEFAULT '{"type":"vless-reality"}';
ALTER TABLE nodes ADD CONSTRAINT nodes_protocol_config_check CHECK (
    jsonb_typeof(protocol_config) = 'object'
    AND protocol_config ? 'type'
    AND COALESCE(protocol_config->>'type' = protocol, FALSE)
);
ALTER TABLE accesses ADD COLUMN credential TEXT NOT NULL DEFAULT '';

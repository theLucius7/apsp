-- VLESS behind a TLS-terminating tunnel (ADR 0109). Only adds a value; existing rows are unchanged.
ALTER TABLE nodes DROP CONSTRAINT nodes_protocol_check;
ALTER TABLE nodes ADD CONSTRAINT nodes_protocol_check CHECK (
    protocol IN ('vless-reality', 'vless-tunnel', 'hysteria2', 'shadowsocks2022', 'tuic', 'anytls', 'naive', 'snell-v6')
);

-- Plugin-owned labels do not change the core ProbeSpec wire format or existing tables.
CREATE TABLE tcpquality_target_regions (
    probe_id UUID PRIMARY KEY REFERENCES network_probes(id) ON DELETE CASCADE,
    region TEXT NOT NULL CHECK (region IN ('east_asia','southeast_asia','europe','americas','other'))
);

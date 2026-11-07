CREATE TABLE latency_tasks (
    id UUID PRIMARY KEY,
    spec JSONB NOT NULL,
    default_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    revision BIGINT NOT NULL DEFAULT 1
);
ALTER TABLE network_probes ADD COLUMN task_id UUID REFERENCES latency_tasks(id) ON DELETE CASCADE;
CREATE UNIQUE INDEX latency_task_server ON network_probes(task_id,server_id) WHERE task_id IS NOT NULL;

use super::*;
use apsp_protocol::{DIAGNOSTIC_SECTION_COUNT, DiagnosticSectionUpdate};

pub(super) const SECTIONS_OUTBOX: &str = "diagnostics:sections:outbox";
const MAX_CHAPTER_BYTES: usize = 512 * 1024;

impl DiagnosticWorker {
    pub(super) async fn capture_sections(&self, spec: &DiagnosticSpec, plugin: &str) -> Result<()> {
        let adapter = self
            .adapters
            .get(plugin)
            .context("diagnostic plugin is not registered")?;
        let id = Uuid::parse_str(&spec.id)?;
        let chapters = self.bounded(adapter.collect_sections(spec)).await?;
        ensure!(
            chapters.len() <= DIAGNOSTIC_SECTION_COUNT,
            "too many diagnostic chapters"
        );
        let updates: Vec<_> = chapters
            .into_iter()
            .map(|chapter| DiagnosticSectionUpdate {
                id,
                name: chapter.name,
                text: chapter.text,
                complete: chapter.complete,
                revision: chapter.revision,
                collected_at: chapter.collected_at,
            })
            .filter(DiagnosticSectionUpdate::valid)
            .collect();
        ensure!(
            updates
                .iter()
                .map(|chapter| chapter.text.len())
                .sum::<usize>()
                <= MAX_CHAPTER_BYTES,
            "diagnostic chapters exceed the job budget"
        );
        self.queue_sections(updates)
    }

    pub(super) fn queue_sections(&self, updates: Vec<DiagnosticSectionUpdate>) -> Result<()> {
        let Some(id) = updates.first().map(|update| update.id) else {
            return Ok(());
        };
        ensure!(
            updates
                .iter()
                .all(|update| update.id == id && update.valid()),
            "invalid diagnostic chapters"
        );
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        let sent: BTreeMap<String, u64> = state
            .get_json(&format!("diagnostics:sections:sent:{id}"))?
            .unwrap_or_default();
        let mut outbox: Vec<DiagnosticSectionUpdate> =
            state.get_json(SECTIONS_OUTBOX)?.unwrap_or_default();
        for update in updates {
            if sent
                .get(&update.name)
                .is_some_and(|revision| *revision >= update.revision)
            {
                continue;
            }
            if let Some(saved) = outbox
                .iter_mut()
                .find(|saved| saved.id == id && saved.name == update.name)
            {
                if saved.revision < update.revision && (!saved.complete || update.complete) {
                    *saved = update;
                }
            } else {
                outbox.push(update);
            }
        }
        ensure!(
            outbox
                .iter()
                .filter(|chapter| chapter.id == id)
                .map(|chapter| chapter.text.len())
                .sum::<usize>()
                <= MAX_CHAPTER_BYTES,
            "pending diagnostic chapters exceed the job budget"
        );
        state.set_json(SECTIONS_OUTBOX, &outbox)
    }

    pub(super) async fn flush_sections(&self, client: &PanelClient) -> Result<()> {
        let pending: Vec<DiagnosticSectionUpdate> = self.read(SECTIONS_OUTBOX)?.unwrap_or_default();
        // Keep each upload bounded and acknowledge only the submitted revision.
        for update in pending.into_iter().take(DIAGNOSTIC_SECTION_COUNT) {
            self.bounded(client.diagnostic_section(&update)).await?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            let mut outbox: Vec<DiagnosticSectionUpdate> =
                state.get_json(SECTIONS_OUTBOX)?.unwrap_or_default();
            outbox.retain(|saved| {
                saved.id != update.id
                    || saved.name != update.name
                    || saved.revision != update.revision
            });
            let key = format!("diagnostics:sections:sent:{}", update.id);
            let mut sent: BTreeMap<String, u64> = state.get_json(&key)?.unwrap_or_default();
            sent.entry(update.name)
                .and_modify(|saved| *saved = (*saved).max(update.revision))
                .or_insert(update.revision);
            state.set_json_batch(&[
                (SECTIONS_OUTBOX.into(), serde_json::to_value(outbox)?),
                (key, serde_json::to_value(sent)?),
            ])?;
        }
        Ok(())
    }
}

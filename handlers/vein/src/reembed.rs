// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The re-embed walk (EMBED, rmbp-ledger B317).
//!
//! A vector is only compared with vectors from the same model, so after the
//! embedder changes (or when an embedder appears after memories were stored
//! with recall off) the vault holds memories recall cannot see. `/reembed`
//! walks them: the vault answers bounded batches (`ReEmbed` → `ReEmbedBatch`),
//! Vein embeds each batch with the configured embedder and writes it back
//! (`ReEmbedWrite` → `ReEmbedDone`), one pass at a time, printing progress,
//! until nothing is stale or a pass makes no progress.

use bandy::SMessage;

use crate::provider::EmbedSlot;

/// Memories per pass (`vein.embed.reembed_batch` overrides).
pub const DEFAULT_REEMBED_BATCH: usize = 16;

struct Run {
    receipt: u64,
    tag: String,
    done: usize,
    total: Option<usize>,
}

/// The driver: owns the receipt ids of the walk in flight and of the start-up count.
#[derive(Default)]
pub struct ReEmbedDriver {
    run: Option<Run>,
    count_receipt: Option<u64>,
    count_tag: String,
    pub batch: usize,
}

impl ReEmbedDriver {
    pub fn new(batch: usize) -> Self {
        ReEmbedDriver { batch: batch.max(1), ..Default::default() }
    }

    pub fn running(&self) -> bool {
        self.run.is_some()
    }

    /// Ask the vault how many memories the configured embedder cannot see (start-up and on a
    /// `vein` preference change). `None` when recall is off.
    pub fn count(&mut self, receipt: u64, slot: &EmbedSlot) -> Option<SMessage> {
        if !slot.enabled() {
            return None;
        }
        self.count_receipt = Some(receipt);
        self.count_tag = slot.tag();
        Some(SMessage::ReEmbed { receipt_id: receipt, embed_model: slot.tag(), limit: 0 })
    }

    /// `/reembed`: the first pass, or the console line saying why not.
    pub fn start(&mut self, receipt: u64, slot: &EmbedSlot) -> Result<SMessage, String> {
        if let Some(off) = slot.recall_off() {
            return Err(format!("{off}\n"));
        }
        if self.run.is_some() {
            return Err(":: BRAIN :: REEMBED already running\n".into());
        }
        let tag = slot.tag();
        self.run = Some(Run { receipt, tag: tag.clone(), done: 0, total: None });
        Ok(SMessage::ReEmbed { receipt_id: receipt, embed_model: tag, limit: self.batch })
    }

    /// The vault's batch. Answers the console lines and the next message.
    pub async fn on_batch(
        &mut self,
        receipt: u64,
        items: Vec<(u64, String)>,
        stale_total: usize,
        slot: &EmbedSlot,
    ) -> (Vec<String>, Option<SMessage>) {
        if self.count_receipt == Some(receipt) {
            self.count_receipt = None;
            if stale_total == 0 {
                return (vec![], None);
            }
            return (
                vec![format!(
                    ":: BRAIN :: VAULT {stale_total} memories not embedded by {} — never compared; /reembed re-embeds them\n",
                    self.count_tag
                )],
                None,
            );
        }
        let Some(run) = self.run.as_mut().filter(|r| r.receipt == receipt) else { return (vec![], None) };
        if run.total.is_none() {
            run.total = Some(stale_total);
        }
        if items.is_empty() {
            let done = run.done;
            self.run = None;
            return (vec![format!(":: BRAIN :: REEMBED complete :: {done} re-embedded, 0 stale\n")], None);
        }
        if slot.tag() != run.tag {
            self.run = None;
            return (vec![":: BRAIN :: REEMBED stopped :: the embedder changed mid-walk — run /reembed again\n".into()], None);
        }
        let texts: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
        match slot.embed_many(&texts).await {
            Ok(vs) => {
                let vectors = items.iter().map(|(id, _)| *id).zip(vs.into_iter().map(|e| e.vector)).collect();
                (vec![], Some(SMessage::ReEmbedWrite { receipt_id: receipt, embed_model: run.tag.clone(), vectors }))
            }
            Err(e) => {
                self.run = None;
                (vec![format!(":: BRAIN :: REEMBED stopped :: {e}\n")], None)
            }
        }
    }

    /// The vault wrote a batch: progress, then the next pass or the end.
    pub fn on_done(
        &mut self,
        receipt: u64,
        written: usize,
        remaining: usize,
        error: Option<String>,
    ) -> (Vec<String>, Option<SMessage>) {
        let Some(run) = self.run.as_mut().filter(|r| r.receipt == receipt) else { return (vec![], None) };
        if let Some(e) = error {
            self.run = None;
            return (vec![format!(":: BRAIN :: REEMBED stopped :: vault :: {e}\n")], None);
        }
        run.done += written;
        let total = run.total.unwrap_or(run.done + remaining);
        let line = format!(":: BRAIN :: REEMBED {}/{} ({} stale left)\n", run.done, total, remaining);
        if remaining == 0 {
            self.run = None;
            return (vec![line, ":: BRAIN :: REEMBED complete\n".into()], None);
        }
        if written == 0 {
            self.run = None;
            return (vec![line, ":: BRAIN :: REEMBED stopped :: a pass made no progress\n".into()], None);
        }
        let next = SMessage::ReEmbed { receipt_id: receipt, embed_model: run.tag.clone(), limit: self.batch };
        (vec![line], Some(next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gneiss_pal::api::{BoxFuture, Embedder, NoEmbedder, ProviderError};
    use std::sync::Arc;

    struct Fake;
    impl Embedder for Fake {
        fn name(&self) -> &str {
            "local"
        }
        fn model(&self) -> &str {
            "fake"
        }
        fn dims(&self) -> usize {
            2
        }
        fn embed<'a>(&'a self, texts: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<Vec<f32>>, ProviderError>> {
            Box::pin(async move { Ok(texts.iter().map(|t| vec![t.len() as f32, 1.0]).collect()) })
        }
    }

    #[test]
    fn walks_in_bounded_passes_with_progress() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let slot = EmbedSlot::from_embedder(Arc::new(Fake));
        let mut d = ReEmbedDriver::new(2);
        let first = d.start(9, &slot).unwrap();
        assert!(matches!(first, SMessage::ReEmbed { receipt_id: 9, ref embed_model, limit: 2 } if embed_model == "local/fake"));
        assert!(d.start(10, &slot).is_err());

        let items = vec![(1, "aa".to_string()), (2, "bbb".to_string())];
        let (lines, next) = rt.block_on(d.on_batch(9, items, 3, &slot));
        assert!(lines.is_empty());
        match next {
            Some(SMessage::ReEmbedWrite { receipt_id: 9, embed_model, vectors }) => {
                assert_eq!(embed_model, "local/fake");
                assert_eq!(vectors, vec![(1, vec![2.0, 1.0]), (2, vec![3.0, 1.0])]);
            }
            other => panic!("{other:?}"),
        }
        let (lines, next) = d.on_done(9, 2, 1, None);
        assert_eq!(lines, vec![":: BRAIN :: REEMBED 2/3 (1 stale left)\n".to_string()]);
        assert!(matches!(next, Some(SMessage::ReEmbed { limit: 2, .. })));
        let (_, next) = rt.block_on(d.on_batch(9, vec![(3, "c".into())], 1, &slot));
        assert!(matches!(next, Some(SMessage::ReEmbedWrite { .. })));
        let (lines, next) = d.on_done(9, 1, 0, None);
        assert_eq!(lines[0], ":: BRAIN :: REEMBED 3/3 (0 stale left)\n");
        assert!(next.is_none() && !d.running());
    }

    #[test]
    fn recall_off_refuses_and_count_reports_stale() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let off = EmbedSlot::from_embedder(Arc::new(NoEmbedder));
        let mut d = ReEmbedDriver::new(4);
        assert_eq!(
            d.start(1, &off).unwrap_err(),
            ":: BRAIN :: RECALL OFF :: no embedder — set vein.embed.provider\n"
        );
        assert!(d.count(2, &off).is_none());

        let on = EmbedSlot::from_embedder(Arc::new(Fake));
        assert!(matches!(d.count(3, &on), Some(SMessage::ReEmbed { limit: 0, .. })));
        let (lines, next) = rt.block_on(d.on_batch(3, vec![], 7, &on));
        assert_eq!(
            lines,
            vec![":: BRAIN :: VAULT 7 memories not embedded by local/fake — never compared; /reembed re-embeds them\n".to_string()]
        );
        assert!(next.is_none());
    }
}

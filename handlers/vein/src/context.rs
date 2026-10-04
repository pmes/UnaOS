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

use gneiss_pal::api::{ChatRequest, ModelProvider, Part, StopReason};

/// The engram compressor. The prompt text is `vein_core::context::engram_prompt` (VEINCORE, B304): the
/// pure half of the assembler lives in the shared core, byte-identical on host and metal; only the
/// network call stays here.
pub async fn compress_into_engram(
    provider: &dyn ModelProvider,
    user_prompt: &str,
    ai_response: &str,
) -> Result<String, String> {
    // merge10 fold: VEINCORE (B304) owns the prompt text — `vein_core::context::engram_prompt` carries the
    // instruction and the history block, byte-identical on host and metal — and VEINPROV (B303) owns the
    // call: one provider-neutral request, the stop reason read before the text.
    let req = ChatRequest::single(None, vec![Part::text(vein_core::context::engram_prompt(user_prompt, ai_response))]);
    let resp = provider.generate(&req).await.map_err(|e| e.to_string())?;
    match resp.stop {
        StopReason::EndTurn | StopReason::MaxTokens => Ok(resp.text),
        other => Err(format!("engram compression stopped: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    /// VEINCORE: the prompt the host sends is the shared core's, and it is the text this file used to
    /// build inline (the instruction, then the history block).
    #[test]
    fn engram_prompt_comes_from_the_shared_core() {
        let p = vein_core::context::engram_prompt("fix the cortex", "done");
        assert!(p.starts_with(vein_core::context::ENGRAM_SYSTEM));
        assert!(p.ends_with("\n\n[CONVERSATION HISTORY TO COMPRESS]:\nUser: fix the cortex\n\nAI: done\n"));
    }

    /// The Messages request a metal caller of Vein (LUMEN.ELF) builds is the shared core's encoder; the
    /// host sees the same bytes (LUMENAPP B323).
    #[test]
    fn shared_core_encodes_the_messages_request() {
        use vein_core::claude::{encode_body, Msg, Params};
        let mut b = [0u8; 256];
        let n = encode_body(&Params::new("claude-opus-5-5", 16, ""), [Msg { role: vein_core::Role::User, text: "hi" }].into_iter(), &mut b).unwrap();
        assert!(core::str::from_utf8(&b[..n]).unwrap().ends_with(r#""messages":[{"role":"user","content":"hi"}]}"#));
    }
}

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

use gneiss_pal::api::{Content, Part, ResilientClient};

/// The engram compressor. The prompt text is `vein_core::context::engram_prompt` (VEINCORE, B304): the
/// pure half of the assembler lives in the shared core, byte-identical on host and metal; only the
/// network call stays here.
pub async fn compress_into_engram(
    client: &mut ResilientClient,
    user_prompt: &str,
    ai_response: &str,
) -> Result<String, String> {
    let request_contents = vec![Content {
        role: "user".to_string(),
        parts: vec![Part::text(vein_core::context::engram_prompt(user_prompt, ai_response))],
    }];

    let (response, _) = client.generate_content(&request_contents).await?;
    Ok(response)
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

    /// The chat wire the host speaks to a metal VEIN.BIN is the same codec (KATs shared).
    #[test]
    fn chat_wire_kats_pass_on_the_host() {
        let (p, t) = vein_core::wire::kats();
        assert_eq!(p, t);
    }
}

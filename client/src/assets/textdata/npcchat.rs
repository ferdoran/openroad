//! NPC dialog speech table (`npcchat.txt`): maps an NPC codename to the two
//! `SN_*` string ids of its dialog speech — the greeting shown when the
//! dialog opens (`_BS`) and the chat page shown for the "Talk to this
//! person." option (`_PS`). The strings themselves live in
//! `textquest_speech&name.txt`.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct NpcChat(pub HashMap<String, NpcChatEntry>);

#[derive(Debug, Clone)]
pub struct NpcChatEntry {
    /// Greeting string id (`SN_..._BS`).
    pub greeting_key: String,
    /// "Talk to this person." page string id (`SN_..._PS`).
    pub talk_key: String,
}

impl NpcChat {
    /// Parse the tab-separated file content (already decoded from UTF-16).
    /// Rows: `service \t codename \t msg1_strid \t msg2_strid`. Service-0
    /// rows are kept: the flag gates server-side chat, but their string ids
    /// are valid and minor NPCs still greet with them.
    pub fn parse(content: &str) -> Self {
        let entries = content
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                if fields.len() < 4 || fields[0].trim_start_matches('\u{feff}').starts_with("//") {
                    return None;
                }
                let codename = fields[1].trim();
                if codename.is_empty() {
                    return None;
                }
                Some((
                    codename.to_string(),
                    NpcChatEntry {
                        greeting_key: fields[2].trim().to_string(),
                        talk_key: fields[3].trim().to_string(),
                    },
                ))
            })
            .collect();
        Self(entries)
    }

    pub fn get(&self, codename: &str) -> Option<&NpcChatEntry> {
        self.0.get(codename)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_npc_chat_rows() {
        let content = "\u{feff}//Service\tOwnerCodeName_128\tmsg1\tmsg2\r\n\
                       1\tNPC_CH_SMITH\tSN_NPC_CH_SMITH_BS\tSN_NPC_CH_SMITH_PS\r\n\
                       0\tNPC_CH_MINISTER\tSN_A\tSN_B\r\n";
        let chat = NpcChat::parse(content);
        let smith = chat.get("NPC_CH_SMITH").unwrap();
        assert_eq!(smith.greeting_key, "SN_NPC_CH_SMITH_BS");
        assert_eq!(smith.talk_key, "SN_NPC_CH_SMITH_PS");
        // service-0 rows still carry valid speech keys and are kept
        assert_eq!(chat.get("NPC_CH_MINISTER").unwrap().greeting_key, "SN_A");
        assert_eq!(chat.0.len(), 2);
    }
}
